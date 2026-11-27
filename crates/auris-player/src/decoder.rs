//! Decodeurs: PCM via symphonia (FLAC, WAV, AIFF, ALAC, MP3, AAC, MP4, OGG...)
//! et DSD via le module dsd (DSF/DFF -> DoP). Le moteur consomme des `Chunk`.

use std::io::{Read, Seek, SeekFrom};

use auris_core::{AudioFormat, AurisError, Result};
use auris_sources::SeekRead;
use symphonia::core::codecs::audio::{AudioDecoder as SymDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::{Hint, Probe};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, StandardTag};
use symphonia::core::units::Time;

use crate::dsd::DsdDecoder;

/// Donnees d'un bloc audio decode.
#[derive(Debug)]
pub enum ChunkData {
    /// PCM interleaved f32 (len = frames * channels).
    F32(Vec<f32>),
    /// DoP 24 bits interleaved (i32, 24 bits bas significatifs; len = frames * channels).
    I24Dop(Vec<i32>),
}

impl ChunkData {
    pub fn frames(&self, channels: usize) -> usize {
        let n = match self {
            ChunkData::F32(v) => v.len(),
            ChunkData::I24Dop(v) => v.len(),
        };
        n / channels.max(1)
    }
}

/// Un bloc pousse dans l'anneau de sortie (avec generation pour les seeks).
#[derive(Debug)]
pub struct Chunk {
    pub gen: u64,
    pub data: ChunkData,
}

/// Spec de sortie requis pour un flux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSpec {
    pub rate: u32,
    pub channels: u8,
    /// Bits par échantillon de la source PCM (16/24/32) ; 24 pour du DoP.
    /// Sert à choisir le conteneur bit-perfect en sortie WASAPI exclusive.
    pub bits: u8,
    /// true = trames DoP 24 bits (DSD), false = PCM f32.
    pub is_dsd_dop: bool,
}

/// Metadonnees extraites d'un fichier.
#[derive(Debug, Clone, Default)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration_secs: Option<f64>,
    pub format: Option<AudioFormat>,
    pub has_cover: bool,
    pub cover: Option<Vec<u8>>,
    /// Paroles embarquees (tag USLT/LYRICS, souvent au format LRC).
    pub lyrics: Option<String>,
    /// Commentaire embarque (tag COMM/COMMENT).
    pub comment: Option<String>,
    /// Credits étendus (#73) : compositeur, producteur, label, éditeur, ISRC, copyright.
    pub composer: Option<String>,
    pub producer: Option<String>,
    pub label: Option<String>,
    pub publisher: Option<String>,
    pub isrc: Option<String>,
    pub copyright: Option<String>,
    /// Identifiants MusicBrainz (#26) : recording (scrobble Last.fm + fiche MB),
    /// artist, album, release group.
    pub mb_track_id: Option<String>,
    pub mb_artist_id: Option<String>,
    pub mb_album_id: Option<String>,
    pub mb_release_group_id: Option<String>,
}

/// Decodeur de flux.
pub trait Decoder: Send {
    fn spec(&self) -> OutputSpec;
    fn duration_secs(&self) -> Option<f64>;
    fn meta(&self) -> &TrackMeta;
    /// Prochain bloc; None = fin de piste.
    fn next_chunk(&mut self, max_frames: usize) -> Result<Option<ChunkData>>;
    fn seek(&mut self, secs: f64) -> Result<()>;
}

/// Adaptateur SeekRead -> symphonia MediaSource.
struct MsWrapper<R: SeekRead> {
    inner: R,
    byte_len: Option<u64>,
}

impl<R: SeekRead> MsWrapper<R> {
    fn new(mut inner: R) -> Self {
        // Taille du flux via seek end (certains demuxers, ex. CAF, en ont besoin).
        let byte_len = inner
            .seek(SeekFrom::End(0))
            .ok()
            .filter(|&end| inner.seek(SeekFrom::Start(0)).ok() == Some(0) && end > 0);
        MsWrapper { inner, byte_len }
    }
}

impl<R: SeekRead> Read for MsWrapper<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}
impl<R: SeekRead> Seek for MsWrapper<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}
impl<R: SeekRead> MediaSource for MsWrapper<R> {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }
}

