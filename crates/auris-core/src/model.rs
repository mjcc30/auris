//! Modeles de donnees: pistes, albums, sources, etat du lecteur.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Extensions de fichiers audio reconnus (PCM + DSD).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "wav", "aif", "aiff", "alac", "m4a", "mp4", "mp3", "aac", "ogg", "opus", "oga",
    "vorbis", "wma", "wv", "caf", "dsf", "dff", "iso",
];

pub fn is_audio_file(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .map(|e| {
            let e = e.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

/// Format audio d'une piste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AudioFormat {
    /// PCM (FLAC, WAV, ALAC, MP3...) : sample_rate en Hz, profondeur en bits.
    Pcm {
        sample_rate: u32,
        bits: u8,
        channels: u8,
    },
    /// DSD (DSF/DFF) : dsd64/128/256 -> sample_rate DSD en Hz (44100*64*n).
    Dsd { sample_rate: u32, channels: u8 },
}

impl AudioFormat {
    pub fn dsd_multiplier(sample_rate: u32) -> Option<u8> {
        let base = 44100u32;
        if !sample_rate.is_multiple_of(base) {
            return None;
        }
        match sample_rate / base {
            64 => Some(1),
            128 => Some(2),
            256 => Some(4),
            512 => Some(8),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            AudioFormat::Pcm {
                sample_rate,
                bits,
                channels,
            } => {
                format!(
                    "{} bit / {} kHz / {} ch",
                    bits,
                    sample_rate / 1000,
                    channels
                )
            }
            AudioFormat::Dsd {
                sample_rate,
                channels,
            } => {
                let mult = Self::dsd_multiplier(*sample_rate).unwrap_or(0);
                format!("DSD{} / {} ch", 64 * mult as u32, channels)
            }
        }
    }
}

/// Une piste de la bibliotheque.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub duration_secs: Option<f64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    /// URI de lecture: `src://<source_id>/<chemin>` ou URL http(s) directe.
    pub uri: String,
    pub source_id: String,
    pub format: Option<AudioFormat>,
    pub size_bytes: Option<u64>,
    /// Cle de l'album (titre + artiste album), pour le regroupement.
    pub album_key: String,
    pub artist_key: String,
    pub has_cover: bool,
    pub added_at: i64,
}

impl Track {
    pub fn new(uri: String, source_id: String) -> Self {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let file_name = uri.rsplit('/').next().unwrap_or("").to_string();
        Track {
            id,
            title: file_name,
            artist: "Artiste inconnu".into(),
            album: "Album inconnu".into(),
            album_artist: "Artiste inconnu".into(),
            duration_secs: None,
            track_number: None,
            disc_number: None,
            year: None,
            genre: None,
            uri,
            source_id,
            format: None,
            size_bytes: None,
            album_key: String::new(),
            artist_key: String::new(),
            has_cover: false,
            added_at: now,
        }
    }

    pub fn finalize_keys(&mut self) {
        self.album_key = format!(
            "{}|{}",
            self.album_artist.to_lowercase(),
            self.album.to_lowercase()
        );
        self.artist_key = self.album_artist.to_lowercase();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub cover_track_id: Option<String>,
    pub track_count: usize,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub album_count: usize,
}

/// Kinds de sources musicales.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceKind {
    /// Repertoire local (dossier monte, disque NTFS...).
    Local { path: String },
    /// Disque amovible monte (cle USB, disque externe): Windows `E:\`,
    /// Linux `/media/<user>/CLE`, macOS `/Volumes/CLE`.
    Usb { path: String },
    /// Partage SMB/CIFS direct vers un NAS (Synology, QNAP...).
    Smb {
        host: String,
        share: String,
        user: String,
        password: String,
        base_path: String,
    },
    /// URL HTTP(S) directe (webradio, serveur DLNA...).
    Http { base_url: Option<String> },
    /// Serveur UPnP/DLNA (decouverte SSDP + navigation ContentDirectory).
    Upnp { url: String },
    /// Webradio: flux HTTP(S) joue directement.
    Radio { url: String },
}

impl SourceKind {
    /// Kind ecritable (import/copie vers cette source possible).
    pub fn is_writable(&self) -> bool {
        matches!(
            self,
            SourceKind::Local { .. } | SourceKind::Usb { .. } | SourceKind::Smb { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: SourceKind,
    pub enabled: bool,
}

impl Source {
    pub fn new_local(name: &str, path: &str) -> Self {
        Source {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SourceKind::Local { path: path.into() },
            enabled: true,
        }
    }

    pub fn new_smb(
        name: &str,
        host: &str,
        share: &str,
        user: &str,
        password: &str,
        base_path: &str,
    ) -> Self {
        Source {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SourceKind::Smb {
                host: host.into(),
                share: share.into(),
                user: user.into(),
                password: password.into(),
                base_path: base_path.trim_matches('/').into(),
            },
            enabled: true,
        }
    }

    pub fn new_usb(name: &str, path: &str) -> Self {
        Source {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SourceKind::Usb { path: path.into() },
            enabled: true,
        }
    }

    pub fn new_radio(name: &str, url: &str) -> Self {
        Source {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SourceKind::Radio { url: url.into() },
            enabled: true,
        }
    }
}

/// Une liste de lecture (file d'attente enregistree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub track_ids: Vec<String>,
    pub created_at: i64,
}

/// Sorte de ressource partagee par un lien public.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    Track,
    Album,
    Playlist,
}

/// Lien de partage public (piste, album ou playlist) : `/share/<token>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLink {
    /// Jeton aleatoire (32 caracteres hex).
    pub token: String,
    pub kind: ShareKind,
    /// ID de la ressource (piste / album / playlist).
    pub id: String,
    /// Titre fige au moment du partage (affiche meme si la ressource disparait).
    pub title: String,
    pub created_at: i64,
    /// Expiration (timestamp unix) ; `None` = permanent.
    pub expires_at: Option<i64>,
}

impl ShareLink {
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|e| now >= e)
    }
}

