//! Repli ffmpeg : decode via un processus `ffmpeg` externe les formats que
//! symphonia 0.6 ne connait pas (Opus, WMA, APE, TTA, Musepack...).
//!
//! Le contenu du fichier est pipe depuis le `SeekRead` (src:// local/SMB/HTTP/UPnP),
//! donc aucune hypothese sur le protocole de la source. `ffprobe` fournit duree,
//! spec et tags ; `ffmpeg -f f32le` fournit le PCM. Un seek relance le processus
//! avec `-ss <secs>` (le moteur remet a zero sa position de lecture).
//!
//! Detection des binaires : env `AURIS_FFMPEG` / `AURIS_FFPROBE`, sinon PATH.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use auris_core::{AudioFormat, AurisError, Result};
use auris_sources::SeekRead;

use crate::decoder::{ChunkData, Decoder, OutputSpec, TrackMeta};

/// Lecteur partage entre la tentative symphonia et le repli ffmpeg :
/// le meme flux seekable sert aux deux (on revient au debut avant le repli).
#[derive(Clone)]
pub struct SharedReader {
    inner: Arc<Mutex<Box<dyn SeekRead>>>,
}

impl SharedReader {
    pub fn new(r: Box<dyn SeekRead>) -> Self {
        SharedReader {
            inner: Arc::new(Mutex::new(r)),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn SeekRead>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn rewind(&self) -> Result<()> {
        self.lock()
            .seek(SeekFrom::Start(0))
            .map_err(|e| AurisError::Decode(format!("rewind: {e}")))?;
        Ok(())
    }
}

impl Read for SharedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.lock().read(buf)
    }
}
impl Seek for SharedReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.lock().seek(pos)
    }
}

/// Chemin de `ffmpeg` : env AURIS_FFMPEG, sinon recherche dans le PATH.
pub fn ffmpeg_path() -> Option<PathBuf> {
    ffmpeg_path_opt()
}

fn ffmpeg_path_opt() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AURIS_FFMPEG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    find_in_path("ffmpeg")
}

/// Vrai si un binaire ffmpeg utilisable est disponible.
pub fn ffmpeg_disponible() -> bool {
    ffmpeg_path().map(|p| p.is_file()).unwrap_or(false)
}