/// Decodeur PCM symphonia.
pub struct PcmDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymDecoder>,
    track_id: u32,
    spec: OutputSpec,
    duration: Option<f64>,
    meta: TrackMeta,
}

impl PcmDecoder {
    pub fn new(reader: Box<dyn SeekRead>, ext: &str) -> Result<Self> {
        let mss = MediaSourceStream::new(Box::new(MsWrapper::new(reader)), Default::default());
        let mut hint = Hint::new();
        if !ext.is_empty() {
            hint.with_extension(ext);
        }
        let fmt_opts: FormatOptions = Default::default();
        let meta_opts: MetadataOptions = Default::default();
        // Registres maison : formats/codecs symphonia par defaut + WavPack (.wv),
        // absent des features "all-*" de symphonia 0.6 (crate externe compatible 0.6).
        let mut probe = Probe::default();
        symphonia::default::register_enabled_formats(&mut probe);
        probe.register_format::<symphonia_codec_wavpack::WavPackReader>();
        let mut codecs = CodecRegistry::default();
        symphonia::default::register_enabled_codecs(&mut codecs);
        codecs.register_audio_decoder::<symphonia_codec_wavpack::WavPackDecoder>();
        let mut format = probe
            .probe(&hint, mss, fmt_opts, meta_opts)
            .map_err(|e| AurisError::UnsupportedFormat(format!("{e}")))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| AurisError::UnsupportedFormat("pas de piste audio".into()))?;
        let dec_opts: AudioDecoderOptions = Default::default();
        let codec_params = track
            .codec_params
            .as_ref()
            .and_then(|c| c.audio())
            .ok_or_else(|| AurisError::UnsupportedFormat("codec sans parametres audio".into()))?;
        let decoder = codecs
            .make_audio_decoder(codec_params, &dec_opts)
            .map_err(|e| AurisError::UnsupportedFormat(format!("codec non supporte: {e}")))?;
        let track_id = track.id;
        let rate = codec_params.sample_rate.unwrap_or(44100);
        let channels = codec_params
            .channels
            .as_ref()
            .map(|c| c.count() as u8)
            .unwrap_or(2);
        let duration = track
            .duration
            .and_then(|d| track.time_base.and_then(|tb| tb.calc_duration(d)))
            .map(|t| t.as_nanos() as f64 / 1e9);
        let bits = codec_params.bits_per_sample.unwrap_or(16) as u8;
        let format_info = AudioFormat::Pcm {
            sample_rate: rate,
            bits,
            channels,
        };
        let mut meta = read_metadata(format.as_mut());
        meta.duration_secs = meta.duration_secs.or(duration);
        meta.format = Some(format_info);
        Ok(PcmDecoder {
            format,
            decoder,
            track_id,
            spec: OutputSpec {
                rate,
                channels,
                bits,
                is_dsd_dop: false,
            },
            duration,
            meta,
        })
    }
}

impl Decoder for PcmDecoder {
    fn spec(&self) -> OutputSpec {
        self.spec
    }
    fn duration_secs(&self) -> Option<f64> {
        self.duration
    }
    fn meta(&self) -> &TrackMeta {
        &self.meta
    }

    fn next_chunk(&mut self, max_frames: usize) -> Result<Option<ChunkData>> {
        let channels = self.spec.channels as usize;
        loop {
            match self.format.next_packet() {
                Ok(Some(packet)) => {
                    if packet.track_id != self.track_id {
                        continue;
                    }
                    match self.decoder.decode(&packet) {
                        Ok(buf) => {
                            let n = buf.samples_interleaved();
                            if n == 0 {
                                continue;
                            }
                            let mut v = vec![0f32; n];
                            buf.copy_to_slice_interleaved(&mut v);
                            // Limite la taille du chunk
                            let max_samples = max_frames.max(1) * channels;
                            if v.len() > max_samples {
                                v.truncate(max_samples);
                            }
                            // met a jour la spec si le decodeur change de rate/canaux
                            let spec = buf.spec();
                            if spec.rate() != self.spec.rate
                                || spec.channels().count() as u8 != self.spec.channels
                            {
                                self.spec.rate = spec.rate();
                                self.spec.channels = spec.channels().count() as u8;
                            }
                            return Ok(Some(ChunkData::F32(v)));
                        }
                        Err(SymError::DecodeError(m)) => {
                            tracing::debug!("pcm: decode error: {m}");
                            continue;
                        }
                        Err(SymError::ResetRequired) => {
                            self.decoder.reset();
                            continue;
                        }
                        Err(e) => return Err(AurisError::Decode(e.to_string())),
                    }
                }
                Ok(None) => {
                    tracing::debug!("pcm: next_packet -> None (fin de conteneur)");
                    return Ok(None);
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    tracing::debug!("pcm: next_packet erreur: {e}");
                    return Ok(None);
                }
            }
        }
    }

