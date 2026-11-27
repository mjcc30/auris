//! Source « line-in » : entrée audio externe du boîtier (jack line-in, optique
//! SPDIF, HDMI eARC, ou boucle WASAPI « ce que tu entends » sous Windows) lue
//! en passthrough live — #75.
//!
//! La source expose une seule entrée jouable, d'URI `src://<id>/live.f32le` :
//! la capture cpal pousse du PCM brut f32le interleaved dans un anneau
//! ([`crate::live::LiveRing`]), lu par le moteur comme un flux live infini
//! (décodeur `f32le`, durée inconnue → badge « EN DIRECT »).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use auris_core::{AudioFormat, AurisError, BrowseEntry, Result, Source, SourceKind};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::live::{LiveRing, LiveStream};
use crate::{make_uri, SeekRead, SourceBackend};

/// Chemin relatif de l'unique entrée jouable de la source.
pub const LIVE_PATH: &str = "live.f32le";

/// Flux de capture line-in : PCM brut f32le interleaved à la spec du
/// périphérique d'entrée. Le moteur lit la spec via `as_any`.
pub struct LineInStream {
    stream: LiveStream,
    spec: (u32, u8),
}

impl LineInStream {
    /// (sample_rate, canaux) du flux capturé.
    pub fn spec(&self) -> (u32, u8) {
        self.spec
    }
}

impl Read for LineInStream {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(out)
    }
}

impl Seek for LineInStream {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.stream.seek(pos)
    }
}

/// Streams de capture en cours, gardés vivants par source (id -> stream).
static STREAMS: LazyLock<Mutex<HashMap<String, cpal::Stream>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Nom d'un périphérique cpal (0.18 : via `description()`).
fn dev_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

/// Périphériques d'entrée audio disponibles : (nom, par_défaut).
pub fn input_devices() -> Vec<(String, bool)> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| dev_name(&d));
    let Ok(devices) = host.input_devices() else {
        return vec![];
    };
    devices
        .filter_map(|d| dev_name(&d).map(|n| (n.clone(), default.as_deref() == Some(n.as_str()))))
        .collect()
}

/// Choisit le périphérique d'entrée : par nom exact si demandé, sinon celui
/// par défaut de l'hôte.
fn pick_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(n) = name {
        let dev = host
            .input_devices()
            .map_err(|e| AurisError::other(format!("énumération entrées audio: {e}")))?
            .find(|d| dev_name(d).as_deref() == Some(n))
            .ok_or_else(|| {
                AurisError::other(format!(
                    "périphérique d'entrée introuvable: {n} (voir /api/audio/input-devices)"
                ))
            })?;
        return Ok(dev);
    }
    host.default_input_device()
        .ok_or_else(|| AurisError::other("aucun périphérique d'entrée audio détecté".to_string()))
}

/// Spec (rate, canaux) de l'entrée choisie, sans lancer de capture.
fn probe_spec(name: Option<&str>) -> (u32, u8) {
    pick_device(name)
        .and_then(|d| {
            d.default_input_config()
                .map_err(|e| AurisError::other(e.to_string()))
        })
        .map(|c| (c.sample_rate(), c.channels().min(u8::MAX as u16) as u8))
        .unwrap_or((44100, 2))
}

/// Construit le flux de capture générique (quel que soit le format d'échantillon).
fn build_capture<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    ring: LiveRing,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    dev.build_input_stream(
        config,
        move |data: &[T], _| {
            if ring.is_closed() {
                return;
            }
            let mut bytes = Vec::with_capacity(data.len() * 4);
            for &s in data {
                bytes.extend_from_slice(&s.to_sample::<f32>().to_le_bytes());
            }
            ring.push(&bytes);
        },
        |e| {
            tracing::warn!("capture line-in: erreur de flux: {e}");
        },
        None,
    )
    .map_err(|e| AurisError::other(format!("ouverture capture entrée: {e}")))
}

pub struct LineInSource {
    source: Source,
    device: Option<String>,
}

impl LineInSource {
    pub fn new(source: Source) -> Result<Self> {
        let device = match &source.kind {
            SourceKind::LineIn { device } => device.clone(),
            _ => return Err(AurisError::other("source non line-in")),
        };
        Ok(LineInSource { source, device })
    }

    fn entry(&self) -> BrowseEntry {
        let (rate, ch) = probe_spec(self.device.as_deref());
        BrowseEntry {
            name: self.source.name.clone(),
            uri: make_uri(&self.source.id, LIVE_PATH),
            is_dir: false,
            size: None,
            format: Some(AudioFormat::Pcm {
                sample_rate: rate,
                bits: 32,
                channels: ch,
            }),
            is_audio: true,
        }
    }
}

#[async_trait]
impl SourceBackend for LineInSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        if rel_path.is_empty() {
            Ok(vec![self.entry()])
        } else {
            Ok(vec![])
        }
    }

    async fn stat(&self, rel_path: &str) -> Result<Option<BrowseEntry>> {
        Ok((rel_path == LIVE_PATH).then(|| self.entry()))
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        if rel_path != LIVE_PATH {
            return Err(AurisError::InvalidUri(format!(
                "src://{}/{}",
                self.source.id, rel_path
            )));
        }
        let dev = pick_device(self.device.as_deref())?;
        let dev_label = dev_name(&dev).unwrap_or_else(|| "entrée".into());
        let cfg = dev
            .default_input_config()
            .map_err(|e| AurisError::other(format!("config entrée {dev_label}: {e}")))?;
        let spec = (cfg.sample_rate(), cfg.channels().min(u8::MAX as u16) as u8);
        let ring = LiveRing::new();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => build_capture::<f32>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::F64 => build_capture::<f64>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::I8 => build_capture::<i8>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::I16 => build_capture::<i16>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::I24 | cpal::SampleFormat::U24 | cpal::SampleFormat::I32 => {
                build_capture::<i32>(&dev, cfg.config(), ring.clone())?
            }
            cpal::SampleFormat::I64 => build_capture::<i64>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::U8 => build_capture::<u8>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::U16 => build_capture::<u16>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::U32 => build_capture::<u32>(&dev, cfg.config(), ring.clone())?,
            cpal::SampleFormat::U64 => build_capture::<u64>(&dev, cfg.config(), ring.clone())?,
            other => {
                return Err(AurisError::other(format!(
                    "format d'entrée non supporté: {other}"
                )))
            }
        };
        stream
            .play()
            .map_err(|e| AurisError::other(format!("démarrage capture: {e}")))?;
        // garde le flux vivant, remplace un éventuel capture précédent
        STREAMS
            .lock()
            .unwrap()
            .insert(self.source.id.clone(), stream);
        let tag = make_uri(&self.source.id, LIVE_PATH);
        Ok(Box::new(LineInStream {
            stream: ring.stream(&tag),
            spec,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_linein_construit() {
        let src = Source {
            id: "linein".into(),
            name: "Line-in".into(),
            kind: SourceKind::LineIn { device: None },
            enabled: true,
        };
        let b = LineInSource::new(src).unwrap();
        assert_eq!(b.device, None);
        let e = b.entry();
        assert_eq!(e.uri, "src://linein/live.f32le");
        assert!(e.is_audio);
        assert!(!e.is_dir);
        assert!(matches!(e.format, Some(AudioFormat::Pcm { bits: 32, .. })));
    }

    #[test]
    fn source_non_linein_refusee() {
        let src = Source {
            id: "x".into(),
            name: "X".into(),
            kind: SourceKind::Radio {
                url: "http://x".into(),
            },
            enabled: true,
        };
        assert!(LineInSource::new(src).is_err());
    }
}