/// Entree de navigation dans une source (dossier ou fichier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseEntry {
    pub name: String,
    pub uri: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub format: Option<AudioFormat>,
    pub is_audio: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayStatus {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

/// Sortie audio active : DAC local ou navigateur (« Jouer ici »).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Local,
    /// « Jouer ici » : le PCM décodé est streamé au navigateur via /ws/audio.
    Browser,
}

impl OutputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputMode::Local => "local",
            OutputMode::Browser => "browser",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(OutputMode::Local),
            "browser" => Some(OutputMode::Browser),
            _ => None,
        }
    }
}

/// Trame envoyée aux clients du flux navigateur (`/ws/audio`).
#[derive(Debug, Clone)]
pub enum BrowserFrame {
    /// Spécifie le format du flux (envoyé à chaque changement de piste).
    Spec { rate: u32, channels: u16 },
    /// Échantillons f32 entrelacés, post-gain (le volume UI s'applique côté serveur).
    Pcm(std::sync::Arc<[f32]>),
}

/// Etat complet du lecteur, envoye tel quel a l'UI (REST + WebSocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlayStatus,
    pub current: Option<Track>,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub volume: u8,
    pub muted: bool,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub queue: Vec<Track>,
    pub queue_index: Option<usize>,
    pub audio_device: Option<String>,
    pub current_format: Option<AudioFormat>,
    /// Volume max autorisé (réglage playback.max_volume).
    #[serde(default = "default_max_volume")]
    pub max_volume: u8,
    /// Mode de volume : "software" ou "disabled" (bit-perfect).
    #[serde(default = "default_volume_mode")]
    pub volume_mode: String,
    /// Sortie active : "local" (DAC) ou "browser" (« Jouer ici »).
    #[serde(default)]
    pub output_mode: OutputMode,
}

fn default_max_volume() -> u8 {
    100
}

fn default_volume_mode() -> String {
    "software".into()
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            status: PlayStatus::Stopped,
            current: None,
            position_secs: 0.0,
            duration_secs: 0.0,
            volume: 80,
            muted: false,
            repeat: RepeatMode::Off,
            shuffle: false,
            queue: Vec::new(),
            queue_index: None,
            audio_device: None,
            current_format: None,
            max_volume: 100,
            volume_mode: "software".into(),
            output_mode: OutputMode::Local,
        }
    }
}

/// Evenements temps reel pousses sur le WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: serde_json::Value,
}

impl WsEvent {
    pub fn state(state: &PlayerState) -> Self {
        WsEvent {
            kind: "state".into(),
            payload: serde_json::to_value(state).unwrap_or_default(),
        }
    }
    pub fn queue(queue: &[Track]) -> Self {
        WsEvent {
            kind: "queue".into(),
            payload: serde_json::to_value(queue).unwrap_or_default(),
        }
    }
    pub fn message(kind: &str, msg: &str) -> Self {
        WsEvent {
            kind: kind.into(),
            payload: serde_json::json!({ "message": msg }),
        }
    }
}

/// Info sur une mise a jour disponible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub checked_at: i64,
}

/// Description d'un peripherique de sortie audio (DAC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
    pub supports_i24: bool,
    pub supports_f32: bool,
    pub max_channels: u16,
}