    fn seek(&mut self, secs: f64) -> Result<()> {
        let time = Time::try_from_secs_f64(secs)
            .ok_or_else(|| AurisError::Decode("seek hors limites".into()))?;
        self.format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| AurisError::Decode(format!("seek: {e}")))?;
        self.decoder.reset();
        Ok(())
    }
}

/// Extrait les metadonnees courantes d'un format reader.
pub fn read_metadata(format: &mut dyn FormatReader) -> TrackMeta {
    let mut meta = TrackMeta::default();
    let mut md = format.metadata();
    while !md.is_latest() {
        md.pop();
    }
    if let Some(rev) = md.current() {
        for tag in &rev.media.tags {
            let value = tag.raw.value.to_string();
            match &tag.std {
                Some(StandardTag::TrackTitle(v)) => meta.title = Some(v.to_string()),
                Some(StandardTag::Artist(v)) => meta.artist = Some(v.to_string()),
                Some(StandardTag::Album(v)) => meta.album = Some(v.to_string()),
                Some(StandardTag::AlbumArtist(v)) => meta.album_artist = Some(v.to_string()),
                Some(StandardTag::TrackNumber(n)) => meta.track_number = Some(*n as u32),
                Some(StandardTag::DiscNumber(n)) => meta.disc_number = Some(*n as u32),
                Some(StandardTag::ReleaseYear(y)) => meta.year = Some(*y as u32),
                Some(StandardTag::Genre(v)) => meta.genre = Some(v.to_string()),
                Some(StandardTag::Lyrics(v)) => meta.lyrics = Some(v.to_string()),
                Some(StandardTag::Comment(v)) => meta.comment = Some(v.to_string()),
                _ => {
                    // repli sur les cles brutes
                    let key = tag.raw.key.to_ascii_uppercase();
                    apply_raw_tag(&mut meta, &key, &value);
                }
            }
        }
        if let Some(visual) = rev.media.visuals.first() {
            meta.has_cover = true;
            meta.cover = Some(visual.data.to_vec());
        }
    }
    meta
}