/// Chemin de `ffprobe` : env AURIS_FFPROBE, sinon a cote de ffmpeg, sinon PATH.
pub fn ffprobe_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AURIS_FFPROBE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    if let Some(f) = ffmpeg_path() {
        let name = if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        };
        let sibling = f.with_file_name(name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    find_in_path("ffprobe")
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

/// Infos extraites de `ffprobe -print_format json -show_format -show_streams`.
struct ProbeInfo {
    duration: Option<f64>,
    rate: u32,
    channels: u8,
    bits: u8,
    tags: HashMap<String, String>,
}

fn bits_from_stream(st: &serde_json::Value) -> u8 {
    if let Some(b) = st.get("bits_per_raw_sample").and_then(|v| v.as_u64()) {
        if b > 0 && b <= 64 {
            return b as u8;
        }
    }
    match st.get("sample_fmt").and_then(|v| v.as_str()).unwrap_or("") {
        "u8" | "u8p" => 8,
        "s16" | "s16p" => 16,
        "s64" | "s64p" => 64,
        _ => 32, // s32/flt et codecs compresses (opus -> f32)
    }
}

/// Sonde un flux via ffprobe (lit depuis pipe:0, le lecteur revient au debut ensuite).
fn run_ffprobe(src: SharedReader) -> Result<ProbeInfo> {
    let ffprobe = ffprobe_path().ok_or_else(|| {
        AurisError::UnsupportedFormat(
            "ffprobe introuvable (definir AURIS_FFPROBE ou installer ffmpeg)".into(),
        )
    })?;
    src.rewind()?;
    let mut child = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "pipe:0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AurisError::Decode(format!("spawn ffprobe: {e}")))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut src2 = src.clone();
    let feeder = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match src2.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let out = child
        .wait_with_output()
        .map_err(|e| AurisError::Decode(format!("ffprobe: {e}")))?;
    let _ = feeder.join();
    src.rewind()?;
    if !out.status.success() {
        return Err(AurisError::UnsupportedFormat(
            "ffprobe a echoue sur ce flux".into(),
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| AurisError::Decode(format!("ffprobe json: {e}")))?;
    let format = &v["format"];
    let mut duration = format
        .get("duration")
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok());
    let stream0 = v["streams"].as_array().and_then(|s| {
        s.iter()
            .find(|st| st.get("codec_type").and_then(|c| c.as_str()) == Some("audio"))
    });
    if duration.is_none() {
        // certains conteneurs (OGG/Opus via pipe) n'ont pas de duree globale
        if let Some(st) = stream0 {
            duration = st
                .get("duration")
                .and_then(|d| d.as_str())
                .and_then(|d| d.parse::<f64>().ok());
        }
    }
    let tags = format
        .get("tags")
        .and_then(|t| t.as_object())
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.to_lowercase(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    let stream = stream0
        .ok_or_else(|| AurisError::UnsupportedFormat("ffprobe: pas de flux audio".into()))?;
    let rate = stream
        .get("sample_rate")
        .and_then(|r| r.as_str())
        .and_then(|r| r.parse::<u32>().ok())
        .unwrap_or(44100);
    let channels = stream
        .get("channels")
        .and_then(|c| c.as_u64())
        .unwrap_or(2)
        .min(255) as u8;
    Ok(ProbeInfo {
        duration,
        rate,
        channels,
        bits: bits_from_stream(stream),
        tags,
    })
}

fn tag(tags: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| tags.get(*k).filter(|v| !v.is_empty()).cloned())
}

fn meta_from_probe(info: &ProbeInfo) -> TrackMeta {
    let t = &info.tags;
    let year =
        tag(t, &["date", "year"]).and_then(|d| d.get(..4).and_then(|y| y.parse::<u32>().ok()));
    let track_number = tag(t, &["track", "tracknumber"]).and_then(|v| {
        v.split('/')
            .next()
            .and_then(|n| n.trim().parse::<u32>().ok())
    });
    TrackMeta {
        title: tag(t, &["title"]),
        artist: tag(t, &["artist"]),
        album: tag(t, &["album"]),
        album_artist: tag(t, &["album_artist", "albumartist"]),
        track_number,
        disc_number: tag(t, &["disc", "discnumber"]).and_then(|v| {
            v.split('/')
                .next()
                .and_then(|n| n.trim().parse::<u32>().ok())
        }),
        year,
        genre: tag(t, &["genre"]),
        duration_secs: info.duration,
        format: Some(AudioFormat::Pcm {
            sample_rate: info.rate,
            bits: info.bits,
            channels: info.channels,
        }),
        has_cover: false,
        cover: None,
        lyrics: tag(t, &["lyrics", "lyrics-xxx", "unsyncedlyrics"]),
        comment: tag(t, &["comment"]),
    }
}

/// Decode tout le flux en f32le et renvoie le nombre de trames (pour la duree
/// quand ffprobe ne la connait pas, ex. OGG via pipe).
fn count_decoded_frames(src: SharedReader, channels: u8) -> Result<u64> {
    let ffmpeg = ffmpeg_path().ok_or_else(|| {
        AurisError::UnsupportedFormat("ffmpeg introuvable (AURIS_FFMPEG ou PATH)".into())
    })?;
    src.rewind()?;
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-vn",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AurisError::Decode(format!("spawn ffmpeg: {e}")))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut src2 = src.clone();
    let feeder = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match src2.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut stdout = child.stdout.take().unwrap();
    let mut total = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total += n as u64,
            Err(_) => break,
        }
    }
    let _ = child.wait();
    let _ = feeder.join();
    src.rewind()?;
    Ok(total / 4 / channels.max(1) as u64)
}

/// Sonde (metadonnees + spec + duree) un flux via ffprobe, avec mesure de
/// duree par decodage rapide si le conteneur ne la porte pas.
pub fn ffmpeg_probe(src: SharedReader) -> Result<(TrackMeta, OutputSpec, Option<f64>)> {
    let info = run_ffprobe(src.clone())?;
    let spec = OutputSpec {
        rate: info.rate,
        channels: info.channels,
        is_dsd_dop: false,
    };
    let duration = match info.duration {
        Some(d) => Some(d),
        None => {
            let frames = count_decoded_frames(src, info.channels)?;
            Some(frames as f64 / info.rate.max(1) as f64)
        }
    };
    Ok((meta_from_probe(&info), spec, duration))
}

