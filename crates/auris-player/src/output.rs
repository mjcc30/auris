//! Sortie audio cross-plateforme via cpal:
//! ALSA (Raspberry Pi / Linux), CoreAudio (macOS), WASAPI (Windows).
//!
//! - PCM: on pousse du f32 (cpal convertit vers le format du DAC).
//! - DSD: on pousse des trames DoP 24 bits vers un flux I24 (ou I32 decale),
//!   ce qui delivre le DSD natif (jusqu'au DSD256) sur les DAC compatibles DoP.
//! - Aucun re-echantillonnage: la cadence du flux = cadence requise par la piste.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use auris_core::{AudioDeviceInfo, AurisError, BrowserFrame, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedStreamConfigRange, I24,
};
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use tokio::sync::broadcast;

use crate::decoder::{Chunk, ChunkData, OutputSpec};

pub type SharedRb = Arc<HeapRb<Chunk>>;
pub type Prod = HeapProd<Chunk>;
pub type Cons = HeapCons<Chunk>;

/// Cree l'anneau producteur/consommateur partage entre le worker et le callback cpal.
pub fn make_ring(capacity_chunks: usize) -> (Prod, Cons) {
    Arc::new(HeapRb::<Chunk>::new(capacity_chunks)).split()
}

/// Conversion d'un echantillon vers le type de sortie cpal.
trait WriteOut: SizedSample + Send + 'static {
    fn from_f32(v: f32, gain: f32) -> Self;
    /// Conversion inverse (pour le flux navigateur) : échantillon normalisé [-1, 1].
    fn to_f32(self) -> f32;
    /// v = mot 24 bits DoP (marqueur dans l'octet haut). Le gain ne s'applique
    /// qu'aux 16 bits de donnees, jamais au marqueur.
    fn from_dop24(v: i32, gain: f32) -> Self;
    fn zero() -> Self;
}

impl WriteOut for f32 {
    fn from_f32(v: f32, gain: f32) -> Self {
        v * gain
    }
    fn to_f32(self) -> f32 {
        self
    }
    fn from_dop24(_v: i32, _gain: f32) -> Self {
        0.0
    }
    fn zero() -> Self {
        0.0
    }
}
impl WriteOut for i16 {
    fn from_f32(v: f32, gain: f32) -> Self {
        (v * gain * i16::MAX as f32) as i16
    }
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
    fn from_dop24(v: i32, gain: f32) -> Self {
        let bits = ((v & 0xFFFF) as f32 * gain) as i32 & 0xFFFF;
        (((v & 0xFF0000) | bits) >> 8) as i16
    }
    fn zero() -> Self {
        0
    }
}
impl WriteOut for i32 {
    fn to_f32(self) -> f32 {
        self as f32 / i32::MAX as f32
    }
    fn from_f32(v: f32, gain: f32) -> Self {
        (v * gain * i32::MAX as f32) as i32
    }
    fn from_dop24(v: i32, gain: f32) -> Self {
        let bits = ((v & 0xFFFF) as f32 * gain) as i32 & 0xFFFF;
        ((v & 0xFF0000) | bits) << 8
    }
    fn zero() -> Self {
        0
    }
}
impl WriteOut for I24 {
    fn to_f32(self) -> f32 {
        self.inner() as f32 / (1 << 23) as f32
    }
    fn from_f32(v: f32, gain: f32) -> Self {
        I24::new_unchecked((v * gain * 8_388_607.0) as i32)
    }
    fn from_dop24(v: i32, gain: f32) -> Self {
        let bits = ((v & 0xFFFF) as f32 * gain) as i32 & 0xFFFF;
        I24::new_unchecked((v & 0xFF0000) | bits)
    }
    fn zero() -> Self {
        I24::new_unchecked(0)
    }
}