/// Applique une paire cle brute/valeur sur les metadonnees (tags ID3, Vorbis,
/// APE... normalisés en MAJUSCULES). Extrait de `read_metadata` pour être testé.
pub fn apply_raw_tag(meta: &mut TrackMeta, key: &str, value: &str) {
    match key {
        "TITLE" | "TIT2" => {
            meta.title.get_or_insert(value.to_string());
        }
        "ARTIST" | " TPE1" => {
            meta.artist.get_or_insert(value.to_string());
        }
        "ALBUM" | "TALB" => {
            meta.album.get_or_insert(value.to_string());
        }
        "ALBUMARTIST" | "TPE2" | "ALBUM ARTIST" => {
            meta.album_artist.get_or_insert(value.to_string());
        }
        "TRACKNUMBER" | "TRCK" | "TRACK" => {
            meta.track_number = value.split('/').next().and_then(|s| s.trim().parse().ok());
        }
        "DISCNUMBER" | "TPOS" | "DISC" => {
            meta.disc_number = value.split('/').next().and_then(|s| s.trim().parse().ok());
        }
        "DATE" | "YEAR" | "TDRC" => {
            meta.year = value.chars().take(4).collect::<String>().parse().ok();
        }
        "GENRE" | "TCON" => {
            meta.genre.get_or_insert(value.to_string());
        }
        "USLT" | "LYRICS" | "UNSYNCEDLYRICS" | "SYLT" => {
            meta.lyrics.get_or_insert(value.to_string());
        }
        "COMM" | "COMMENT" => {
            meta.comment.get_or_insert(value.to_string());
        }
        // credits étendus (#73)
        "COMPOSER" | "TCOM" | "SONGWRITER" | "WRITER" => {
            meta.composer.get_or_insert(value.to_string());
        }
        "PRODUCER" | "TPE3" => {
            meta.producer.get_or_insert(value.to_string());
        }
        "LABEL" | "RECORDLABEL" | "ORGANIZATION" => {
            meta.label.get_or_insert(value.to_string());
        }
        "PUBLISHER" | "TPUB" => {
            meta.publisher.get_or_insert(value.to_string());
        }
        "ISRC" | "TSRC" => {
            meta.isrc.get_or_insert(value.to_string());
        }
        "COPYRIGHT" | "TCOP" => {
            meta.copyright.get_or_insert(value.to_string());
        }
        // MusicBrainz (#26) : vorbis MUSICBRAINZ_* et ID3 TXXX « MusicBrainz … »
        "MUSICBRAINZ_TRACKID" | "MUSICBRAINZ RECORDING ID" => {
            meta.mb_track_id.get_or_insert(value.to_string());
        }
        "MUSICBRAINZ_ARTISTID" | "MUSICBRAINZ ARTIST ID" => {
            meta.mb_artist_id.get_or_insert(value.to_string());
        }
        "MUSICBRAINZ_ALBUMID" | "MUSICBRAINZ ALBUM ID" => {
            meta.mb_album_id.get_or_insert(value.to_string());
        }
        "MUSICBRAINZ_RELEASEGROUPID" | "MUSICBRAINZ RELEASE GROUP ID" => {
            meta.mb_release_group_id.get_or_insert(value.to_string());
        }
        _ => {}
    }
}

/// Ouvre un decodeur adapte au format (DSD si .dsf/.dff, PCM sinon).
/// PCM : symphonia d'abord, repli sur un processus ffmpeg si symphonia
/// ne connait pas le format (Opus, WMA, APE...) et que ffmpeg est disponible.
pub fn open_decoder(
    reader: Box<dyn SeekRead>,
    ext: &str,
    meta: TrackMeta,
) -> Result<Box<dyn Decoder>> {
    open_decoder_with_path(reader, ext, meta, None)
}

/// Variante de [`open_decoder`] avec le chemin local du fichier (#54) :
/// le repli ffmpeg lit alors le fichier directement (`-i <path>`), ce qui rend
/// le seek natif et instantané au lieu d'un re-decodage depuis le debut.
pub fn open_decoder_with_path(
    reader: Box<dyn SeekRead>,
    ext: &str,
    meta: TrackMeta,
    local_path: Option<std::path::PathBuf>,
) -> Result<Box<dyn Decoder>> {
    let ext = ext.to_ascii_lowercase();
    if ext == "dsf" || ext == "dff" {
        return Ok(Box::new(DsdDecoder::new(reader, meta)?));
    }
    let shared = match local_path {
        Some(p) => crate::ffmpeg::SharedReader::with_local_path(reader, p),
        None => crate::ffmpeg::SharedReader::new(reader),
    };
    match PcmDecoder::new(Box::new(shared.clone()), &ext) {
        Ok(d) => Ok(Box::new(d)),
        Err(e) => match crate::ffmpeg::FfmpegDecoder::new(shared) {
            Ok(d) => Ok(Box::new(d)),
            Err(fe) => Err(fallback_error(e, fe)),
        },
    }
}

/// Erreur finale du repli : si ffmpeg est indisponible, on garde l'erreur
/// symphonia en precisant le repli ; sinon c'est ffmpeg qui a echoue.
fn fallback_error(sym_err: AurisError, ff_err: AurisError) -> AurisError {
    if !crate::ffmpeg::ffmpeg_disponible() {
        AurisError::UnsupportedFormat(format!(
            "{sym_err} (repli ffmpeg indisponible: ffmpeg introuvable, definir AURIS_FFMPEG ou l'installer)"
        ))
    } else {
        ff_err
    }
}