/// Decodeur PCM base sur un processus ffmpeg (`-f f32le` en sortie).
pub struct FfmpegDecoder {
    src: SharedReader,
    spec: OutputSpec,
    duration: Option<f64>,
    meta: TrackMeta,
    child: Option<Child>,
    feeder: Option<JoinHandle<()>>,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    frames: u64,
}

impl FfmpegDecoder {
    pub fn new(src: SharedReader) -> Result<Self> {
        if !ffmpeg_disponible() {
            return Err(AurisError::UnsupportedFormat(
                "ffmpeg introuvable (definir AURIS_FFMPEG ou installer ffmpeg)".into(),
            ));
        }
        let info = run_ffprobe(src.clone())?;
        let spec = OutputSpec {
            rate: info.rate,
            channels: info.channels,
            is_dsd_dop: false,
        };
        // Duree manquante (ex. OGG via pipe) : mesure par decodage rapide.
        let duration = match info.duration {
            Some(d) => Some(d),
            None => count_decoded_frames(src.clone(), info.channels)
                .ok()
                .map(|f| f as f64 / info.rate.max(1) as f64),
        };
        let mut d = FfmpegDecoder {
            src,
            spec,
            duration,
            meta: meta_from_probe(&info),
            child: None,
            feeder: None,
            buf: Vec::new(),
            pos: 0,
            eof: false,
            frames: 0,
        };
        d.spawn(0.0)?;
        Ok(d)
    }

    /// (Re)lance ffmpeg a partir de `ss` secondes.
    fn spawn(&mut self, ss: f64) -> Result<()> {
        self.kill_child();
        self.src.rewind()?;
        self.buf.clear();
        self.pos = 0;
        self.eof = false;
        let ffmpeg = ffmpeg_path().unwrap();
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error"]);
        cmd.arg("-i").arg("pipe:0");
        if ss > 0.0 {
            // seek en sortie (apres -i) : un pipe n'est pas seekable, un -ss
            // en entree ne produirait aucun octet.
            cmd.arg("-ss").arg(format!("{ss:.3}"));
        }
        cmd.args([
            "-vn",
            "-sn",
            "-dn",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "pipe:1",
        ]);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AurisError::Decode(format!("spawn ffmpeg: {e}")))?;
        let mut stdin = child.stdin.take().unwrap();
        let mut src = self.src.clone();
        let feeder = std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdin.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        self.child = Some(child);
        self.feeder = Some(feeder);
        Ok(())
    }

    fn kill_child(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(t) = self.feeder.take() {
            let _ = t.join();
        }
    }

    fn fill(&mut self) -> Result<()> {
        if self.eof {
            return Ok(());
        }
        let mut tmp = [0u8; 65536];
        let n = self
            .child
            .as_mut()
            .and_then(|c| c.stdout.as_mut())
            .ok_or_else(|| AurisError::Decode("ffmpeg sans stdout".into()))?
            .read(&mut tmp)
            .map_err(|e| AurisError::Decode(format!("read ffmpeg: {e}")))?;
        if n == 0 {
            self.eof = true;
        } else {
            self.buf.extend_from_slice(&tmp[..n]);
        }
        Ok(())
    }
}

impl Decoder for FfmpegDecoder {
    fn spec(&self) -> OutputSpec {
        self.spec
    }
    fn duration_secs(&self) -> Option<f64> {
        self.duration.or_else(|| {
            if self.eof && self.frames > 0 {
                Some(self.frames as f64 / self.spec.rate.max(1) as f64)
            } else {
                None
            }
        })
    }
    fn meta(&self) -> &TrackMeta {
        &self.meta
    }
    fn next_chunk(&mut self, max_frames: usize) -> Result<Option<ChunkData>> {
        let channels = self.spec.channels.max(1) as usize;
        let need = max_frames.saturating_mul(channels).saturating_mul(4);
        while self.buf.len() - self.pos < need && !self.eof {
            self.fill()?;
        }
        let avail = self.buf.len() - self.pos;
        let take = avail.min(need) & !3;
        if take == 0 {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(take / 4);
        for c in self.buf[self.pos..self.pos + take].as_chunks::<4>().0 {
            out.push(f32::from_le_bytes(*c));
        }
        self.pos += take;
        if self.pos >= self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        self.frames += (take / 4 / channels) as u64;
        Ok(Some(ChunkData::F32(out)))
    }
    fn seek(&mut self, secs: f64) -> Result<()> {
        self.spawn(secs)
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        self.kill_child();
    }
}