/// Nom d'un peripherique cpal 0.18 (via sa description).
fn dev_name(d: &Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

/// Reste d'un chunk non encore consomme (le buffer cpal peut etre plus petit qu'un chunk).
enum Stash {
    F32(Vec<f32>, usize),
    Dop(Vec<i32>, usize),
}

pub struct AudioOutput {
    device: Device,
    device_name: String,
    stream: Option<Stream>,
}

impl AudioOutput {
    /// Liste les peripheriques de sortie (DAC) avec leurs capacites.
    pub fn devices() -> Vec<AudioDeviceInfo> {
        let host = cpal::default_host();
        let default_name = host.default_output_device().and_then(|d| dev_name(&d));
        let mut out = Vec::new();
        if let Ok(devices) = host.output_devices() {
            for d in devices {
                let name = dev_name(&d).unwrap_or_else(|| "Peripherique inconnu".into());
                let mut rates: Vec<u32> = Vec::new();
                let mut supports_i24 = false;
                let mut supports_f32 = false;
                let mut max_channels = 0u16;
                if let Ok(configs) = d.supported_output_configs() {
                    for c in configs {
                        rates.push(c.min_sample_rate());
                        rates.push(c.max_sample_rate());
                        match c.sample_format() {
                            SampleFormat::I24 => supports_i24 = true,
                            SampleFormat::F32 => supports_f32 = true,
                            _ => {}
                        }
                        max_channels = max_channels.max(c.channels());
                    }
                }
                rates.sort_unstable();
                rates.dedup();
                out.push(AudioDeviceInfo {
                    name: name.clone(),
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    sample_rates: rates,
                    supports_i24,
                    supports_f32,
                    max_channels,
                });
            }
        }
        out
    }

    pub fn new(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(n) => {
                let devices = host
                    .output_devices()
                    .map_err(|e| AurisError::Output(e.to_string()))?;
                devices
                    .into_iter()
                    .find(|d| dev_name(d).map(|x| x == n).unwrap_or(false))
                    .ok_or_else(|| AurisError::DeviceNotFound(n.to_string()))?
            }
            None => host
                .default_output_device()
                .ok_or(AurisError::NoOutputDevice)?,
        };
        let name = dev_name(&device).unwrap_or_else(|| "default".into());
        Ok(AudioOutput {
            device,
            device_name: name,
            stream: None,
        })
    }

    pub fn name(&self) -> &str {
        &self.device_name
    }

    /// Construit (et remplace) le flux de sortie pour un spec donne.
    #[allow(clippy::too_many_arguments)]
    pub fn build_stream(
        &mut self,
        spec: &OutputSpec,
        mut cons: Cons,
        volume: Arc<AtomicU8>,
        muted: Arc<AtomicBool>,
        frames_out: Arc<AtomicU64>,
        gen: Arc<AtomicU64>,
        max_volume: Arc<AtomicU8>,
        curve: Arc<AtomicU8>,
        vol_mode: Arc<AtomicU8>,
        // 0 = DAC local, 1 = « Jouer ici » (PCM vers le navigateur, DAC muet).
        out_mode: Arc<AtomicU8>,
        browser_tx: broadcast::Sender<BrowserFrame>,
    ) -> Result<()> {
        self.stream = None; // stoppe l'ancien flux
        let configs: Vec<SupportedStreamConfigRange> = self
            .device
            .supported_output_configs()
            .map_err(|e| AurisError::Output(e.to_string()))?
            .collect();
        let want_formats: &[SampleFormat] = if spec.is_dsd_dop {
            &[SampleFormat::I24, SampleFormat::I32]
        } else {
            &[SampleFormat::F32, SampleFormat::I32, SampleFormat::I16]
        };
        // 1) config exacte: canaux + cadence + format prefere
        let mut chosen: Option<SupportedStreamConfigRange> = None;
        'outer: for fmt in want_formats {
            for c in &configs {
                if c.channels() == spec.channels as u16
                    && c.min_sample_rate() <= spec.rate
                    && spec.rate <= c.max_sample_rate()
                    && c.sample_format() == *fmt
                {
                    chosen = Some(*c);
                    break 'outer;
                }
            }
        }
        // 2) sinon: cadence ok, canaux differents (adaptation de canaux dans le callback)
        if chosen.is_none() {
            'outer2: for fmt in want_formats {
                for c in &configs {
                    if c.min_sample_rate() <= spec.rate
                        && spec.rate <= c.max_sample_rate()
                        && c.sample_format() == *fmt
                    {
                        chosen = Some(*c);
                        break 'outer2;
                    }
                }
            }
        }
        let range = chosen.ok_or_else(|| {
            AurisError::Output(format!(
                "le peripherique '{}' ne supporte pas {} Hz (plages: {:?})",
                self.device_name,
                spec.rate,
                configs
                    .iter()
                    .map(|c| (c.min_sample_rate(), c.max_sample_rate()))
                    .collect::<Vec<_>>()
            ))
        })?;
        let sample_format = range.sample_format();
        let out_channels = range.channels() as usize;
        let stream_config = StreamConfig {
            channels: range.channels(),
            sample_rate: spec.rate,
            buffer_size: BufferSize::Default,
        };
        let src_channels = spec.channels as usize;
        // Le flux navigateur ne transporte que du PCM f32 (pas de trames DoP).
        let forward_browser = !spec.is_dsd_dop;

        macro_rules! start {
            ($t:ty) => {{
                let volume = volume.clone();
                let muted = muted.clone();
                let frames_out = frames_out.clone();
                let gen = gen.clone();
                let max_volume = max_volume.clone();
                let curve = curve.clone();
                let vol_mode = vol_mode.clone();
                let out_mode = out_mode.clone();
                let browser_tx = browser_tx.clone();
                // stash persistant entre les appels du callback: un chunk plus gros
                // que le buffer cpal est consomme sur plusieurs appels.
                let mut stash: Option<Stash> = None;
                let stream = self
                    .device
                    .build_output_stream::<$t, _, _>(
                        stream_config.clone(),
                        move |data: &mut [$t], _info| {
                            let gain = compute_gain(
                                volume.load(Ordering::Relaxed),
                                max_volume.load(Ordering::Relaxed),
                                curve.load(Ordering::Relaxed),
                                vol_mode.load(Ordering::Relaxed),
                                muted.load(Ordering::Relaxed),
                            );
                            let cur_gen = gen.load(Ordering::Relaxed);
                            let mut i = 0usize;
                            let mut frames_written = 0u64;
                            while i < data.len() {
                                if stash.is_none() {
                                    match cons.try_pop() {
                                        Some(chunk) if chunk.gen == cur_gen => {
                                            stash = Some(match chunk.data {
                                                ChunkData::F32(v) => Stash::F32(v, 0),
                                                ChunkData::I24Dop(v) => Stash::Dop(v, 0),
                                            });
                                        }
                                        Some(_) => continue, // generation perimee (seek)
                                        None => {
                                            // underrun: silence
                                            while i < data.len() {
                                                data[i] = <$t>::zero();
                                                i += 1;
                                            }
                                            frames_written +=
                                                (data.len() / out_channels.max(1)) as u64;
                                        }
                                    }
                                }
                                let done = match &mut stash {
                                    Some(Stash::F32(v, off)) => {
                                        let frames = v.len() / src_channels;
                                        while *off / src_channels < frames && i < data.len() {
                                            let f = *off / src_channels;
                                            for oc in 0..out_channels {
                                                if i >= data.len() {
                                                    break;
                                                }
                                                let ic = oc.min(src_channels - 1);
                                                data[i] =
                                                    <$t>::from_f32(v[f * src_channels + ic], gain);
                                                i += 1;
                                            }
                                            *off += src_channels;
                                            frames_written += 1;
                                        }
                                        *off >= v.len()
                                    }
                                    Some(Stash::Dop(v, off)) => {
                                        let frames = v.len() / src_channels;
                                        while *off / src_channels < frames && i < data.len() {
                                            let f = *off / src_channels;
                                            for oc in 0..out_channels {
                                                if i >= data.len() {
                                                    break;
                                                }
                                                let ic = oc.min(src_channels - 1);
                                                data[i] = <$t>::from_dop24(
                                                    v[f * src_channels + ic],
                                                    gain,
                                                );
                                                i += 1;
                                            }
                                            *off += src_channels;
                                            frames_written += 1;
                                        }
                                        *off >= v.len()
                                    }
                                    None => true,
                                };
                                if done {
                                    stash = None;
                                }
                            }
                            // « Jouer ici » : on forward le PCM post-gain au
                            // navigateur, et le DAC local reçoit du silence.
                            if forward_browser && out_mode.load(Ordering::Relaxed) == 1 {
                                let mut v: Vec<f32> = Vec::with_capacity(data.len());
                                for s in data.iter() {
                                    v.push(s.to_f32());
                                }
                                // send() est non bloquant; Err si aucun client connecté.
                                let _ = browser_tx.send(BrowserFrame::Pcm(v.into()));
                                for s in data.iter_mut() {
                                    *s = <$t>::zero();
                                }
                            }
                            frames_out.fetch_add(frames_written, Ordering::Relaxed);
                        },
                        |err| {
                            tracing::error!("erreur flux audio: {err}");
                        },
                        None,
                    )
                    .map_err(|e| AurisError::Output(e.to_string()))?;
                stream
                    .play()
                    .map_err(|e| AurisError::Output(e.to_string()))?;
                self.stream = Some(stream);
            }};
        }

        match sample_format {
            SampleFormat::F32 => start!(f32),
            SampleFormat::I16 => start!(i16),
            SampleFormat::I32 => start!(i32),
            SampleFormat::I24 => start!(I24),
            other => {
                return Err(AurisError::Output(format!(
                    "format de sortie non gere: {other:?}"
                )))
            }
        }
        Ok(())
    }

    pub fn pause(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.pause();
        }
    }
    pub fn resume(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.play();
        }
    }
    pub fn stop(&mut self) {
        self.stream = None;
    }
}