/// Sonde un fichier: metadonnees + spec, sans lire tout le flux.
pub fn probe_meta(
    reader: Box<dyn SeekRead>,
    ext: &str,
) -> Result<(TrackMeta, OutputSpec, Option<f64>)> {
    probe_meta_with_path(reader, ext, None)
}

/// Variante de [`probe_meta`] avec le chemin local du fichier (#54) : ffprobe
/// sonde alors le fichier directement au lieu du pipe.
pub fn probe_meta_with_path(
    reader: Box<dyn SeekRead>,
    ext: &str,
    local_path: Option<std::path::PathBuf>,
) -> Result<(TrackMeta, OutputSpec, Option<f64>)> {
    let ext_l = ext.to_ascii_lowercase();
    if ext_l == "dsf" || ext_l == "dff" {
        let d = DsdDecoder::new(reader, TrackMeta::default())?;
        let meta = d.meta().clone();
        let spec = d.spec();
        let dur = d.duration_secs();
        let mut meta = meta;
        meta.format = Some(AudioFormat::Dsd {
            sample_rate: d.info().rate,
            channels: d.info().channels,
        });
        return Ok((meta, spec, dur));
    }
    let shared = match local_path {
        Some(p) => crate::ffmpeg::SharedReader::with_local_path(reader, p),
        None => crate::ffmpeg::SharedReader::new(reader),
    };
    match PcmDecoder::new(Box::new(shared.clone()), &ext_l) {
        Ok(pcm) => {
            let meta = pcm.meta().clone();
            let spec = pcm.spec();
            let dur = pcm.duration_secs();
            Ok((meta, spec, dur))
        }
        Err(e) => crate::ffmpeg::ffmpeg_probe(shared).map_err(|fe| fallback_error(e, fe)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_credits_etendus_id3_et_vorbis() {
        let mut m = TrackMeta::default();
        apply_raw_tag(&mut m, "TCOM", "Max Richter");
        apply_raw_tag(&mut m, "TPE3", "Deutsche Grammophon");
        apply_raw_tag(&mut m, "PUBLISHER", "DG");
        apply_raw_tag(&mut m, "ISRC", "DEUM71800001");
        apply_raw_tag(&mut m, "TCOP", "© 2018");
        apply_raw_tag(&mut m, "COMPOSER", "Max Richter"); // vorbis: premier gagne
        apply_raw_tag(&mut m, "LABEL", "DG");
        // MusicBrainz (#26) : vorbis + TXXX
        apply_raw_tag(&mut m, "MUSICBRAINZ_TRACKID", "rec-mbid-1");
        apply_raw_tag(&mut m, "MusicBrainz Recording Id", "ignore-premier-gagne");
        apply_raw_tag(&mut m, "MUSICBRAINZ_ARTISTID", "art-mbid-1");
        apply_raw_tag(&mut m, "MUSICBRAINZ_ALBUMID", "alb-mbid-1");
        apply_raw_tag(&mut m, "MUSICBRAINZ_RELEASEGROUPID", "rg-mbid-1");
        assert_eq!(m.mb_track_id.as_deref(), Some("rec-mbid-1"));
        assert_eq!(m.mb_artist_id.as_deref(), Some("art-mbid-1"));
        assert_eq!(m.mb_album_id.as_deref(), Some("alb-mbid-1"));
        assert_eq!(m.mb_release_group_id.as_deref(), Some("rg-mbid-1"));
        assert_eq!(m.composer.as_deref(), Some("Max Richter"));
        assert_eq!(m.producer.as_deref(), Some("Deutsche Grammophon"));
        assert_eq!(m.publisher.as_deref(), Some("DG"));
        assert_eq!(m.isrc.as_deref(), Some("DEUM71800001"));
        assert_eq!(m.copyright.as_deref(), Some("© 2018"));
        assert_eq!(m.label.as_deref(), Some("DG"));
    }

    #[test]
    fn tags_credits_ne_ecrasent_pas_la_valeur_deja_lue() {
        let mut m = TrackMeta::default();
        apply_raw_tag(&mut m, "PRODUCER", "Premier");
        apply_raw_tag(&mut m, "TPE3", "Second");
        assert_eq!(m.producer.as_deref(), Some("Premier"));
    }
}
