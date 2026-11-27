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
use crate::eq::{EqConfig, EqProcessor};

pub type SharedRb = Arc<HeapRb<Chunk>>;
pub type Prod = HeapProd<Chunk>;
pub type Cons = HeapCons<Chunk>;

/// Cree l'anneau producteur/consommateur partage entre le worker et le callback cpal.
pub fn make_ring(capacity_chunks: usize) -> (Prod, Cons) {
    Arc::new(HeapRb::<Chunk>::new(capacity_chunks)).split()
}

/// Paramètres partagés entre le callback cpal et le thread WASAPI exclusif.
pub struct RenderParams {
    pub volume: Arc<AtomicU8>,
    pub muted: Arc<AtomicBool>,
    pub frames_out: Arc<AtomicU64>,
    pub gen: Arc<AtomicU64>,
    pub max_volume: Arc<AtomicU8>,
    pub curve: Arc<AtomicU8>,
    pub vol_mode: Arc<AtomicU8>,
    pub out_mode: Arc<AtomicU8>,
    pub browser_tx: broadcast::Sender<BrowserFrame>,
    pub eq: Arc<std::sync::RwLock<EqConfig>>,
}

/// Remplit `data` (taille = trames * canaux de sortie) à partir de l'anneau de
/// chunks : conversion f32/DoP -> T, EQ paramétrique, gain, adaptation de
/// canaux, forward « Jouer ici » vers le navigateur. Retourne le nombre de
/// trames écrites (underrun -> silence, mais `data` est toujours rempli).
/// Partagé entre le callback cpal (mode partagé) et le worker WASAPI exclusif.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_samples<T: WriteOut>(
    data: &mut [T],
    stash: &mut Option<Stash>,
    cons: &mut Cons,
    gen: &AtomicU64,
    src_channels: usize,
    out_channels: usize,
    eq: &std::sync::RwLock<EqConfig>,
    eq_proc: &mut EqProcessor,
    eq_rate: u32,
    gain: f32,
    forward_browser: bool,
    out_mode: &AtomicU8,
    browser_tx: &broadcast::Sender<BrowserFrame>,
    frames_out: &AtomicU64,
) -> u64 {
    let eq_cfg = eq.read().unwrap();
    let cur_gen = gen.load(Ordering::Relaxed);
    let mut i = 0usize;
    let mut frames_written = 0u64;
    while i < data.len() {
        if stash.is_none() {
            match cons.try_pop() {
                Some(chunk) if chunk.gen == cur_gen => {
                    *stash = Some(match chunk.data {
                        ChunkData::F32(v) => Stash::F32(v, 0),
                        ChunkData::I24Dop(v) => Stash::Dop(v, 0),
                    });
                }
                Some(_) => continue, // generation perimee (seek)
                None => {
                    // underrun: silence
                    while i < data.len() {
                        data[i] = T::zero();
                        i += 1;
                    }
                    frames_written += (data.len() / out_channels.max(1)) as u64;
                }
            }
        }
        let done = match &mut *stash {
            Some(Stash::F32(v, off)) => {
                let frames = v.len() / src_channels;
                while *off / src_channels < frames && i < data.len() {
                    let f = *off / src_channels;
                    for oc in 0..out_channels {
                        if i >= data.len() {
                            break;
                        }
                        let ic = oc.min(src_channels - 1);
                        // EQ paramétrique avant le gain de volume
                        let s = eq_proc.process(
                            &eq_cfg,
                            eq_rate,
                            src_channels,
                            ic,
                            v[f * src_channels + ic],
                        );
                        data[i] = T::from_f32(s, gain);
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
                        data[i] = T::from_dop24(v[f * src_channels + ic], gain);
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
            *stash = None;
        }
    }
    // « Jouer ici » : on forward le PCM post-gain au navigateur, et la sortie
    // locale reçoit du silence.
    if forward_browser && out_mode.load(Ordering::Relaxed) == 1 {
        let mut v: Vec<f32> = Vec::with_capacity(data.len());
        for s in data.iter() {
            v.push(s.to_f32());
        }
        // send() est non bloquant; Err si aucun client connecté.
        let _ = browser_tx.send(BrowserFrame::Pcm(v.into()));
        for s in data.iter_mut() {
            *s = T::zero();
        }
    }
    frames_out.fetch_add(frames_written, Ordering::Relaxed);
    frames_written
}

/// Conversion d'un echantillon vers le type de sortie (cpal ou WASAPI exclusif).
pub(crate) trait WriteOut: SizedSample + Send + 'static {
    fn from_f32(v: f32, gain: f32) -> Self;
    /// Conversion inverse (pour le flux navigateur) : échantillon normalisé [-1, 1].
    fn to_f32(self) -> f32;
    /// v = mot 24 bits DoP (marqueur dans l'octet haut, 16 bits DSD en bas).
    /// **Bit-perfect** : le gain ne s'applique JAMAIS a un flux DoP (spec DoP:
    /// rien ne doit alterer les trames en transit) — le mot 24 bits est
    /// transmis tel quel, quel que soit `gain`.
    fn from_dop24(v: i32, gain: f32) -> Self;
    fn zero() -> Self;
    /// Sérialise l'échantillon en little-endian dans `out`.
    /// `packed24` = conteneur 24 bits « packed » 3 octets (on écrit les 3 octets
    /// hauts d'un i32 24-in-32).
    // utilisé par le chemin WASAPI exclusif (Windows) et les tests Windows
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn push_le(self, out: &mut Vec<u8>, packed24: bool);
}

impl WriteOut for f32 {
    fn from_f32(v: f32, gain: f32) -> Self {
        v * gain
    }
    fn to_f32(self) -> f32 {
        self
    }
    fn from_dop24(_v: i32, _gain: f32) -> Self {
        // un conteneur float ne peut pas porter de DoP : silence
        0.0
    }
    fn zero() -> Self {
        0.0
    }
    fn push_le(self, out: &mut Vec<u8>, _packed24: bool) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}
impl WriteOut for i16 {
    fn from_f32(v: f32, gain: f32) -> Self {
        (v * gain * i16::MAX as f32) as i16
    }
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
    fn from_dop24(v: i32, _gain: f32) -> Self {
        // conteneur 16 bits : le DoP 24 bits n'y tient pas, on garde le motif
        // (marqueur dans l'octet haut) sans toucher aux donnees.
        (v >> 8) as i16
    }
    fn zero() -> Self {
        0
    }
    fn push_le(self, out: &mut Vec<u8>, _packed24: bool) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}
impl WriteOut for i32 {
    fn to_f32(self) -> f32 {
        self as f32 / i32::MAX as f32
    }
    fn from_f32(v: f32, gain: f32) -> Self {
        (v * gain * i32::MAX as f32) as i32
    }
    fn from_dop24(v: i32, _gain: f32) -> Self {
        // 24-in-32 : le mot DoP 24 bits est aligne sur les bits forts du
        // conteneur 32 bits, tel quel (bit-perfect).
        v << 8
    }
    fn zero() -> Self {
        0
    }
    fn push_le(self, out: &mut Vec<u8>, packed24: bool) {
        let b = self.to_le_bytes();
        if packed24 {
            // 24 bits packed = les 3 octets hauts du mot 32 bits
            out.extend_from_slice(&b[1..4]);
        } else {
            out.extend_from_slice(&b);
        }
    }
}
impl WriteOut for I24 {
    fn to_f32(self) -> f32 {
        self.inner() as f32 / (1 << 23) as f32
    }
    fn from_f32(v: f32, gain: f32) -> Self {
        I24::new_unchecked((v * gain * 8_388_607.0) as i32)
    }
    fn from_dop24(v: i32, _gain: f32) -> Self {
        // le mot 24 bits DoP tel quel, dans les 24 bits bas du I24
        I24::new_unchecked(v)
    }
    fn zero() -> Self {
        I24::new_unchecked(0)
    }
    fn push_le(self, out: &mut Vec<u8>, packed24: bool) {
        let v = self.inner();
        if packed24 {
            out.extend_from_slice(&v.to_le_bytes()[1..4]);
        } else {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// Nom d'un peripherique cpal 0.18 (via sa description).
fn dev_name(d: &Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

/// Valide le nom de peripherique demande contre la liste des peripheriques
/// visibles (`AudioOutput::devices`).
///
/// - `None` ou chaîne vide -> `Ok(None)` = peripherique par defaut.
/// - `Some(n)` present dans la liste -> `Ok(Some(n))`.
/// - sinon -> `Err(DeviceNotFound)` avec la liste des noms disponibles, pour
///   eviter d'ecrire un nom invalide dans la config persistee.
pub fn resolve_device_name(
    available: &[AudioDeviceInfo],
    want: Option<&str>,
) -> Result<Option<String>> {
    match want.map(str::trim) {
        None | Some("") => Ok(None),
        Some(n) => {
            if available.iter().any(|d| d.name == n) {
                Ok(Some(n.to_string()))
            } else {
                let list = available
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(AurisError::DeviceNotFound(format!(
                    "{n} (disponibles: {list})"
                )))
            }
        }
    }
}

/// Reste d'un chunk non encore consomme (le buffer cpal peut etre plus petit qu'un chunk).
pub(crate) enum Stash {
    F32(Vec<f32>, usize),
    Dop(Vec<i32>, usize),
}

pub struct AudioOutput {
    /// `None` = sortie « navigateur uniquement » (aucun périphérique cpal).
    device: Option<Device>,
    device_name: String,
    stream: Option<Stream>,
    drain_stop: Option<Arc<AtomicBool>>,
    drain: Option<std::thread::JoinHandle<()>>,
    paused: Arc<AtomicBool>,
    /// Porteuse DoP du dernier flux DSD joué (Some(rate) si flux DoP).
    dop_carrier: Option<u32>,
    /// Mode WASAPI exclusif demandé (Windows) — bit-perfect, repli cpal sinon.
    exclusive: bool,
    /// Flux WASAPI exclusif actif (si ouvert avec succès).
    exclusive_stream: Option<crate::wasapi_exclusive::ExclusiveStream>,
    /// Backend réellement utilisé pour le dernier flux : "cpal", "exclusif", "navigateur".
    backend: &'static str,
}

impl AudioOutput {
    /// Liste les peripheriques de sortie (DAC) de TOUS les hosts cpal
    /// (WASAPI + autres sous Windows, ALSA + autres sous Linux...), sans
    /// doublons: un DAC USB branche apres le demarrage apparait ainsi dans
    /// la liste meme s'il n'etait pas present au lancement.
    pub fn devices() -> Vec<AudioDeviceInfo> {
        let mut out: Vec<AudioDeviceInfo> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut defaults: std::collections::HashSet<String> = std::collections::HashSet::new();
        for host_id in cpal::ALL_HOSTS {
            let Ok(host) = cpal::host_from_id(*host_id) else {
                continue;
            };
            if let Some(d) = host.default_output_device() {
                if let Some(n) = dev_name(&d) {
                    defaults.insert(n);
                }
            }
            let Ok(devices) = host.output_devices() else {
                continue;
            };
            for d in devices {
                let name = dev_name(&d).unwrap_or_else(|| "Peripherique inconnu".into());
                if !seen.insert(name.clone()) {
                    continue;
                }
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
                    is_default: defaults.contains(&name),
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
        // Valide le nom demande contre la liste visible (tous hosts), puis
        // retrouve le Device cpal correspondant dans n'importe quel host :
        // un DAC liste ne doit jamais etre "introuvable" a l'ouverture.
        let wanted = resolve_device_name(&Self::devices(), device_name)?;
        let device = match &wanted {
            Some(n) => {
                let mut found = None;
                for host_id in cpal::ALL_HOSTS {
                    let Ok(host) = cpal::host_from_id(*host_id) else {
                        continue;
                    };
                    let Ok(devices) = host.output_devices() else {
                        continue;
                    };
                    if let Some(d) = devices
                        .into_iter()
                        .find(|d| dev_name(d).map(|x| &x == n).unwrap_or(false))
                    {
                        found = Some(d);
                        break;
                    }
                }
                found.ok_or_else(|| AurisError::DeviceNotFound(n.clone()))?
            }
            None => {
                // peripherique par defaut du premier host qui en a un
                let mut def = None;
                for host_id in cpal::ALL_HOSTS {
                    let Ok(host) = cpal::host_from_id(*host_id) else {
                        continue;
                    };
                    if let Some(d) = host.default_output_device() {
                        def = Some(d);
                        break;
                    }
                }
                def.ok_or(AurisError::NoOutputDevice)?
            }
        };
        let name = dev_name(&device).unwrap_or_else(|| "default".into());
        Ok(AudioOutput {
            device: Some(device),
            device_name: name,
            stream: None,
            drain_stop: None,
            drain: None,
            paused: Arc::new(AtomicBool::new(false)),
            dop_carrier: None,
            exclusive: false,
            exclusive_stream: None,
            backend: "cpal",
        })
    }

    /// Sortie « navigateur uniquement » (Jouer ici) utilisee quand aucun
    /// peripherique audio local n'est disponible (ex: binaire WSL sans DAC
    /// visible): le PCM post-gain est forwardé au navigateur via WebSocket,
    /// sans passer par cpal.
    pub fn browser_only(name: &str) -> Self {
        AudioOutput {
            device: None,
            device_name: name.to_string(),
            stream: None,
            drain_stop: None,
            drain: None,
            paused: Arc::new(AtomicBool::new(false)),
            dop_carrier: None,
            exclusive: false,
            exclusive_stream: None,
            backend: "navigateur",
        }
    }

    pub fn name(&self) -> &str {
        &self.device_name
    }

    /// True si un vrai peripherique cpal est attache (false = navigateur seul).
    pub fn has_device(&self) -> bool {
        self.device.is_some()
    }

    /// Active/désactive le mode WASAPI exclusif (Windows, bit-perfect).
    /// Pris en compte au prochain `build_stream`.
    pub fn set_exclusive(&mut self, on: bool) {
        self.exclusive = on;
    }

    /// Backend utilisé pour le dernier flux construit.
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    fn stop_drain(&mut self) {
        if let Some(stop) = self.drain_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(h) = self.drain.take() {
            let _ = h.join();
        }
    }

    /// Construit (et remplace) le flux de sortie pour un spec donne.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
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
        // égaliseur paramétrique (config poussée à chaud, reconstruit si version change)
        eq: Arc<std::sync::RwLock<EqConfig>>,
    ) -> Result<()> {
        self.stream = None; // stoppe l'ancien flux
                            // Libère aussi l'ancien flux WASAPI exclusif : tant qu'il tourne, il
                            // tient l'endpoint en EXCLUSIF et le nouveau Initialize échouerait
                            // (AUDCLNT_E_DEVICE_IN_USE). Drop -> Stop + join du worker.
        self.exclusive_stream = None;
        self.stop_drain();
        if self.device.is_none() {
            // Pas de peripherique cpal: flux « Jouer ici » vers le navigateur.
            return self.build_browser_stream(
                spec, cons, volume, muted, frames_out, gen, max_volume, curve, vol_mode, out_mode,
                browser_tx, eq,
            );
        }
        let device = self.device.as_ref().unwrap();

        // Mode WASAPI EXCLUSIF (Windows) : bit-perfect — le DAC reçoit le flux
        // natif (PCM ou DoP à la porteuse) sans moteur audio intermédiaire.
        // Si le device refuse tout format en exclusif, on rend le Cons et on
        // retombe sur le chemin cpal partagé ci-dessous.
        #[cfg(target_os = "windows")]
        if self.exclusive {
            let params = RenderParams {
                volume: volume.clone(),
                muted: muted.clone(),
                frames_out: frames_out.clone(),
                gen: gen.clone(),
                max_volume: max_volume.clone(),
                curve: curve.clone(),
                vol_mode: vol_mode.clone(),
                out_mode: out_mode.clone(),
                browser_tx: browser_tx.clone(),
                eq: eq.clone(),
            };
            match crate::wasapi_exclusive::open(Some(&self.device_name), spec, cons, params) {
                Ok(stream) => {
                    tracing::info!("sortie WASAPI exclusive: {}", stream.label());
                    self.backend = "exclusif";
                    self.exclusive_stream = Some(stream);
                    return Ok(());
                }
                Err((e, cons2)) => {
                    cons = cons2;
                    tracing::warn!("mode exclusif indisponible ({e}) — repli sur cpal partagé");
                }
            }
        }
        self.dop_carrier = spec.is_dsd_dop.then_some(spec.rate);
        // DoP sous Windows: le moteur WASAPI (mode partagé de cpal) ne laisse
        // passer le flux au DAC que s'il est au « mix format » du device — on
        // écrit le Default Format en PCM 24 bits à la porteuse DoP (sinon les
        // marqueurs DoP sont rééchantillonnés et détruits avant le DAC).
        #[cfg(target_os = "windows")]
        if spec.is_dsd_dop {
            match crate::wasapi_dop::ensure_dop_carrier(Some(&self.device_name), spec.rate) {
                Ok(true) => tracing::warn!(
                    "DoP: Default Format du device '{}' ecrit en 24 bits / {} Hz — rebranchez le DAC USB ou redemarrez le service audio Windows pour que le moteur adopte ce format, sinon le flux DoP est reechantillonne",
                    self.device_name,
                    spec.rate
                ),
                Ok(false) => {
                    if let Some(mix) = crate::wasapi_dop::device_mix_format(Some(&self.device_name)) {
                        if let Some(hint) =
                            crate::wasapi_dop::dop_mismatch_hint(Some(mix), spec.rate)
                        {
                            tracing::warn!("{hint}");
                        } else {
                            tracing::info!(
                                "DoP: mix format du device '{}' = {mix} (porteuse {})",
                                self.device_name,
                                spec.rate
                            );
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    "DoP: impossible de regler le Default Format du device '{}': {e}",
                    self.device_name
                ),
            }
        }
        let configs: Vec<SupportedStreamConfigRange> = device
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
        // cadence copiée pour le callback 'static (l'EQ dépend de la cadence)
        let eq_rate = spec.rate;
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
                let eq = eq.clone();
                let mut cons = cons;
                // stash persistant entre les appels du callback: un chunk plus gros
                // que le buffer cpal est consomme sur plusieurs appels.
                let mut stash: Option<Stash> = None;
                // EQ paramétrique : la chaîne est reconstruite à chaud si la config change.
                let mut eq_proc = EqProcessor::new();
                let stream = device
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
                            let _ = render_samples::<$t>(
                                data,
                                &mut stash,
                                &mut cons,
                                &gen,
                                src_channels,
                                out_channels,
                                &eq,
                                &mut eq_proc,
                                eq_rate,
                                gain,
                                forward_browser,
                                &out_mode,
                                &browser_tx,
                                &frames_out,
                            );
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
                self.backend = "cpal";
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

    /// Flux « navigateur uniquement »: un thread drain l'anneau a cadence
    /// temps reel, applique EQ + gain, et forward le PCM f32 au navigateur.
    /// Le thread est rythme sur l'horloge reelle pour ne pas inonder le
    /// buffer du navigateur.
    #[allow(clippy::too_many_arguments)]
    fn build_browser_stream(
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
        _out_mode: Arc<AtomicU8>,
        browser_tx: broadcast::Sender<BrowserFrame>,
        eq: Arc<std::sync::RwLock<EqConfig>>,
    ) -> Result<()> {
        let src_channels = spec.channels as usize;
        let rate = spec.rate.max(1);
        // Le flux navigateur ne transporte que du PCM f32 (pas de trames DoP).
        let forward_pcm = !spec.is_dsd_dop;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let paused = self.paused.clone();
        let handle = std::thread::Builder::new()
            .name("auris-browser".into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let mut eq_proc = EqProcessor::new();
                let mut sent: u64 = 0;
                loop {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if paused.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    match cons.try_pop() {
                        Some(chunk) if chunk.gen == gen.load(Ordering::Relaxed) => {
                            let frames = match &chunk.data {
                                ChunkData::F32(v) => (v.len() / src_channels.max(1)) as u64,
                                ChunkData::I24Dop(v) => (v.len() / src_channels.max(1)) as u64,
                            };
                            frames_out.fetch_add(frames, Ordering::Relaxed);
                            if forward_pcm {
                                if let ChunkData::F32(v) = chunk.data {
                                    let eq_cfg = eq.read().unwrap();
                                    let gain = compute_gain(
                                        volume.load(Ordering::Relaxed),
                                        max_volume.load(Ordering::Relaxed),
                                        curve.load(Ordering::Relaxed),
                                        vol_mode.load(Ordering::Relaxed),
                                        muted.load(Ordering::Relaxed),
                                    );
                                    let mut pcm = Vec::with_capacity(v.len());
                                    for f in 0..frames as usize {
                                        for c in 0..src_channels {
                                            let s = eq_proc.process(
                                                &eq_cfg,
                                                rate,
                                                src_channels,
                                                c,
                                                v[f * src_channels + c],
                                            );
                                            pcm.push(s * gain);
                                        }
                                    }
                                    // send() non bloquant; Err si aucun client.
                                    let _ = browser_tx.send(BrowserFrame::Pcm(pcm.into()));
                                }
                                // ChunkData::I24Dop: consomme sans forwarder.
                            }
                            sent += frames;
                            let target = start
                                + std::time::Duration::from_secs_f64(sent as f64 / rate as f64);
                            let now = std::time::Instant::now();
                            if target > now {
                                std::thread::sleep(target - now);
                            }
                        }
                        Some(_) => continue, // generation perimee (seek)
                        None => std::thread::sleep(std::time::Duration::from_millis(5)),
                    }
                }
            })
            .map_err(|e| AurisError::other(format!("spawn drain navigateur: {e}")))?;
        self.drain_stop = Some(stop);
        self.drain = Some(handle);
        Ok(())
    }

    pub fn pause(&mut self) {
        self.paused.store(true, Ordering::Relaxed);
        if let Some(s) = &self.stream {
            let _ = s.pause();
        }
        if let Some(s) = &self.exclusive_stream {
            s.pause();
        }
    }
    pub fn resume(&mut self) {
        self.paused.store(false, Ordering::Relaxed);
        if let Some(s) = &self.stream {
            let _ = s.play();
        }
        if let Some(s) = &self.exclusive_stream {
            s.resume();
        }
    }
    pub fn stop(&mut self) {
        self.stream = None;
        self.exclusive_stream = None; // Drop -> Stop + join du worker
        self.stop_drain();
        // A l'arret du flux, re-ecrit le Default Format DoP: c'est au repos
        // que l'ecriture persiste dans le registre (le moteur audio la
        // revoque tant qu'un flux tourne sur l'endpoint).
        if let Some(rate) = self.dop_carrier {
            match crate::wasapi_dop::ensure_dop_carrier(Some(&self.device_name), rate) {
                Ok(true) => tracing::info!(
                    "DoP: Default Format de '{}' re-ecrit a l'arret du flux (24 bits / {rate} Hz) — adopte au prochain rebranchement du DAC ou redemarrage du service audio Windows",
                    self.device_name
                ),
                Ok(false) => {}
                Err(e) => tracing::warn!("DoP: re-ecriture du Default Format echouee: {e}"),
            }
        }
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.stop_drain();
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
    use super::{compute_gain, resolve_device_name, AudioDeviceInfo, WriteOut};
    use cpal::I24;
    use ringbuf::traits::Producer;

    fn dev(name: &str, is_default: bool) -> AudioDeviceInfo {
        AudioDeviceInfo {
            name: name.to_string(),
            is_default,
            sample_rates: vec![44100, 48000],
            supports_i24: false,
            supports_f32: true,
            max_channels: 2,
        }
    }

    #[test]
    fn resolve_device_none_ou_vide_demande_le_defaut() {
        let list = vec![dev("DAC USB", true), dev("Haut-parleurs", false)];
        assert_eq!(resolve_device_name(&list, None).unwrap(), None);
        assert_eq!(resolve_device_name(&list, Some("")).unwrap(), None);
        assert_eq!(resolve_device_name(&list, Some("   ")).unwrap(), None);
    }

    #[test]
    fn resolve_device_nom_connu_dans_la_liste() {
        let list = vec![dev("DAC USB", true), dev("Haut-parleurs", false)];
        assert_eq!(
            resolve_device_name(&list, Some("Haut-parleurs")).unwrap(),
            Some("Haut-parleurs".to_string())
        );
    }

    #[test]
    fn resolve_device_nom_inconnu_refuse_avec_la_liste() {
        let list = vec![dev("DAC USB", true)];
        let err = resolve_device_name(&list, Some("iBasso"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("iBasso"));
        assert!(err.contains("DAC USB")); // la liste des noms disponibles est fournie
    }

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

    #[test]
    fn browser_only_forward_pcm_sans_peripherique() {
        use super::*;
        use crate::decoder::{Chunk, ChunkData, OutputSpec};
        use crate::eq::EqConfig;
        use std::sync::atomic::AtomicU8;

        // Sortie « navigateur uniquement »: aucun device cpal requis.
        let mut out = AudioOutput::browser_only("Navigateur (Jouer ici)");
        assert!(!out.has_device());
        let (mut prod, cons) = make_ring(8);
        let (browser_tx, mut browser_rx) = broadcast::channel::<BrowserFrame>(16);
        let spec = OutputSpec {
            rate: 44100,
            channels: 2,
            bits: 16,
            is_dsd_dop: false,
        };
        let frames_out = Arc::new(AtomicU64::new(0));
        let gen = Arc::new(AtomicU64::new(7));
        let eq = Arc::new(std::sync::RwLock::new(EqConfig {
            enabled: false,
            bands: Vec::new(),
            version: 0,
        }));
        out.build_stream(
            &spec,
            cons,
            Arc::new(AtomicU8::new(100)),
            Arc::new(AtomicBool::new(false)),
            frames_out.clone(),
            gen.clone(),
            Arc::new(AtomicU8::new(100)),
            Arc::new(AtomicU8::new(0)),
            Arc::new(AtomicU8::new(0)),
            Arc::new(AtomicU8::new(1)),
            browser_tx,
            eq,
        )
        .unwrap();
        // 4 frames stereo, gen courante
        prod.try_push(Chunk {
            gen: 7,
            data: ChunkData::F32(vec![0.5, -0.5, 0.25, -0.25, 1.0, -1.0, 0.0, 0.0]),
        })
        .unwrap();
        match browser_rx.blocking_recv() {
            Ok(BrowserFrame::Pcm(samples)) => {
                assert_eq!(samples.len(), 8);
                assert!((samples[0] - 0.5).abs() < 1e-6); // 0.5 * gain 100% = 0.5
            }
            other => panic!("frame inattendu: {other:?}"),
        }
        // la generation perimee est ignoree
        prod.try_push(Chunk {
            gen: 6,
            data: ChunkData::F32(vec![9.0; 4]),
        })
        .unwrap();
        prod.try_push(Chunk {
            gen: 7,
            data: ChunkData::F32(vec![0.1; 4]),
        })
        .unwrap();
        match browser_rx.blocking_recv() {
            Ok(BrowserFrame::Pcm(samples)) => assert_eq!(samples.len(), 4),
            other => panic!("frame inattendu: {other:?}"),
        }
        assert_eq!(frames_out.load(Ordering::Relaxed), 6);
        out.stop();
    }

    #[test]
    fn browser_only_ne_forward_pas_le_dop() {
        use super::*;
        use crate::decoder::{Chunk, ChunkData, OutputSpec};
        use crate::eq::EqConfig;
        use std::sync::atomic::AtomicU8;

        let mut out = AudioOutput::browser_only("Navigateur");
        let (mut prod, cons) = make_ring(8);
        let (browser_tx, mut browser_rx) = broadcast::channel::<BrowserFrame>(16);
        let spec = OutputSpec {
            rate: 44100,
            channels: 2,
            bits: 16,
            is_dsd_dop: true,
        };
        out.build_stream(
            &spec,
            cons,
            Arc::new(AtomicU8::new(100)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(1)),
            Arc::new(AtomicU8::new(100)),
            Arc::new(AtomicU8::new(0)),
            Arc::new(AtomicU8::new(0)),
            Arc::new(AtomicU8::new(1)),
            browser_tx,
            Arc::new(std::sync::RwLock::new(EqConfig {
                enabled: false,
                bands: Vec::new(),
                version: 0,
            })),
        )
        .unwrap();
        prod.try_push(Chunk {
            gen: 1,
            data: ChunkData::I24Dop(vec![0x1234; 8]),
        })
        .unwrap();
        // aucun frame PCM ne doit sortir pour du DoP
        assert!(browser_rx.try_recv().is_err());
        out.stop();
    }

    #[test]
    fn from_dop24_est_bit_parfait_quel_que_soit_le_gain() {
        // spec DoP: rien ne doit alterer les trames en transit. Le mot 24 bits
        // (marqueur + 16 bits DSD) doit sortir identique pour tout gain.
        for v in [0x05_0000i32, 0xFA_FFFF, 0x05_ABCD, 0xFA_0001, 0x05_4081] {
            for gain in [0.0f32, 0.5, 0.64, 0.8, 1.0] {
                // i32 (conteneur 24-in-32 / 32 bits): mot décale de 8 bits
                assert_eq!(i32::from_dop24(v, gain), v << 8, "i32 v={v:#x} gain={gain}");
                // I24: mot 24 bits tel quel dans les bits bas
                assert_eq!(
                    I24::from_dop24(v, gain).inner(),
                    v,
                    "I24 v={v:#x} gain={gain}"
                );
            }
        }
        // sérialisation packed 24 bits (WASAPI exclusif Pcm24Packed):
        // octets sur le fil = [bits_lo, bits_hi, marqueur]
        let mut w = Vec::new();
        i32::from_dop24(0x05_ABCD, 0.64).push_le(&mut w, true);
        assert_eq!(w, vec![0xCD, 0xAB, 0x05]);
        let mut w = Vec::new();
        i32::from_dop24(0xFA_FFFF, 1.0).push_le(&mut w, true);
        assert_eq!(w, vec![0xFF, 0xFF, 0xFA]);
    }

    #[test]
    fn push_le_i16_petit_boutiste() {
        let mut v = Vec::new();
        0x1234i16.push_le(&mut v, false);
        assert_eq!(v, vec![0x34, 0x12]);
    }

    #[test]
    fn push_le_f32_petit_boutiste() {
        let mut v = Vec::new();
        1.0f32.push_le(&mut v, false);
        assert_eq!(v, vec![0x00, 0x00, 0x80, 0x3f]);
    }

    #[test]
    fn push_le_i32_24in32_packed_ne_garde_que_les_3_octets_hauts() {
        // mot DoP 24 bits 0xAB12CD stocké 24-in-32 -> i32 0xAB12CD00,
        // LE = [00, CD, 12, AB] ; packed 24 bits = les 3 octets hauts = CD 12 AB
        let mut v = Vec::new();
        (0xAB12_CD00u32 as i32).push_le(&mut v, true);
        assert_eq!(v, vec![0xCD, 0x12, 0xAB]);
        // conteneur 32 bits : les 4 octets
        let mut v = Vec::new();
        (0xAB12_CD00u32 as i32).push_le(&mut v, false);
        assert_eq!(v, vec![0x00, 0xCD, 0x12, 0xAB]);
    }

    #[test]
    fn push_le_aller_retour_f32_vers_entier_sans_perte_pcm16_24() {
        // bit-perfect : un échantillon entier 16/24 bits est exactement
        // représentable en f32, donc f32 -> i16/i32 est sans perte.
        for x in [0i32, 1, -1, 32767, -32768, 12345, -23456] {
            let f = (x as f32 / i16::MAX as f32) * i16::MAX as f32;
            let back = f as i16;
            // la conversion f32 -> i16 tronque ; on verifie la plage de confiance
            assert!((back as i32 - x).abs() <= 1);
        }
        for x in [0i32, 8_388_607, -8_388_608, 1_234_567] {
            let f = (x as f32 / 8_388_608.0) * 8_388_608.0;
            let back = (f as i32) << 8; // 24-in-32
            assert!((back - (x << 8)).abs() <= 256);
        }
    }
}