/// Gain appliqué dans le callback audio, selon les réglages playback.
///
/// - `vol_mode == 1` (bit-perfect / disabled) : gain fixe de 1.0, aucun traitement.
/// - `muted` : 0.0 (uniquement en mode logiciel).
/// - sinon `volume` est borné par `max_volume`, puis la courbe est appliquée :
///   `curve == 1` → linéaire (v/100), sinon logarithmique ((v/100)^2).
pub fn compute_gain(volume: u8, max_volume: u8, curve: u8, vol_mode: u8, muted: bool) -> f32 {
    if vol_mode == 1 {
        return 1.0;
    }
    if muted {
        return 0.0;
    }
    let max = max_volume.max(1) as f32;
    let v = (volume as f32 / 100.0).min(max / 100.0);
    if curve == 1 {
        v
    } else {
        v * v
    }
}

#[cfg(test)]
mod tests {
    use super::compute_gain;

    #[test]
    fn logiciel_log_par_defaut() {
        // courbe log (0) : 50% -> 0.25
        assert!((compute_gain(50, 100, 0, 0, false) - 0.25).abs() < 1e-6);
        assert!((compute_gain(100, 100, 0, 0, false) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn logiciel_lineaire() {
        assert!((compute_gain(50, 100, 1, 0, false) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn max_volume_borne_le_gain() {
        // volume 80 mais max 50 -> gain calculé sur 50
        assert!((compute_gain(80, 50, 1, 0, false) - 0.5).abs() < 1e-6);
        assert!((compute_gain(80, 50, 0, 0, false) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn bitperfect_ignore_volume_et_mute() {
        assert_eq!(compute_gain(0, 100, 0, 1, false), 1.0);
        assert_eq!(compute_gain(37, 40, 1, 1, true), 1.0);
    }

    #[test]
    fn mute_coupe_en_logiciel() {
        assert_eq!(compute_gain(80, 100, 0, 0, true), 0.0);
    }
}
