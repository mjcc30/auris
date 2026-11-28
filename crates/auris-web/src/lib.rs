//! Serveur web Auris: API REST + WebSocket temps reel + UI embarquee (rust-embed).
//!
//! L'UI est un build Vite (ui/dist) embarque dans le binaire: le binaire reste
//! autonome et clé en main, aucune installation фронт separee n'est requise.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use axum::{
    body::Bytes,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use auris_core::{
    lyrics, read_zip, search_semantic, Alarm, AlarmState, AurisError, BrowseEntry, FavKind,
    Favorites, HistoryEntry, LibraryStats, OutputMode, PinKind, Pins, PlayStatus, Playlist,
    RadioEntry, RepeatMode, SearchGroup, SearchResults, Settings, ShareKind, ShareLink, SmartRules,
    Source, Track, WsEvent, ZipWriter,
};
use auris_library::Library;
use auris_player::{
    probe_meta_with_path, wasapi_dop, wasapi_exclusive, AudioOutput, PlayerEngine, TrackMeta,
};
use auris_sources::{discover_removable, SourcesManager};
use auris_updater::Updater;

mod artistinfo;
mod artwork;
mod auth;
mod camilla;
mod dlna_renderer;
mod lastfm;
mod mix;
pub mod mpd;
mod multiroom;
mod network;
mod packs;
mod plugins;
mod podcasts;
mod radios;
pub mod recorder;
pub mod ripcd;
pub mod signal_path;
pub mod socketio;
pub mod spotify;
pub mod subsonic;
mod volumio_v1;

pub use multiroom::{peer_id, MultiroomHub};

#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
struct UiAssets;

/// Sink de persistance des réglages (config.toml).
pub type SettingsSink = Arc<dyn Fn(&Settings) + Send + Sync>;

/// Sink de persistance de la sortie audio (DAC + mode) dans config.toml.
pub type PlayerSink = Arc<dyn Fn(Option<String>, OutputMode) + Send + Sync>;

/// Liste les périphériques audio disponibles (injectable pour les tests).
pub type DeviceLister = Arc<dyn Fn() -> Vec<auris_core::AudioDeviceInfo> + Send + Sync>;

/// GET HTTP externe (lrclib.net, Wikipedia), injectable pour les tests.
/// Retourne le corps de la réponse, ou None si la requête échoue.
pub type ExtHttpGet = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Etat partagé de l'application web.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<PlayerEngine>,
    pub library: Arc<Library>,
    pub sources: Arc<SourcesManager>,
    pub updater: Arc<Updater>,
    pub started_at: i64,
    /// Appele apres chaque modification des sources (ajout/suppression/toggle)
    /// pour persister la config. `None` = pas de persistance (tests).
    pub sources_sink: Option<Arc<dyn Fn(Vec<Source>) + Send + Sync>>,
    /// Réglages courants (menu Réglages, façon Volumio Nova).
    pub settings: Arc<RwLock<Settings>>,
    /// Persiste les réglages dans config.toml. `None` = pas de persistance (tests).
    pub settings_sink: Option<SettingsSink>,
    /// Persiste le périphérique audio choisi + le mode de sortie (config.toml).
    /// `None` = pas de persistance (tests).
    pub player_sink: Option<PlayerSink>,
    /// Persiste le mode WASAPI exclusif bit-perfect (config.toml `audio_exclusive`).
    /// `None` = pas de persistance (tests).
    pub exclusive_sink: Option<Arc<dyn Fn(bool) + Send + Sync>>,
    /// Liste les périphériques audio (par défaut `AudioOutput::devices`).
    pub device_lister: Option<DeviceLister>,
    /// GET HTTP externe (paroles lrclib, bio Wikipedia). `None` = reqwest réel.
    pub ext_http: Option<ExtHttpGet>,
    /// Répertoire de données (stockage, logs, reset usine).
    pub data_dir: PathBuf,
    /// Autorise redémarrer/éteindre depuis l'UI (config `power_control`).
    pub power_control: bool,
    /// Exécute une action d'alimentation. `None` = commande système réelle.
    pub power_ops: Option<Arc<dyn Fn(PowerAction) + Send + Sync>>,
    /// Fichier de logs si la journalisation fichier est active.
    pub log_path: Option<PathBuf>,
    /// État de la minuterie de sommeil / du réveil (tick 1x/s dans `serve`).
    pub alarms: Arc<Mutex<AlarmState>>,
    /// Cache MusicBrainz (fiche recording par piste) — #26, construit à la volée.
    pub mb_cache: Arc<Mutex<Option<Arc<lastfm::MbCache>>>>,
    /// Adresse réellement bindée par `serve` (renseigné au démarrage) —
    /// sert de repli pour l'URL de base annoncée en SSDP/DLNA (#35).
    pub bound_addr: Arc<RwLock<Option<SocketAddr>>>,
    /// Multiroom : pairs découverts (balise multicast) + groupes persistés (#23).
    pub multiroom: Arc<multiroom::MultiroomHub>,
    /// Hôte de plugins communautaires (#8 — docs/PLUGIN_SYSTEM.md).
    pub plugins: Arc<auris_plugins::PluginHost>,
    /// Hub socket.io (Engine.IO v3) pour clients mobiles Volumio (#45).
    pub socketio: Arc<socketio::SocketIoHub>,
    /// Rip CD via whipper (#27).
    pub ripcd: Arc<ripcd::RipCdHub>,
    /// Enregistrement des webradios « comme à l'ancienne » (#101).
    pub recorder: Arc<recorder::RecorderHub>,
    /// Spotify Connect via librespot (#24).
    pub spotify: Arc<spotify::SpotHub>,
    /// Sessions de login (id -> expiration) pour le token d'accès optionnel (#97).
    pub sessions: Arc<RwLock<HashMap<String, i64>>>,
}

/// Action d'alimentation (redémarrer / éteindre la machine hôte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    Shutdown,
}

impl AppState {
    /// Construit un état avec des réglages par défaut et aucune persistance.
    pub fn new(
        engine: Arc<PlayerEngine>,
        library: Arc<Library>,
        sources: Arc<SourcesManager>,
        updater: Arc<Updater>,
        data_dir: PathBuf,
    ) -> Self {
        AppState {
            engine,
            library,
            sources,
            updater,
            started_at: chrono::Utc::now().timestamp(),
            sources_sink: None,
            settings: Arc::new(RwLock::new(Settings::default())),
            settings_sink: None,
            player_sink: None,
            exclusive_sink: None,
            device_lister: None,
            ext_http: None,
            multiroom: Arc::new(multiroom::MultiroomHub::new(
                data_dir.clone(),
                multiroom::peer_id(&data_dir),
            )),
            plugins: Arc::new(auris_plugins::PluginHost::new(&data_dir)),
            socketio: socketio::SocketIoHub::new(),
            ripcd: ripcd::RipCdHub::new(),
            recorder: recorder::RecorderHub::new(),
            spotify: spotify::SpotHub::new(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            data_dir,
            power_control: false,
            power_ops: None,
            log_path: None,
            alarms: Arc::new(Mutex::new(AlarmState::default())),
            mb_cache: Arc::new(Mutex::new(None)),
            bound_addr: Arc::new(RwLock::new(None)),
        }
    }

    fn persist_sources(&self) {
        if let Some(sink) = &self.sources_sink {
            sink(self.sources.list());
        }
    }

    fn persist_settings(&self) {
        if let Some(sink) = &self.settings_sink {
            sink(&self.settings.read().unwrap());
        }
    }

    fn list_devices(&self) -> Vec<auris_core::AudioDeviceInfo> {
        match &self.device_lister {
            Some(l) => l(),
            None => AudioOutput::devices(),
        }
    }

    fn persist_player(&self, device: Option<String>, mode: OutputMode) {
        if let Some(sink) = &self.player_sink {
            sink(device, mode);
        }
    }
}

/// Erreur API -> JSON {"error": ...}.
pub struct ApiError(pub(crate) AurisError);
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let msg = self.0.to_string();
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response()
    }
}
impl From<AurisError> for ApiError {
    fn from(e: AurisError) -> Self {
        ApiError(e)
    }
}
pub(crate) type ApiResult<T> = Result<T, ApiError>;

/// Exécute une opération d'IO bloquante (listing disque/réseau des sources,
/// sonde de pochette embarquée...) sur le pool de threads bloquants de tokio
/// au lieu d'un worker async : l'UI web reste réactive pendant le parcours
/// des dossiers, même sur une source SMB lente.
pub(crate) async fn offload_io<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> auris_core::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(ApiError::from),
        Err(e) => Err(ApiError(auris_core::AurisError::other(format!(
            "tâche d'IO interrompue: {e}"
        )))),
    }
}

// ---------- payloads ----------

#[derive(Deserialize)]
struct PlayBody {
    uri: Option<String>,
    track_id: Option<String>,
    queue: Option<Vec<Track>>,
    index: Option<usize>,
}

#[derive(Deserialize)]
struct SeekBody {
    secs: f64,
}

#[derive(Deserialize)]
struct VolumeBody {
    volume: u8,
}

#[derive(Deserialize)]
struct MuteBody {
    muted: bool,
}

#[derive(Deserialize)]
struct RepeatBody {
    mode: RepeatMode,
}

#[derive(Deserialize)]
struct ShuffleBody {
    enabled: bool,
}

#[derive(Deserialize)]
struct ScanBody {
    source_id: Option<String>,
}

#[derive(Deserialize)]
struct DeviceBody {
    name: Option<String>,
}

#[derive(Deserialize)]
struct BrowseQuery {
    uri: String,
}

#[derive(Deserialize)]
struct CopyBody {
    /// URI du fichier ou dossier a copier: `src://<id>/<chemin>`.
    from: String,
    /// URI du dossier destination: `src://<id>/<chemin>`.
    to: String,
}

#[derive(Deserialize)]
struct ExportQuery {
    uri: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct PlaylistBody {
    name: String,
    /// Si absent: la file d'attente courante est enregistree.
    track_ids: Option<Vec<String>>,
    /// Si renseigne: la playlist est cree a partir de cet URI (fichier ou
    /// dossier, parcouru recursivement) — utilisé depuis Parcourir.
    uri: Option<String>,
    /// Si renseigne: smart playlist (#98) — la liste est évaluee dynamiquement
    /// contre la bibliothèque à chaque lecture.
    rules: Option<SmartRules>,
}

#[derive(Deserialize)]
struct PlaylistPatchBody {
    name: Option<String>,
    /// `Some(Some(r))` = règles, `Some(None)` = repasse en statique, absent = inchangé.
    rules: Option<Option<SmartRules>>,
}

#[derive(Deserialize)]
struct ToggleBody {
    enabled: bool,
}

#[derive(Deserialize)]
struct QueueAddBody {
    uri: String,
}

#[derive(Serialize)]
struct SystemInfo {
    name: &'static str,
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    target: String,
    uptime_secs: i64,
    track_count: usize,
    source_count: usize,
    audio_device: Option<String>,
}

// ---------- routes ----------

pub fn router(state: AppState) -> Router {
    // Pont événements moteur -> pushState socket.io pour tous les clients (#45).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let mut events = state.engine.events.subscribe();
        let st = state.clone();
        handle.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(_) => socketio::broadcast_state(&st),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    let api = Router::new()
        // compatibilité socket.io (clients mobiles Volumio) — #45
        .route(
            "/socket.io/",
            get(socketio::socketio_get).post(socketio::socketio_post),
        )
        // lecteur
        .route("/api/state", get(get_state))
        .route("/api/player/play", post(post_play))
        .route(
            "/api/player/pause",
            post(|State(s): State<AppState>| async move {
                s.engine.pause();
                Json(s.engine.state())
            }),
        )
        .route(
            "/api/player/resume",
            post(|State(s): State<AppState>| async move {
                s.engine.resume();
                Json(s.engine.state())
            }),
        )
        .route(
            "/api/player/stop",
            post(|State(s): State<AppState>| async move {
                s.engine.stop();
                Json(s.engine.state())
            }),
        )
        .route(
            "/api/player/next",
            post(|State(s): State<AppState>| async move {
                s.engine.next();
                Json(s.engine.state())
            }),
        )
        .route(
            "/api/player/prev",
            post(|State(s): State<AppState>| async move {
                s.engine.prev();
                Json(s.engine.state())
            }),
        )
        .route("/api/player/seek", post(post_seek))
        .route("/api/player/volume", post(post_volume))
        .route("/api/player/mute", post(post_mute))
        .route("/api/player/repeat", post(post_repeat))
        .route("/api/player/shuffle", post(post_shuffle))
        // file d'attente
        .route("/api/queue", get(get_queue).post(post_queue))
        .route("/api/queue/add", post(post_queue_add))
        .route("/api/queue/add-next", post(post_queue_add_next))
        .route(
            "/api/queue/clear",
            post(|State(s): State<AppState>| async move {
                s.engine.clear_queue();
                Json(s.engine.state())
            }),
        )
        .route("/api/queue/{index}", delete(delete_queue_item))
        // bibliotheque
        .route("/api/library/tracks", get(get_tracks))
        .route("/api/library/tracks/page", get(get_tracks_page))
        .route("/api/library/tracks/{id}", get(get_track))
        .route("/api/library/facets", get(get_facets))
        .route("/api/library/stats", get(get_library_stats))
        .route("/api/library/history", get(get_history))
        .route("/api/library/loudness/scan", post(post_loudness_scan))
        .route("/api/library/loudness/{id}", get(get_loudness))
        .route("/api/favorites", get(get_favorites))
        .route("/api/favorites/toggle", post(post_favorite_toggle))
        .route("/api/favorites/resolved", get(get_favorites_resolved))
        .route("/api/pins", get(get_pins))
        .route("/api/pins/toggle", post(post_pin_toggle))
        .route("/api/pins/resolved", get(get_pins_resolved))
        .route("/api/library/albums", get(get_albums))
        .route("/api/library/albums/{id}", get(get_album))
        .route("/api/library/artists", get(get_artists))
        .route("/api/library/artists/{id}", get(get_artist))
        .route("/api/library/search", get(get_search))
        .route("/api/library/cover/{track_id}", get(get_cover))
        .route(
            "/api/library/artist_image/{artist_id}",
            get(artwork::get_artist_image),
        )
        .route(
            "/api/artist_image/name/{name}",
            get(artwork::get_artist_image_by_name),
        )
        .route(
            "/api/library/album_image/{album_id}",
            get(artwork::get_album_image),
        )
        .route("/api/lyrics/{track_id}", get(get_lyrics))
        // alias nommé explicitement dans le ticket #102
        .route("/api/track_lyrics/{track_id}", get(get_lyrics))
        .route(
            "/api/library/artist_info/{artist_id}",
            get(artistinfo::get_artist_info),
        )
        .route("/api/infos/{track_id}", get(get_infos))
        .route("/api/library/scan", post(post_scan))
        .route("/api/library/scan/status", get(get_scan_status))
        // sources
        .route("/api/sources", get(get_sources).post(post_source))
        .route(
            "/api/sources/{id}",
            delete(delete_source).patch(patch_source).put(put_source),
        )
        .route("/api/browse", get(get_browse))
        .route("/api/browse/tree", get(get_browse_tree))
        .route("/api/browse/cover", get(get_browse_cover))
        // radios web
        .route(
            "/api/radios",
            get(radios::get_radios).post(radios::post_radio),
        )
        .route("/api/radios/dir/genres", get(radios::get_dir_genres))
        .route("/api/radios/dir/countries", get(radios::get_dir_countries))
        .route("/api/radios/dir/stations", get(radios::get_dir_stations))
        .route("/api/radios/selection", get(radios::get_selection))
        .route("/api/radios/artist", get(radios::get_artist_radio))
        .route("/api/radios/artist-radio", post(radios::post_artist_radio))
        // Instant Mix (#105)
        .route("/api/mix", post(mix::post_mix))
        .route("/api/radios/play-url", post(radios::post_play_url))
        // podcasts (#81)
        .route("/api/podcasts", get(podcasts::get_podcasts))
        .route("/api/podcasts/feed", get(podcasts::get_feed))
        .route(
            "/api/podcasts/subscriptions",
            get(podcasts::get_subscriptions),
        )
        .route("/api/podcasts/subscribe", post(podcasts::post_subscribe))
        .route(
            "/api/podcasts/subscriptions/{id}",
            delete(podcasts::delete_subscription),
        )
        .route("/api/podcasts/play", post(podcasts::post_play))
        // rip CD (whipper) — #27
        .route("/api/ripcd/status", get(ripcd::get_status))
        .route("/api/ripcd/info", post(ripcd::post_info))
        .route("/api/ripcd/rip", post(ripcd::post_rip))
        .route("/api/ripcd/stop", post(ripcd::post_stop))
        .route("/api/ripcd/eject", post(ripcd::post_eject))
        // Spotify Connect (librespot) — #24
        .route("/api/spotify/status", get(spotify::get_status))
        .route("/api/spotify/start", post(spotify::post_start))
        .route("/api/spotify/stop", post(spotify::post_stop))
        .route("/api/spotify/event", post(spotify::post_event))
        .route("/api/radios/{id}", delete(radios::delete_radio))
        .route("/api/radios/{id}/play", post(radios::post_radio_play))
        // enregistrement radio « comme à l'ancienne » (#101)
        .route("/api/radio/record/status", get(recorder::get_status))
        .route("/api/radio/record/start", post(recorder::post_start))
        .route("/api/radio/record/stop", post(recorder::post_stop))
        // packs d'interface (thèmes CSS + langues) embarqués ou communautaires
        .route("/api/packs", get(packs::get_packs))
        .route(
            "/api/packs/theme/{file}",
            get(packs::get_theme_css)
                .post(packs::post_theme)
                .delete(packs::delete_theme),
        )
        .route(
            "/api/packs/locale/{file}",
            get(packs::get_locale_json)
                .post(packs::post_locale)
                .delete(packs::delete_locale),
        )
        .route("/api/upnp/discover", get(get_upnp_discover))
        .route("/api/usb/devices", get(get_usb_devices))
        // import / export
        .route("/api/transfer/copy", post(post_copy))
        .route("/api/export", get(get_export))
        .route("/api/library/albums/{id}/export", get(get_album_export))
        // listes de lecture
        .route("/api/playlists", get(get_playlists).post(post_playlist))
        .route(
            "/api/playlists/{id}",
            get(get_playlist)
                .delete(delete_playlist)
                .patch(patch_playlist),
        )
        .route("/api/playlists/{id}/play", post(post_playlist_play))
        .route("/api/playlists/{id}/add", post(post_playlist_add))
        // audio
        .route("/api/audio/devices", get(get_devices))
        // entrées audio (line-in #75) : périphériques de capture disponibles
        .route("/api/audio/input-devices", get(get_input_devices))
        .route("/api/audio/device", post(post_device))
        .route("/api/audio/dop-status", get(get_dop_status))
        .route("/api/audio/exclusive", post(post_exclusive))
        .route("/api/audio/dop-setup", post(post_dop_setup))
        .route("/api/audio/restart-audio", post(post_restart_audio))
        // systeme
        .route("/api/system/info", get(get_system_info))
        .route("/api/system/update", get(get_update).post(post_update))
        .route("/api/system/logs", get(get_logs))
        .route("/api/system/storage", get(get_storage))
        .route("/api/system/reset", post(post_reset))
        .route("/api/system/reboot", post(post_reboot))
        .route("/api/system/shutdown", post(post_shutdown))
        // réglages (menu Réglages façon Volumio Nova)
        .route("/api/settings", get(get_settings).put(put_settings))
        // sauvegarde / restauration de la configuration (#40)
        .route("/api/settings/backup", get(get_backup))
        .route("/api/settings/restore", post(post_restore))
        .route("/api/settings/backup/webdav", post(post_backup_webdav))
        .route("/api/settings/restore/webdav", post(post_restore_webdav))
        .route("/api/lastfm/test", post(post_lastfm_test))
        .route("/api/lastfm/token", post(post_lastfm_token))
        .route("/api/lastfm/session", post(post_lastfm_session))
        .route("/api/alarms", get(get_alarms))
        .route("/api/shares", get(get_shares).post(post_share))
        .route("/api/shares/{token}", delete(delete_share))
        // multiroom / regroupement — #23
        // interfaces réseau (#44)
        .route(
            "/api/network/interfaces",
            get(network::get_interfaces).post(network::post_interface),
        )
        // plugins communautaires (#8)
        .route("/api/plugins", get(plugins::get_plugins))
        .route("/api/plugins/install", post(plugins::post_install))
        .route("/api/plugins/{id}/enable", post(plugins::post_enable))
        .route("/api/plugins/{id}/disable", post(plugins::post_disable))
        .route("/api/plugins/{id}", delete(plugins::delete_plugin))
        .route("/api/plugins/{id}/call", post(plugins::post_call))
        .route("/api/plugins/{id}/test", post(plugins::post_test))
        .route("/api/multiroom/state", get(multiroom::get_state))
        .route("/api/multiroom/clock", get(multiroom::get_clock))
        .route("/api/multiroom/groups", post(multiroom::post_group))
        .route(
            "/api/multiroom/groups/{id}",
            delete(multiroom::delete_group),
        )
        .route(
            "/api/multiroom/groups/{id}/members",
            post(multiroom::post_member),
        )
        .route("/api/multiroom/play", post(multiroom::post_play))
        .route("/api/multiroom/pause", post(multiroom::post_pause))
        .route("/api/multiroom/stop", post(multiroom::post_stop))
        .route("/api/multiroom/seek", post(multiroom::post_seek))
        .route("/api/multiroom/volume", post(multiroom::post_volume))
        .route("/api/multiroom/receive", post(multiroom::post_receive))
        // websocket
        .route("/ws", get(ws_handler))
        .route("/ws/audio", get(ws_audio_handler))
        .route("/api/player/output", post(post_output));

    // Routes publiques même quand un token d'accès est configuré (#97) :
    // authentification, liens de partage (token dans l'URL) et enceinte DLNA
    // (UPnP n'a pas de mécanisme d'auth standard).
    let public = auth::router()
        .route("/api/public/shares/{token}", get(public_share))
        .route("/share/{token}", get(share_page))
        .route("/share/{token}/stream", get(share_stream))
        .route("/share/{token}/stream/{track_id}", get(share_stream_track))
        .route("/dlna/desc.xml", get(dlna_renderer::get_desc))
        .route("/dlna/scpd/{name}", get(dlna_renderer::get_scpd))
        .route("/dlna/avtransport", post(dlna_renderer::post_avtransport))
        .route("/dlna/rendering", post(dlna_renderer::post_rendering));

    // Tout le reste de l'API est protégé par le token d'accès s'il est configuré (#97).
    let protected = Router::new()
        .merge(api)
        // compatibilité REST v1 façon Volumio (clients tiers) — #76
        .merge(volumio_v1::router())
        // chaîne du signal façon Roon « Signal Path » — #77
        .merge(signal_path::router())
        // pont CamillaDSP (statut + profil) — #82
        .merge(camilla::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        // Subsonic a sa propre auth (u/t/salt, style Navidrome)
        .merge(subsonic::router())
        .route("/", get(index_html))
        .route("/{*path}", get(static_asset))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state.clone());
    // annonce SSDP de l'enceinte DLNA (#35) — thread dédié, ne bloque jamais
    {
        let ip = if addr.ip().is_unspecified() {
            dlna_renderer::local_ip()
        } else {
            addr.ip().to_string()
        };
        let base = format!("http://{ip}:{}", addr.port());
        dlna_renderer::spawn_announcer(state.clone(), base);
    }
    // clone pour la balise multiroom, lancée après le bind (cf. ci-dessous)
    let state_mr = state.clone();
    // tâche de fond : historique d'écoute (#69) — enregistre chaque piste qui démarre
    // (dédup consécutifs géré par Library::record_play).
    {
        let mut events = state.engine.events.subscribe();
        let library = state.library.clone();
        tokio::spawn(async move {
            let mut last: Option<String> = None;
            loop {
                match events.recv().await {
                    Ok(ev) if ev.kind == "state" => {
                        let cur = ev
                            .payload
                            .get("current")
                            .and_then(|c| c.get("id"))
                            .and_then(|i| i.as_str())
                            .map(|s| s.to_string());
                        if cur != last {
                            if let Some(id) = &cur {
                                if let Err(e) = library.record_play(id) {
                                    tracing::warn!("historique: {e}");
                                }
                            }
                            last = cur;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    // tâche de fond : scrobble Last.fm + now-playing (#26)
    lastfm::spawn_scrobbler(state.clone());
    // tâche de fond : minuterie de sommeil + réveil (vérification 1x/s)
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let now = chrono::Utc::now().timestamp();
            let offset = chrono::Local::now().offset().local_minus_utc();
            let (sleep, wake) = {
                let r = state.settings.read().unwrap();
                (r.playback.sleep_timer, r.playback.wake_time.clone())
            };
            let events = {
                let mut a = state.alarms.lock().unwrap();
                auris_core::tick(now, offset, sleep, wake.as_deref(), &mut a)
            };
            for ev in events {
                match ev {
                    Alarm::Sleep => {
                        tracing::info!("minuterie de sommeil expirée: pause");
                        state.engine.pause();
                        let _ = state.engine.events.send(WsEvent::message(
                            "alarm",
                            "Minuterie de sommeil terminée : lecture en pause",
                        ));
                    }
                    Alarm::Wake => {
                        tracing::info!(
                            "réveil {}: relance de la lecture",
                            wake.as_deref().unwrap_or("")
                        );
                        let st = state.engine.state();
                        match st.status {
                            PlayStatus::Paused => state.engine.resume(),
                            PlayStatus::Stopped if !st.queue.is_empty() => {
                                let idx = st.queue_index.unwrap_or(0).min(st.queue.len() - 1);
                                state.engine.play_queue(st.queue.clone(), idx);
                            }
                            _ => {}
                        }
                        let _ = state
                            .engine
                            .events
                            .send(WsEvent::message("alarm", "Réveil : lecture relancée"));
                    }
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr().unwrap_or(addr);
    *state_mr.bound_addr.write().unwrap() = Some(bound);
    // balise multicast de découverte des pairs multiroom (#23) — après le
    // bind pour annoncer la bonne URL de base
    multiroom::spawn_beacon(state_mr);
    tracing::info!("interface web: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- handlers ----------

async fn get_state(State(s): State<AppState>) -> Json<auris_core::PlayerState> {
    Json(s.engine.state())
}

async fn post_play(
    State(s): State<AppState>,
    Json(body): Json<PlayBody>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    if let Some(queue) = body.queue {
        let idx = body.index.unwrap_or(0);
        // Adopte les vrais Tracks de la bibliothèque quand l'uri est scannée :
        // l'id de la piste en cours doit rester celui de la bibliothèque,
        // sinon « Partager » renvoie « piste introuvable ».
        let queue = queue
            .into_iter()
            .map(|t| s.library.get_by_uri(&t.uri).unwrap_or(t))
            .collect::<Vec<_>>();
        // Enregistre les pistes jouées dans la bibliothèque: les lookups par
        // id (paroles / infos / partage / pochette) doivent retomber sur la
        // piste, même si elle vient de Parcourir et n'a pas été scannée.
        // Les URI http(s) (webradios) ne sont PAS enregistrées : ce ne sont
        // pas des fichiers scannés.
        for t in &queue {
            if !auris_sources::is_http_uri(&t.uri) {
                let _ = s.library.upsert_track(t);
            }
        }
        s.engine.play_queue(queue, idx);
    } else if let Some(track_id) = body.track_id {
        let t = s
            .library
            .track(&track_id)
            .ok_or_else(|| AurisError::TrackNotFound(track_id.clone()))?;
        s.engine.play_queue(vec![t], 0);
    } else if let Some(uri) = body.uri {
        // un dossier est joue en "tout lire": file remplie de ses fichiers audio
        let sources = s.sources.clone();
        let uri_stat = uri.clone();
        let handle = tokio::runtime::Handle::current();
        let is_dir = offload_io(move || handle.block_on(sources.stat(&uri_stat)).map(|e| e.is_dir))
            .await
            .unwrap_or(false);
        if is_dir {
            let tracks = browse_tracks_offload(&s.sources, &uri).await?;
            if tracks.is_empty() {
                return Err(AurisError::other("dossier sans fichier audio".to_string()).into());
            }
            let tracks = tracks
                .into_iter()
                .map(|t| s.library.get_by_uri(&t.uri).unwrap_or(t))
                .collect::<Vec<_>>();
            for t in &tracks {
                if !auris_sources::is_http_uri(&t.uri) {
                    let _ = s.library.upsert_track(t);
                }
            }
            s.engine.play_queue(tracks, 0);
            return Ok(Json(s.engine.state()));
        }
        // fichier: piste de la bibliothèque si scannée, sinon lecture par uri
        if let Some(t) = s.library.get_by_uri(&uri) {
            s.engine.play_queue(vec![t], 0);
        } else if auris_sources::is_http_uri(&uri) {
            // URL http(s) directe (webradio / fichier distant) : lecture par
            // uri sans upsert bibliothèque (pas un fichier scanné)
            let mut t = Track::new(uri.clone(), "http".into());
            t.title = uri
                .rsplit('/')
                .next()
                .unwrap_or(&uri)
                .split(['?', '#'])
                .next()
                .unwrap_or(&uri)
                .to_string();
            t.finalize_keys();
            s.engine.play_queue(vec![t], 0);
        } else {
            // piste non scannée: on la crée et on l'enregistre pour que les
            // lookups par id (paroles / infos / partage) fonctionnent ensuite
            let source_id = auris_sources::parse_src_uri(&uri)
                .map(|(id, _)| id)
                .unwrap_or_default();
            let mut t = Track::new(uri.clone(), source_id);
            t.finalize_keys();
            let _ = s.library.upsert_track(&t);
            s.engine.play_queue(vec![t], 0);
        }
    } else {
        return Err(AurisError::InvalidUri("uri, track_id ou queue requis".into()).into());
    }
    Ok(Json(s.engine.state()))
}

/// Construit une piste jouable a partir d'une entree de parcours.
fn track_from_entry(e: BrowseEntry) -> Track {
    let source_id = auris_sources::parse_src_uri(&e.uri)
        .map(|(id, _)| id)
        .unwrap_or_default();
    let mut t = Track::new(e.uri.clone(), source_id);
    t.title = e.name.clone();
    t.size_bytes = e.size;
    t.finalize_keys();
    t
}

/// Construit des pistes jouables a partir des fichiers audio d'un dossier.
/// Le parcours récursif (IO disque/réseau bloquantes, parfois long
/// sur un NAS) est exécuté hors des workers async pour garder l'UI réactive.
async fn browse_tracks_offload(
    sources: &Arc<SourcesManager>,
    dir_uri: &str,
) -> ApiResult<Vec<Track>> {
    let sources = sources.clone();
    let uri = dir_uri.to_string();
    let handle = tokio::runtime::Handle::current();
    let entries = offload_io(move || handle.block_on(sources.walk_audio(&uri))).await?;
    Ok(entries.into_iter().map(track_from_entry).collect())
}

async fn post_seek(
    State(s): State<AppState>,
    Json(b): Json<SeekBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.seek(b.secs);
    Json(s.engine.state())
}
async fn post_volume(
    State(s): State<AppState>,
    Json(b): Json<VolumeBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.set_volume(b.volume);
    Json(s.engine.state())
}
async fn post_mute(
    State(s): State<AppState>,
    Json(b): Json<MuteBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.set_muted(b.muted);
    Json(s.engine.state())
}
async fn post_repeat(
    State(s): State<AppState>,
    Json(b): Json<RepeatBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.set_repeat(b.mode);
    Json(s.engine.state())
}
async fn post_shuffle(
    State(s): State<AppState>,
    Json(b): Json<ShuffleBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.set_shuffle(b.enabled);
    Json(s.engine.state())
}

async fn get_queue(State(s): State<AppState>) -> Json<Vec<Track>> {
    Json(s.engine.state().queue)
}
async fn post_queue(
    State(s): State<AppState>,
    Json(t): Json<Track>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    s.engine.enqueue(t);
    Ok(Json(s.engine.state()))
}

/// Ajoute un URI (fichier, dossier ou URL) a la file d'attente.
async fn post_queue_add(
    State(s): State<AppState>,
    Json(b): Json<QueueAddBody>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    // une URL http(s) n'est jamais un dossier : on saute le stat réseau
    let is_dir = if auris_sources::is_http_uri(&b.uri) {
        false
    } else {
        let sources = s.sources.clone();
        let uri_stat = b.uri.clone();
        let handle = tokio::runtime::Handle::current();
        offload_io(move || handle.block_on(sources.stat(&uri_stat)).map(|e| e.is_dir))
            .await
            .unwrap_or(false)
    };
    if is_dir {
        for t in browse_tracks_offload(&s.sources, &b.uri).await? {
            s.engine.enqueue(t);
        }
        return Ok(Json(s.engine.state()));
    }
    let source_id = auris_sources::parse_src_uri(&b.uri)
        .map(|(id, _)| id)
        .unwrap_or_default();
    let mut t = Track::new(b.uri.clone(), source_id);
    t.title = b.uri.rsplit('/').next().unwrap_or(&b.uri).to_string();
    t.finalize_keys();
    s.engine.enqueue(t);
    Ok(Json(s.engine.state()))
}

/// « Lire ensuite » : insere un URI (fichier, dossier ou URL) juste apres la
/// piste en cours dans la file d'attente.
async fn post_queue_add_next(
    State(s): State<AppState>,
    Json(b): Json<QueueAddBody>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    // une URL http(s) n'est jamais un dossier : on saute le stat réseau
    let is_dir = if auris_sources::is_http_uri(&b.uri) {
        false
    } else {
        let sources = s.sources.clone();
        let uri_stat = b.uri.clone();
        let handle = tokio::runtime::Handle::current();
        offload_io(move || handle.block_on(sources.stat(&uri_stat)).map(|e| e.is_dir))
            .await
            .unwrap_or(false)
    };
    if is_dir {
        let tracks = browse_tracks_offload(&s.sources, &b.uri).await?;
        for t in &tracks {
            if !auris_sources::is_http_uri(&t.uri) {
                let _ = s.library.upsert_track(t);
            }
        }
        s.engine.enqueue_next(tracks);
        return Ok(Json(s.engine.state()));
    }
    let source_id = auris_sources::parse_src_uri(&b.uri)
        .map(|(id, _)| id)
        .unwrap_or_default();
    let mut t = Track::new(b.uri.clone(), source_id);
    t.title = b.uri.rsplit('/').next().unwrap_or(&b.uri).to_string();
    t.finalize_keys();
    if !auris_sources::is_http_uri(&t.uri) {
        let _ = s.library.upsert_track(&t);
    }
    s.engine.enqueue_next(vec![t]);
    Ok(Json(s.engine.state()))
}
async fn delete_queue_item(
    State(s): State<AppState>,
    Path(i): Path<usize>,
) -> Json<auris_core::PlayerState> {
    s.engine.remove_queue_item(i);
    Json(s.engine.state())
}

async fn get_tracks(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Vec<Track>> {
    let genre = q.get("genre").map(|s| s.as_str());
    let decade = q.get("decade").and_then(|d| d.parse::<u32>().ok());
    // Liste allégée : les paroles embarquées restent sur la route individuelle (#73).
    let mut tracks = s.library.tracks_filtered(genre, decade);
    for t in &mut tracks {
        t.lyrics = None;
    }
    Json(tracks)
}

/// `GET /api/library/tracks/page?limit&offset&q&genre&decade` — page de pistes avec total
/// (`{total, items}`), pour un onglet Titres fluide même avec des dizaines de milliers de
/// pistes (#89). Les paroles embarquées sont ôtées de la liste.
async fn get_tracks_page(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 1000);
    let offset = q
        .get("offset")
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    let genre = q.get("genre").map(|s| s.as_str());
    let decade = q.get("decade").and_then(|d| d.parse::<u32>().ok());
    let query = q.get("q").map(|s| s.as_str());
    let (total, mut items) = s.library.tracks_page(genre, decade, query, limit, offset);
    for t in &mut items {
        t.lyrics = None;
    }
    Json(serde_json::json!({ "total": total, "items": items }))
}

/// `GET /api/library/tracks/{id}` — piste complète, crédits étendus et paroles embarquées inclus (#73).
async fn get_track(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Track>> {
    let t = s
        .library
        .track(&id)
        .ok_or_else(|| AurisError::TrackNotFound(id.clone()))?;
    Ok(Json(t))
}

/// `GET /api/library/facets` — facettes genre/décennie pour l'accueil façon Nova.
async fn get_facets(State(s): State<AppState>) -> Json<serde_json::Value> {
    let genres: Vec<serde_json::Value> = s
        .library
        .genres()
        .into_iter()
        .map(|g| {
            serde_json::json!({
                "name": g.name,
                "track_count": g.track_count,
                "album_count": g.album_count,
                "cover_url": g.cover_track_id.as_ref().map(|c| format!("/api/library/cover/{c}")),
            })
        })
        .collect();
    let decades: Vec<serde_json::Value> = s
        .library
        .decades()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "decade": d.decade,
                "track_count": d.track_count,
                "album_count": d.album_count,
                "cover_url": d.cover_track_id.as_ref().map(|c| format!("/api/library/cover/{c}")),
            })
        })
        .collect();
    Json(serde_json::json!({ "genres": genres, "decades": decades }))
}

/// `GET /api/library/stats` — compteurs globaux de l'index (équivalent Volumio `collectionstats`, #74).
async fn get_library_stats(State(s): State<AppState>) -> Json<LibraryStats> {
    Json(s.library.stats())
}

/// `GET /api/library/loudness/{id}` — loudness intégrée (LUFS) en cache pour une
/// piste (null si non mesurée) — #37.
async fn get_loudness(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "track_id": id,
        "lufs": s.library.loudness_get(&id),
        "cached_count": s.library.loudness_count(),
    }))
}

/// `POST /api/library/loudness/scan` — lance l'analyse R128 de toute la
/// bibliothèque en arrière-plan (décodage complet des fichiers locaux) et
/// met en cache le LUFS de chaque piste. Retourne immédiatement les compteurs.
async fn post_loudness_scan(State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let lib = s.library.clone();
    let sources = s.sources.clone();
    std::thread::spawn(move || {
        let tracks = lib.tracks();
        let mut ok = 0usize;
        let mut failed = 0usize;
        for t in &tracks {
            let Some(path) = sources.local_path_for_uri(&t.uri) else {
                failed += 1;
                continue;
            };
            match auris_player::loudness::analyze_file(&path) {
                Ok(lufs) if lufs.is_finite() => {
                    if lib.loudness_set(&t.id, lufs).is_ok() {
                        ok += 1;
                    } else {
                        failed += 1;
                    }
                }
                _ => failed += 1,
            }
        }
        tracing::info!("loudness scan terminé: {ok} pistes analysées, {failed} sans mesure");
    });
    Ok(Json(serde_json::json!({
        "started": true,
        "total": s.library.track_count(),
        "cached": s.library.loudness_count(),
    })))
}

/// `GET /api/library/history?limit=50` — pistes récemment jouées (plus récente d'abord),
/// chacune avec `last_played` (timestamp Unix) — #69.
async fn get_history(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Vec<serde_json::Value>> {
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let out = s
        .library
        .history_tracks(limit)
        .into_iter()
        .map(|(t, played_at)| {
            let mut v = serde_json::to_value(&t).unwrap_or_default();
            if let Some(m) = v.as_object_mut() {
                m.insert("last_played".into(), played_at.into());
            }
            v
        })
        .collect();
    Json(out)
}

/// `GET /api/favorites` — IDs des favoris (pistes / albums / artistes).
async fn get_favorites(State(s): State<AppState>) -> Json<Favorites> {
    Json(s.library.favorites())
}

#[derive(Deserialize)]
struct FavToggleBody {
    kind: FavKind,
    id: String,
}

/// `POST /api/favorites/toggle` — bascule un favori, renvoie le nouvel état.
async fn post_favorite_toggle(
    State(s): State<AppState>,
    Json(b): Json<FavToggleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let favorited = s.library.toggle_favorite(b.kind, &b.id)?;
    Ok(Json(serde_json::json!({
        "kind": b.kind,
        "id": b.id,
        "favorited": favorited,
    })))
}

/// `GET /api/favorites/resolved` — favoris résolus en objets complets (pour la vue Favoris).
async fn get_favorites_resolved(State(s): State<AppState>) -> Json<serde_json::Value> {
    let tracks = s.library.favorite_tracks();
    let albums: Vec<serde_json::Value> = s
        .library
        .favorite_albums()
        .into_iter()
        .map(|a| {
            let mut v = serde_json::to_value(&a).unwrap_or_default();
            if let Some(cover_id) = &a.cover_track_id {
                v["cover_url"] =
                    serde_json::Value::String(format!("/api/library/cover/{cover_id}"));
            }
            v
        })
        .collect();
    let artists = s.library.favorite_artists();
    Json(serde_json::json!({
        "tracks": tracks,
        "albums": albums,
        "artists": artists,
    }))
}

/// `GET /api/pins` — IDs des épingles (albums / playlists / radios), #70.
async fn get_pins(State(s): State<AppState>) -> Json<Pins> {
    Json(s.library.pins())
}

#[derive(Deserialize)]
struct PinToggleBody {
    kind: PinKind,
    id: String,
}

/// `POST /api/pins/toggle` — bascule une épingle, renvoie le nouvel état.
async fn post_pin_toggle(
    State(s): State<AppState>,
    Json(body): Json<PinToggleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.id.trim().is_empty() {
        return Err(AurisError::other("id vide".to_string()).into());
    }
    let on = s.library.toggle_pin(body.kind, &body.id)?;
    Ok(Json(serde_json::json!({
        "kind": body.kind,
        "id": body.id,
        "pinned": on,
    })))
}

/// `GET /api/pins/resolved` — épingles résolues : albums (avec cover_url) et playlists
/// complets, radios en IDs/URLs bruts (résolus côté client dans la liste des radios).
async fn get_pins_resolved(State(s): State<AppState>) -> Json<serde_json::Value> {
    let albums: Vec<serde_json::Value> = s
        .library
        .pinned_albums()
        .into_iter()
        .map(|a| {
            let mut v = serde_json::to_value(&a).unwrap_or_default();
            if let Some(cover_id) = &a.cover_track_id {
                v["cover_url"] =
                    serde_json::Value::String(format!("/api/library/cover/{cover_id}"));
            }
            v
        })
        .collect();
    let playlists = s.library.pinned_playlists();
    let radios = s.library.pins().radios;
    Json(serde_json::json!({
        "albums": albums,
        "playlists": playlists,
        "radios": radios,
    }))
}
async fn get_albums(State(s): State<AppState>) -> Json<serde_json::Value> {
    let albums = s.library.albums();
    let with_covers: Vec<serde_json::Value> = albums
        .into_iter()
        .map(|a| {
            let mut v = serde_json::to_value(&a).unwrap_or_default();
            if let Some(cover_id) = &a.cover_track_id {
                v["cover_url"] =
                    serde_json::Value::String(format!("/api/library/cover/{cover_id}"));
            }
            v
        })
        .collect();
    Json(serde_json::Value::Array(with_covers))
}
async fn get_album(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (album, tracks) = s
        .library
        .album(&id)
        .ok_or_else(|| AurisError::AlbumNotFound(id.clone()))?;
    Ok(Json(serde_json::json!({
        "album": album,
        "tracks": tracks,
        "cover_url": album.cover_track_id.as_ref().map(|c| format!("/api/library/cover/{c}")),
    })))
}
async fn get_artists(State(s): State<AppState>) -> Json<Vec<auris_core::Artist>> {
    Json(s.library.artists())
}
async fn get_artist(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (artist, albums) = s
        .library
        .artist(&id)
        .ok_or_else(|| AurisError::ArtistNotFound(id.clone()))?;
    let tracks = s.library.artist_tracks(&id).unwrap_or_default();
    // Enrichissements pour la page artiste : genres, plage d'années, durée totale, bio en cache.
    let mut genres: Vec<String> = tracks
        .iter()
        .filter_map(|t| t.genre.clone())
        .flat_map(|g| {
            g.split(';')
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|g| !g.is_empty())
        .collect();
    genres.sort();
    genres.dedup();
    let years: Vec<u32> = tracks.iter().filter_map(|t| t.year).collect();
    let duration_secs: f64 = tracks.iter().filter_map(|t| t.duration_secs).sum();
    let bio = s.library.bio_get(&artist.name.to_lowercase());
    Ok(Json(serde_json::json!({
        "artist": artist,
        "albums": albums,
        "tracks": tracks,
        "genres": genres,
        "year_min": years.iter().min().copied(),
        "year_max": years.iter().max().copied(),
        "duration_secs": duration_secs,
        "bio": bio,
        "cover_url": albums
            .iter()
            .find_map(|a| a.cover_track_id.as_ref())
            .map(|c| format!("/api/library/cover/{c}")),
    })))
}
async fn get_search(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<SearchResults> {
    let query = q.get("q").cloned().unwrap_or_default();
    let source = q.get("source").map(String::as_str);
    let mut res = s.library.search_grouped(&query, source);
    // Groupe radios (#38) : favoris + intégrées + sources radio, scorées comme le reste.
    let terms = search_semantic::query_terms(&query);
    let rl = radios::list_radios(&s);
    let mut scored: Vec<(f64, RadioEntry)> = rl
        .favorites
        .into_iter()
        .chain(rl.builtin)
        .chain(rl.sources)
        .map(|r| {
            let sc = search_semantic::score_fields(&terms, &[(&r.name, 3.0)]);
            (sc, r)
        })
        .filter(|(sc, _)| *sc > 0.0 || terms.is_empty()) // q vide = tout
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    let total = scored.len();
    res.radios = SearchGroup {
        total,
        items: scored
            .into_iter()
            .take(auris_library::SEARCH_CAP)
            .map(|(_, r)| r)
            .collect(),
    };
    // Groupe sources externes (Qobuz #25) : recherche déléguée aux backends
    // qui la supportent (les autres renvoient une liste vide).
    let src_items = s.sources.search_sources(&query).await;
    let total = src_items.len();
    res.sources = SearchGroup {
        total,
        items: src_items
            .into_iter()
            .take(auris_library::SEARCH_CAP)
            .collect(),
    };
    Json(res)
}
async fn get_cover(State(s): State<AppState>, Path(track_id): Path<String>) -> Response {
    match s.library.cover(&track_id) {
        Some(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "image/jpeg")],
            bytes,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "pas de pochette"})),
        )
            .into_response(),
    }
}

/// Encodage percent-encoding minimal pour les paramètres de requête.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// GET HTTP bloquant (6 s max) via le fetcher injecté, ou reqwest par défaut.
/// Retourne le corps si la réponse est 2xx, sinon None.
fn http_get(s: &AppState, url: &str) -> Option<String> {
    if let Some(f) = &s.ext_http {
        return f(url);
    }
    let url = url.to_string();
    std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .ok()?
            .get(&url)
            .send()
            .ok()?
            .error_for_status()
            .ok()?
            .text()
            .ok()
    })
    .join()
    .ok()?
}

/// Version async de `http_get` (ne bloque pas l'executor tokio).
async fn http_get_async(s: &AppState, url: &str) -> Option<String> {
    let s = s.clone();
    let url = url.to_string();
    tokio::task::spawn_blocking(move || http_get(&s, &url))
        .await
        .ok()
        .flatten()
}

/// Sonde les métadonnées embarquées d'une piste (probe bloquant délégué).
async fn probe_track_meta(s: &AppState, t: &Track) -> ApiResult<TrackMeta> {
    let reader = s.sources.open_uri(&t.uri).await?;
    let ext = t
        .uri
        .rsplit('.')
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_string();
    // Chemin local si dispo : ffprobe lit le fichier directement (#54).
    let local_path = s.sources.local_path_for_uri(&t.uri);
    tokio::task::spawn_blocking(move || {
        probe_meta_with_path(reader, &ext, local_path).map(|(m, _, _)| m)
    })
    .await
    .map_err(|e| AurisError::other(format!("join sonde: {e}")))?
    .map_err(ApiError)
}

/// GET /api/lyrics/{track_id} — paroles d'une piste.
/// Source 1: tag embarqué (USLT/LYRICS, souvent au format LRC).
/// Source 2: lrclib.net (correspondance exacte puis recherche).
/// Réponse: { track_id, source: "tag"|"lrclib"|"none", plain, synced: [[secs, ligne], ...] }.
async fn get_lyrics(
    State(s): State<AppState>,
    Path(track_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let t = s
        .library
        .track(&track_id)
        .ok_or_else(|| AurisError::TrackNotFound(track_id.clone()))?;
    let meta = probe_track_meta(&s, &t).await?;
    if let Some(raw) = meta.lyrics.filter(|l| !l.trim().is_empty()) {
        return Ok(Json(serde_json::json!({
            "track_id": track_id,
            "source": "tag",
            "plain": lyrics::plain_text(&raw),
            "synced": lyrics::parse_lrc(&raw),
        })));
    }
    let title = meta
        .title
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| t.title.clone());
    let artist = meta
        .artist
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| t.artist.clone());
    let album = meta
        .album
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| t.album.clone());
    let duration = meta
        .duration_secs
        .or(t.duration_secs)
        .map(|d| d.round() as u64)
        .unwrap_or(0);
    // 1) correspondance exacte lrclib
    let url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}&album_name={}&duration={}",
        urlencode(&artist),
        urlencode(&title),
        urlencode(&album),
        duration
    );
    let pick = |v: &serde_json::Value| -> Option<(Option<String>, Option<String>)> {
        let synced = v.get("syncedLyrics").and_then(|x| x.as_str());
        let plain = v.get("plainLyrics").and_then(|x| x.as_str());
        if synced.map(|x| !x.is_empty()).unwrap_or(false)
            || plain.map(|x| !x.is_empty()).unwrap_or(false)
        {
            Some((
                synced.filter(|x| !x.is_empty()).map(str::to_string),
                plain.filter(|x| !x.is_empty()).map(str::to_string),
            ))
        } else {
            None
        }
    };
    let mut hit = http_get_async(&s, &url)
        .await
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| pick(&v));
    // 2) recherche lrclib
    if hit.is_none() {
        let url = format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            urlencode(&title),
            urlencode(&artist)
        );
        hit = http_get_async(&s, &url)
            .await
            .and_then(|body| serde_json::from_str::<Vec<serde_json::Value>>(&body).ok())
            .and_then(|arr| arr.iter().find_map(pick));
    }
    let Some((synced_raw, plain_raw)) = hit else {
        return Ok(Json(serde_json::json!({
            "track_id": track_id,
            "source": "none",
            "plain": serde_json::Value::Null,
            "synced": Vec::<serde_json::Value>::new(),
        })));
    };
    let plain = plain_raw
        .filter(|p| !p.trim().is_empty())
        .or_else(|| synced_raw.as_deref().map(lyrics::plain_text))
        .unwrap_or_default();
    let synced = synced_raw
        .as_deref()
        .map(lyrics::parse_lrc)
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "track_id": track_id,
        "source": "lrclib",
        "plain": plain,
        "synced": synced,
    })))
}

/// GET /api/infos/{track_id} — infos de la lecture en cours: métadonnées de la
/// piste, album, commentaire embarqué et bio Wikipedia de l'artiste (cache sled).
async fn get_infos(
    State(s): State<AppState>,
    Path(track_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let t = s
        .library
        .track(&track_id)
        .ok_or_else(|| AurisError::TrackNotFound(track_id.clone()))?;
    let meta = probe_track_meta(&s, &t).await?;
    let album =
        s.library.albums().into_iter().find(|a| {
            format!("{}|{}", a.artist.to_lowercase(), a.title.to_lowercase()) == t.album_key
        });
    let artist_name = meta
        .album_artist
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| meta.artist.clone().filter(|v| !v.is_empty()))
        .filter(|v| v != "Artiste inconnu")
        .unwrap_or_else(|| t.album_artist.clone());
    // bio Wikipedia: cache sled d'abord, sinon fr puis en
    let (bio, bio_source) = match s.library.bio_get(&t.artist_key) {
        Some(b) => (Some(b), "cache"),
        None => {
            let enc = urlencode(&artist_name);
            let mut found = None;
            for lang in ["fr", "en"] {
                let url = format!("https://{lang}.wikipedia.org/api/rest_v1/page/summary/{enc}");
                if let Some(extract) = http_get_async(&s, &url)
                    .await
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                    .and_then(|v| {
                        v.get("extract")
                            .and_then(|x| x.as_str())
                            .map(str::to_string)
                    })
                    .filter(|x| !x.trim().is_empty())
                {
                    let _ = s.library.bio_set(&t.artist_key, &extract);
                    found = Some(extract);
                    break;
                }
            }
            match found {
                Some(b) => (Some(b), "wikipedia"),
                None => (None, "none"),
            }
        }
    };
    Ok(Json(serde_json::json!({
        "track_id": track_id,
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "album_artist": artist_name,
        "track_number": t.track_number,
        "disc_number": t.disc_number,
        "year": t.year,
        "genre": t.genre,
        "duration_secs": t.duration_secs,
        "format": t.format,
        "comment": meta.comment,
        // crédits étendus (#73), lus au scan et stockés sur la piste
        "composer": t.composer,
        "producer": t.producer,
        "label": t.label,
        "publisher": t.publisher,
        "isrc": t.isrc,
        "copyright": t.copyright,
        "lyrics_embedded": t.lyrics.is_some(),
        "album_info": album.as_ref().map(|a| serde_json::json!({
            "id": a.id,
            "title": a.title,
            "artist": a.artist,
            "year": a.year,
            "track_count": a.track_count,
            "duration_secs": a.duration_secs,
            "label": a.label,
            "publisher": a.publisher,
            "copyright": a.copyright,
            "cover_url": a.cover_track_id.as_ref().map(|c| format!("/api/library/cover/{c}")),
        })),
        "bio": bio,
        "bio_source": bio_source,
        // fiche MusicBrainz (#26) : MBID tagué ou recherche artist+titre (cache 10 min)
        "musicbrainz": if lastfm::mb_enabled(&s) {
            lastfm::mb_cache(&s).ficha(&t).await
        } else {
            serde_json::Value::Null
        },
    })))
}
async fn post_scan(
    State(s): State<AppState>,
    Json(b): Json<ScanBody>,
) -> ApiResult<Json<serde_json::Value>> {
    match b.source_id {
        Some(id) => s.library.scan_source(&id)?,
        None => s.library.scan_all()?,
    }
    Ok(Json(
        serde_json::to_value(s.library.scan_status()).unwrap_or_default(),
    ))
}
async fn get_scan_status(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.library.scan_status()).unwrap_or_default())
}

async fn get_sources(State(s): State<AppState>) -> Json<Vec<Source>> {
    Json(s.sources.list())
}
async fn post_source(
    State(s): State<AppState>,
    Json(src): Json<Source>,
) -> ApiResult<Json<Source>> {
    s.sources.add(src.clone())?;
    s.persist_sources();
    Ok(Json(src))
}
async fn delete_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    s.sources.remove(&id)?;
    s.persist_sources();
    Ok(Json(serde_json::json!({ "removed": id })))
}
async fn put_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(mut src): Json<Source>,
) -> ApiResult<Json<Source>> {
    if src.id != id {
        return Err(
            AurisError::other("l'id du corps doit correspondre a l'URL".to_string()).into(),
        );
    }
    src.id = id;
    s.sources.update(src.clone())?;
    s.persist_sources();
    Ok(Json(src))
}
async fn patch_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ToggleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    s.sources.set_enabled(&id, b.enabled)?;
    s.persist_sources();
    Ok(Json(serde_json::json!({ "id": id, "enabled": b.enabled })))
}
async fn get_browse(
    State(s): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<Vec<auris_core::BrowseEntry>>> {
    let sources = s.sources.clone();
    let uri = q.uri.clone();
    let handle = tokio::runtime::Handle::current();
    let mut entries = offload_io(move || handle.block_on(sources.browse(&uri))).await?;
    // Parcourir n'affiche que les dossiers et les fichiers audio (les autres
    // fichiers restent accessibles via l'export ZIP / la copie, qui passent
    // par walk_all et ne sont pas filtres ici).
    entries.retain(|e| e.is_dir || e.is_audio);
    Ok(Json(entries))
}

/// Parcours recursif: tous les fichiers audio sous un dossier (ou le fichier
/// lui-meme si l'URI pointe sur un fichier audio).
async fn get_browse_tree(
    State(s): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<Vec<BrowseEntry>>> {
    let sources = s.sources.clone();
    let uri = q.uri.clone();
    let handle = tokio::runtime::Handle::current();
    let out = offload_io(move || {
        handle.block_on(async {
            let entry = sources.stat(&uri).await?;
            if entry.is_dir {
                sources.walk_audio(&uri).await
            } else if entry.is_audio {
                Ok(vec![entry])
            } else {
                Ok(Vec::new())
            }
        })
    })
    .await?;
    Ok(Json(out))
}

/// Extensions image reconnues pour la pochette d'un dossier.
const COVER_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// Noms de fichiers pochette prioritaires (cover.jpg, folder.jpg...).
const COVER_NAMES: [&str; 6] = ["cover", "folder", "front", "albumart", "art", "thumb"];

/// Content-Type d'une image, deviné depuis les octets magiques puis l'extension.
fn content_type_for_image(name: &str, bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return "image/png";
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// Pochette d'un dossier (vue Parcourir): fichier image du dossier
/// (cover.jpg, folder.jpg...), sinon pochette embarquee du premier fichier
/// audio. `None` si aucune.
async fn browse_cover(
    sources: &SourcesManager,
    uri: &str,
) -> auris_core::Result<Option<(Vec<u8>, &'static str)>> {
    use std::io::Read as _;
    let entries = sources.browse(uri).await?;
    let files: Vec<&BrowseEntry> = entries.iter().filter(|e| !e.is_dir).collect();
    let ext_of =
        |e: &BrowseEntry| -> String { e.name.rsplit('.').next().unwrap_or("").to_lowercase() };
    let img = COVER_NAMES
        .iter()
        .find_map(|want| {
            files.iter().copied().find(|e| {
                let lower = e.name.to_lowercase();
                let stem = lower.rsplit('.').next().unwrap_or(lower.as_str());
                stem == *want && COVER_EXTS.contains(&ext_of(e).as_str())
            })
        })
        .or_else(|| {
            files
                .iter()
                .copied()
                .find(|e| COVER_EXTS.contains(&ext_of(e).as_str()))
        });
    if let Some(e) = img {
        let r = sources.open_uri(&e.uri).await?;
        let mut buf = Vec::new();
        std::io::Read::take(r, 15 * 1024 * 1024)
            .read_to_end(&mut buf)
            .map_err(|e| AurisError::other(format!("lecture pochette: {e}")))?;
        let ct = content_type_for_image(&e.name, &buf);
        return Ok(Some((buf, ct)));
    }
    let audio = files
        .iter()
        .copied()
        .filter(|e| e.is_audio)
        .min_by_key(|e| e.name.to_lowercase());
    if let Some(e) = audio {
        let ext = ext_of(e);
        let r = sources.open_uri(&e.uri).await?;
        if let Ok((meta, _, _)) = auris_player::probe_meta(r, &ext) {
            if let Some(c) = meta.cover {
                let ct = content_type_for_image(&e.name, &c);
                return Ok(Some((c, ct)));
            }
        }
    }
    Ok(None)
}

async fn get_browse_cover(State(s): State<AppState>, Query(q): Query<BrowseQuery>) -> Response {
    let sources = s.sources.clone();
    let uri = q.uri.clone();
    let handle = tokio::runtime::Handle::current();
    // browse + ouverture du 1er fichier audio + sonde des metadonnees:
    // tout est bloquant -> hors des workers async.
    let res = offload_io(move || handle.block_on(browse_cover(&sources, &uri))).await;
    match res {
        Ok(Some((bytes, ct))) => {
            (StatusCode::OK, [(header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "pas de pochette pour ce dossier"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.0.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct UpnpDiscoverQuery {
    force: Option<bool>,
}

async fn get_upnp_discover(
    State(s): State<AppState>,
    Query(q): Query<UpnpDiscoverQuery>,
) -> Json<serde_json::Value> {
    // force=1 : nouvelle découverte SSDP (bouton « actualiser » de l'UI) ;
    // sinon le cache du manager (TTL 30 s) est utilisé.
    let devs = if q.force.unwrap_or(false) {
        s.sources.refresh_upnp()
    } else {
        s.sources.discover_upnp()
    };
    Json(serde_json::to_value(devs).unwrap_or_default())
}

/// Disques amovibles (USB) detectes par l'OS.
async fn get_usb_devices() -> Json<serde_json::Value> {
    Json(serde_json::to_value(discover_removable()).unwrap_or_default())
}

/// Copie (import) d'un fichier/dossier d'une source vers un dossier d'une
/// autre source (ex: NAS -> cle USB).
async fn post_copy(
    State(s): State<AppState>,
    Json(b): Json<CopyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let report = s.sources.copy(&b.from, &b.to).await?;
    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

/// Construit l'archive ZIP (stored) d'un URI de source: fichier ou dossier.
async fn build_zip_from_uri(sources: &SourcesManager, uri: &str) -> ApiResult<Vec<u8>> {
    let entry = sources.stat(uri).await?;
    let mut z = ZipWriter::new();
    if entry.is_dir {
        // prefixe a retirer pour les noms internes: src://<id>/<dir_rel>/
        let (src_id, dir_rel) = auris_sources::parse_src_uri(uri)
            .ok_or_else(|| AurisError::InvalidUri(uri.to_string()))?;
        let prefix = format!("src://{}/{}/", src_id, dir_rel.trim_matches('/'));
        let files = sources.walk_all(uri).await?;
        for f in files {
            let rel = f.uri.strip_prefix(&prefix).unwrap_or(&f.uri);
            let name = format!("{}/{}", entry.name, rel);
            let reader = sources.open_uri(&f.uri).await?;
            z.add_stream(&name, reader, f.size)
                .map_err(AurisError::Io)?;
        }
    } else {
        let reader = sources.open_uri(uri).await?;
        z.add_stream(&entry.name, reader, entry.size)
            .map_err(AurisError::Io)?;
    }
    Ok(z.finish())
}

/// Nom de fichier ZIP "propre" pour l'en-tete Content-Disposition.
fn zip_download_name(raw: &str) -> String {
    let ascii: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " -_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ascii = ascii.trim_matches('_').to_string();
    let name = if ascii.is_empty() {
        "export".to_string()
    } else {
        ascii
    };
    format!("{name}.zip")
}

fn zip_response(bytes: Vec<u8>, name: &str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\"").as_str(),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// Export ZIP d'un fichier ou dossier d'une source (telechargement).
async fn get_export(
    State(s): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    let bytes = build_zip_from_uri(&s.sources, &q.uri).await?;
    let base = s
        .sources
        .stat(&q.uri)
        .await
        .map(|e| e.name)
        .unwrap_or_else(|_| "export".into());
    let name = q
        .name
        .map(|n| format!("{n}.zip"))
        .unwrap_or_else(|| zip_download_name(&base));
    Ok(zip_response(bytes, &name))
}

// ---------- sauvegarde / restauration de la configuration (#40) ----------

#[derive(Deserialize)]
struct WebdavBody {
    /// URL de base WebDAV (backup) ou URL complete du fichier (restore).
    url: String,
    /// Nom du fichier pour l'envoi WebDAV (defaut: auris-backup-<ts>.zip).
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Construit le ZIP de sauvegarde : manifest + config.toml + donnees
/// utilisateur (favoris, epingles, playlists, historique, partages, reglags).
/// L'index de la bibliotheque (sled `tracks`/`albums`...) n'est PAS inclu :
/// il est reconstitue par un scan.
fn build_backup_zip(s: &AppState) -> ApiResult<Vec<u8>> {
    let jerr = |e: serde_json::Error| AurisError::other(format!("json: {e}"));
    let mut z = ZipWriter::new();
    let manifest = serde_json::json!({
        "app": "auris",
        "version": env!("CARGO_PKG_VERSION"),
        "created_at": chrono::Utc::now().timestamp(),
        "includes": ["config.toml", "favorites", "pins", "playlists", "history", "shares", "settings"],
    });
    z.add_file("manifest.json", manifest.to_string().as_bytes());
    if let Ok(cfg) = std::fs::read(s.data_dir.join("config.toml")) {
        z.add_file("config.toml", &cfg);
    }
    let lib = &s.library;
    z.add_file(
        "data/favorites.json",
        &serde_json::to_vec(&lib.favorites()).map_err(jerr)?,
    );
    z.add_file(
        "data/pins.json",
        &serde_json::to_vec(&lib.pins()).map_err(jerr)?,
    );
    z.add_file(
        "data/playlists.json",
        &serde_json::to_vec(&lib.playlists()).map_err(jerr)?,
    );
    z.add_file(
        "data/history.json",
        &serde_json::to_vec(&lib.history(500)).map_err(jerr)?,
    );
    z.add_file(
        "data/shares.json",
        &serde_json::to_vec(&lib.shares()).map_err(jerr)?,
    );
    let settings = s.settings.read().unwrap_or_else(|e| e.into_inner()).clone();
    z.add_file(
        "data/settings.json",
        &serde_json::to_vec(&settings).map_err(jerr)?,
    );
    Ok(z.finish())
}

/// Restaure un ZIP de sauvegarde produit par [`build_backup_zip`].
fn restore_from_zip(s: &AppState, bytes: &[u8]) -> ApiResult<Json<serde_json::Value>> {
    let entries = read_zip(bytes).map_err(|e| AurisError::other(format!("zip illisible: {e}")))?;
    let n_files = entries.len();
    let jerr = |e: serde_json::Error| AurisError::other(format!("json: {e}"));
    let mut restored = serde_json::Map::new();
    let mut config_restored = false;
    for (name, data) in &entries {
        match name.as_str() {
            "config.toml" => {
                let dst = s.data_dir.join("config.toml");
                if dst.exists() {
                    let _ = std::fs::copy(&dst, s.data_dir.join("config.toml.bak"));
                }
                std::fs::write(&dst, data)
                    .map_err(|e| AurisError::other(format!("ecriture config.toml: {e}")))?;
                config_restored = true;
            }
            "data/favorites.json" => {
                let f: Favorites = serde_json::from_slice(data).map_err(jerr)?;
                s.library.restore_favorites(&f)?;
                restored.insert("favorites".into(), true.into());
            }
            "data/pins.json" => {
                let p: Pins = serde_json::from_slice(data).map_err(jerr)?;
                s.library.restore_pins(&p)?;
                restored.insert("pins".into(), true.into());
            }
            "data/playlists.json" => {
                let ps: Vec<Playlist> = serde_json::from_slice(data).map_err(jerr)?;
                s.library.restore_playlists(&ps)?;
                restored.insert("playlists".into(), ps.len().into());
            }
            "data/history.json" => {
                let h: Vec<HistoryEntry> = serde_json::from_slice(data).map_err(jerr)?;
                s.library.restore_history(&h)?;
                restored.insert("history".into(), h.len().into());
            }
            "data/shares.json" => {
                let ss: Vec<ShareLink> = serde_json::from_slice(data).map_err(jerr)?;
                s.library.restore_shares(&ss)?;
                restored.insert("shares".into(), ss.len().into());
            }
            "data/settings.json" => {
                // les reglages sont appliques en memoire ; la persistance
                // config.toml suivra via le prochain PUT /api/settings.
                let st: Settings = serde_json::from_slice(data).map_err(jerr)?;
                *s.settings.write().unwrap_or_else(|e| e.into_inner()) = st;
                restored.insert("settings".into(), true.into());
            }
            _ => {}
        }
    }
    Ok(Json(serde_json::json!({
        "files": n_files,
        "restored": restored,
        "config_restored": config_restored,
    })))
}

/// `GET /api/settings/backup` — ZIP telechargeable de la configuration + donnees.
async fn get_backup(State(s): State<AppState>) -> ApiResult<Response> {
    let bytes = build_backup_zip(&s)?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    Ok(zip_response(bytes, &format!("auris-backup-{ts}.zip")))
}

/// `POST /api/settings/restore` — body = le ZIP de sauvegarde (application/zip).
async fn post_restore(
    State(s): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    if body.is_empty() {
        return Err(AurisError::other("corps vide (ZIP de sauvegarde attendu)").into());
    }
    restore_from_zip(&s, &body)
}

/// Envoie un ZIP vers un serveur WebDAV (PUT, Basic auth optionnelle).
fn webdav_put(
    url: &str,
    bytes: Vec<u8>,
    user: &Option<String>,
    password: &Option<String>,
) -> Result<(), String> {
    let mut req = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?
        .put(url)
        .body(bytes);
    if let (Some(u), Some(p)) = (user, password) {
        req = req.basic_auth(u, Some(p));
    }
    let r = req.send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("PUT {url} -> {}", r.status()));
    }
    Ok(())
}

/// Recupere un fichier depuis un serveur WebDAV (GET, Basic auth optionnelle).
fn webdav_get(
    url: &str,
    user: &Option<String>,
    password: &Option<String>,
) -> Result<Vec<u8>, String> {
    let mut req = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?
        .get(url);
    if let (Some(u), Some(p)) = (user, password) {
        req = req.basic_auth(u, Some(p));
    }
    let r = req.send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("GET {url} -> {}", r.status()));
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

/// `POST /api/settings/backup/webdav` — pousse la sauvegarde vers un dossier WebDAV.
/// Body: `{url: "https://dav.example.com/auris", name?, user?, password?}`.
async fn post_backup_webdav(
    State(s): State<AppState>,
    Json(b): Json<WebdavBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if b.url.trim().is_empty() {
        return Err(AurisError::other("url WebDAV vide").into());
    }
    let bytes = build_backup_zip(&s)?;
    let len = bytes.len();
    let name = b.name.unwrap_or_else(|| {
        format!(
            "auris-backup-{}.zip",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        )
    });
    let url = format!("{}/{}", b.url.trim_end_matches('/'), name);
    let (user, password) = (b.user.clone(), b.password.clone());
    let pushed = url.clone();
    tokio::task::spawn_blocking(move || webdav_put(&url, bytes, &user, &password))
        .await
        .map_err(|e| AurisError::other(format!("join webdav: {e}")))?
        .map_err(AurisError::other)?;
    Ok(Json(serde_json::json!({"pushed": pushed, "bytes": len})))
}

/// `POST /api/settings/restore/webdav` — telecharge un ZIP depuis WebDAV et restaure.
/// Body: `{url: "https://dav.example.com/auris/auris-backup-xxx.zip", user?, password?}`.
async fn post_restore_webdav(
    State(s): State<AppState>,
    Json(b): Json<WebdavBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if b.url.trim().is_empty() {
        return Err(AurisError::other("url WebDAV vide").into());
    }
    let (user, password) = (b.user.clone(), b.password.clone());
    let url = b.url.clone();
    let bytes = tokio::task::spawn_blocking(move || webdav_get(&url, &user, &password))
        .await
        .map_err(|e| AurisError::other(format!("join webdav: {e}")))?
        .map_err(AurisError::other)?;
    restore_from_zip(&s, &bytes)
}

// ---------- Last.fm / MusicBrainz (#26) ----------

fn lastfm_client_or_err(s: &AppState, need_session: bool) -> ApiResult<lastfm::LastfmClient> {
    let cfg = lastfm::lastfm_cfg(s);
    if cfg.api_key.trim().is_empty() || cfg.shared_secret.trim().is_empty() {
        return Err(AurisError::other(
            "Last.fm non configuré : api_key + shared_secret requis (Réglages → Last.fm)"
                .to_string(),
        )
        .into());
    }
    if need_session && cfg.session_key.trim().is_empty() {
        return Err(AurisError::other(
            "Last.fm : aucune session — faites « 1. Autoriser » puis « 2. Finaliser la session »"
                .to_string(),
        )
        .into());
    }
    Ok(lastfm::LastfmClient::new(cfg))
}

/// `POST /api/lastfm/test` — vérifie la configuration via `user.getInfo`.
async fn post_lastfm_test(State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let c = lastfm_client_or_err(&s, true)?;
    let v = c.user_info().await.map_err(AurisError::other)?;
    let user = v.get("user").cloned().unwrap_or(serde_json::Value::Null);
    Ok(Json(serde_json::json!({
        "ok": true,
        "name": user.get("name").cloned().unwrap_or(serde_json::Value::Null),
        "playcount": user.get("playcount").cloned().unwrap_or(serde_json::Value::Null),
        "url": user.get("url").cloned().unwrap_or(serde_json::Value::Null),
    })))
}

/// `POST /api/lastfm/token` — démarre le flux d'autorisation (auth.getToken).
/// Réponse : `{token, auth_url}` à ouvrir dans un navigateur.
async fn post_lastfm_token(State(s): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let c = lastfm_client_or_err(&s, false)?;
    let token = c.auth_get_token().await.map_err(AurisError::other)?;
    Ok(Json(serde_json::json!({
        "token": token,
        "auth_url": c.auth_url(&token),
    })))
}

#[derive(Deserialize)]
struct LastfmSessionBody {
    token: String,
}

/// `POST /api/lastfm/session` — échange un token autorisé contre une session
/// (`auth.getSession`). L'UI enregistre ensuite `session_key` dans les réglages.
async fn post_lastfm_session(
    State(s): State<AppState>,
    Json(body): Json<LastfmSessionBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let c = lastfm_client_or_err(&s, false)?;
    let (key, name) = c
        .auth_get_session(&body.token)
        .await
        .map_err(AurisError::other)?;
    Ok(Json(serde_json::json!({
        "session_key": key,
        "name": name,
    })))
}

/// Export ZIP d'un album de la bibliotheque (toutes ses pistes).
async fn get_album_export(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (album, tracks) = s
        .library
        .album(&id)
        .ok_or_else(|| AurisError::AlbumNotFound(id.clone()))?;
    let mut z = ZipWriter::new();
    for t in &tracks {
        let file_name = t.uri.rsplit('/').next().unwrap_or(&t.uri).to_string();
        let name = format!("{} - {}/{}", album.artist, album.title, file_name);
        let reader = s.sources.open_uri(&t.uri).await?;
        z.add_stream(&name, reader, t.size_bytes)
            .map_err(AurisError::Io)?;
    }
    let name = zip_download_name(&format!("{} - {}", album.artist, album.title));
    Ok(zip_response(z.finish(), &name))
}

// ---------- listes de lecture ----------

async fn get_playlists(State(s): State<AppState>) -> Json<Vec<serde_json::Value>> {
    // Enrichi avec `track_count` : pour une smart playlist (#98), le nombre est
    // celui évalué dynamiquement (sinon `track_ids.len()`).
    let mut out = Vec::new();
    for p in s.library.playlists() {
        let mut v = serde_json::to_value(&p).unwrap_or_default();
        if let Some(obj) = v.as_object_mut() {
            let n = match &p.rules {
                Some(r) => s.library.eval_smart(r).len(),
                None => p.track_ids.len(),
            };
            obj.insert("track_count".into(), n.into());
        }
        out.push(v);
    }
    Json(out)
}

async fn post_playlist(
    State(s): State<AppState>,
    Json(b): Json<PlaylistBody>,
) -> ApiResult<Json<Playlist>> {
    if let Some(rules) = b.rules {
        let p = s.library.create_smart_playlist(&b.name, rules)?;
        return Ok(Json(p));
    }
    let ids = if let Some(uri) = b.uri {
        // création depuis Parcourir: fichier ou dossier (récursif)
        resolve_uri_tracks(&s, &uri)
            .await?
            .iter()
            .map(|t| t.id.clone())
            .collect()
    } else if let Some(ids) = b.track_ids {
        ids
    } else {
        // sauvegarde de la file d'attente: les pistes hors scan y sont indexees
        let queue = s.engine.state().queue;
        for t in &queue {
            s.library.upsert_track(t)?;
        }
        queue.iter().map(|t| t.id.clone()).collect()
    };
    let p = s.library.create_playlist(&b.name, ids)?;
    Ok(Json(p))
}

/// Resout un URI (fichier ou dossier) en pistes enregistrables: dossier ->
/// tous ses fichiers audio (recursif), fichier -> piste bibliothèque ou
/// piste creee a la volee. Les pistes sont upsertées dans la bibliothèque.
async fn resolve_uri_tracks(s: &AppState, uri: &str) -> ApiResult<Vec<Track>> {
    let mut tracks: Vec<Track> = Vec::new();
    let sources = s.sources.clone();
    let uri_stat = uri.to_string();
    let handle = tokio::runtime::Handle::current();
    let is_dir = offload_io(move || handle.block_on(sources.stat(&uri_stat)).map(|e| e.is_dir))
        .await
        .unwrap_or(false);
    if is_dir {
        tracks = browse_tracks_offload(&s.sources, uri).await?;
    }
    if tracks.is_empty() {
        if let Some(t) = s.library.get_by_uri(uri) {
            tracks = vec![t];
        } else {
            let source_id = auris_sources::parse_src_uri(uri)
                .map(|(id, _)| id)
                .unwrap_or_default();
            let mut t = Track::new(uri.to_string(), source_id);
            t.finalize_keys();
            tracks = vec![t];
        }
    }
    for t in &tracks {
        let _ = s.library.upsert_track(t);
    }
    Ok(tracks)
}

async fn get_playlist(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (p, tracks) = s
        .library
        .playlist(&id)
        .ok_or_else(|| AurisError::other(format!("playlist introuvable: {id}")))?;
    Ok(Json(serde_json::json!({ "playlist": p, "tracks": tracks })))
}

async fn delete_playlist(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if !s.library.delete_playlist(&id)? {
        return Err(AurisError::other(format!("playlist introuvable: {id}")).into());
    }
    Ok(Json(serde_json::json!({ "removed": id })))
}

/// `PATCH /api/playlists/{id}` — renomme et/ou remplace les règles d'une smart
/// playlist (#98). `rules: null` repasse la liste en statique.
async fn patch_playlist(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<PlaylistPatchBody>,
) -> ApiResult<Json<Playlist>> {
    let p = s
        .library
        .update_playlist(&id, b.name.as_deref(), b.rules)?
        .ok_or_else(|| AurisError::other(format!("playlist introuvable: {id}")))?;
    Ok(Json(p))
}

// ---------- liens de partage (v0.6) ----------

#[derive(Deserialize)]
struct ShareBody {
    kind: ShareKind,
    /// ID de la ressource (piste / album / playlist).
    id: String,
    /// Secondes avant expiration ; absent = permanent.
    expires_in_secs: Option<i64>,
}

fn share_json(s: &ShareLink) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    v["url"] = serde_json::json!(format!("/share/{}", s.token));
    v
}

fn share_not_found(token: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": format!("lien de partage introuvable ou expire: {token}") })),
    )
        .into_response()
}

async fn get_shares(State(s): State<AppState>) -> Json<Vec<serde_json::Value>> {
    Json(s.library.shares().iter().map(share_json).collect())
}

async fn post_share(
    State(s): State<AppState>,
    Json(b): Json<ShareBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    // la ressource doit exister ; son titre est fige dans le lien
    let title = match b.kind {
        ShareKind::Track => {
            s.library
                .track(&b.id)
                .ok_or_else(|| AurisError::TrackNotFound(b.id.clone()))?
                .title
        }
        ShareKind::Album => {
            s.library
                .album(&b.id)
                .ok_or_else(|| AurisError::AlbumNotFound(b.id.clone()))?
                .0
                .title
        }
        ShareKind::Playlist => {
            s.library
                .playlist(&b.id)
                .ok_or_else(|| AurisError::other(format!("playlist introuvable: {}", b.id)))?
                .0
                .name
        }
    };
    let share = s
        .library
        .create_share(b.kind, &b.id, &title, b.expires_in_secs)?;
    Ok((StatusCode::CREATED, Json(share_json(&share))))
}

async fn delete_share(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    match s.library.delete_share(&token) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "removed": token })),
        )
            .into_response(),
        Ok(false) => share_not_found(&token),
        Err(e) => ApiError(e).into_response(),
    }
}

/// Pistes resolues d'un lien (piste seule, album ou playlist).
fn share_tracks(s: &AppState, share: &ShareLink) -> Result<Vec<Track>, AurisError> {
    match share.kind {
        ShareKind::Track => Ok(vec![s
            .library
            .track(&share.id)
            .ok_or_else(|| AurisError::TrackNotFound(share.id.clone()))?]),
        ShareKind::Album => Ok(s
            .library
            .album(&share.id)
            .ok_or_else(|| AurisError::AlbumNotFound(share.id.clone()))?
            .1),
        ShareKind::Playlist => Ok(s
            .library
            .playlist(&share.id)
            .ok_or_else(|| AurisError::other(format!("playlist introuvable: {}", share.id)))?
            .1),
    }
}

fn stream_url(share: &ShareLink, track_id: &str) -> String {
    match share.kind {
        ShareKind::Track => format!("/share/{}/stream", share.token),
        _ => format!("/share/{}/stream/{}", share.token, track_id),
    }
}

fn share_payload(s: &AppState, share: &ShareLink) -> Result<serde_json::Value, AurisError> {
    let tracks = share_tracks(s, share)?;
    let items: Vec<serde_json::Value> = tracks
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "title": t.title,
                "artist": t.artist,
                "album": t.album,
                "duration_secs": t.duration_secs,
                "cover_url": if t.has_cover { Some(format!("/api/library/cover/{}", t.id)) } else { None::<String> },
                "stream_url": stream_url(share, &t.id),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": share.kind,
        "title": share.title,
        "created_at": share.created_at,
        "expires_at": share.expires_at,
        "cover_url": tracks.iter().find(|t| t.has_cover).map(|t| format!("/api/library/cover/{}", t.id)),
        "tracks": items,
    }))
}

/// JSON public d'un lien de partage (sans authentification, comme la page HTML).
async fn public_share(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(share) = s.library.share(&token) else {
        return share_not_found(&token);
    };
    match share_payload(&s, &share) {
        Ok(v) => Json(v).into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn audio_content_type(uri: &str) -> &'static str {
    let after_dot = uri.rsplit('.').next().unwrap_or("");
    let ext = after_dot
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "aif" | "aiff" => "audio/aiff",
        "dsf" | "dff" => "audio/dsd",
        _ => "application/octet-stream",
    }
}

/// Lit le fichier audio d'une piste en memoire (morceaux de taille raisonnable).
pub(crate) async fn stream_track_bytes(s: &AppState, t: &Track) -> Result<Vec<u8>, AurisError> {
    let mut reader = s.sources.open_uri(&t.uri).await?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(AurisError::Io)?;
    Ok(buf)
}

fn stream_response(uri: &str, bytes: Vec<u8>) -> Response {
    let ct = audio_content_type(uri);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct),
            (header::CONTENT_LENGTH, bytes.len().to_string().as_str()),
        ],
        bytes,
    )
        .into_response()
}

/// Flux audio d'un lien « piste » : `/share/<token>/stream`.
async fn share_stream(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(share) = s.library.share(&token) else {
        return share_not_found(&token);
    };
    if share.kind != ShareKind::Track {
        return ApiError(AurisError::other(
            "ce lien ne partage pas une piste seule".to_string(),
        ))
        .into_response();
    }
    let Some(t) = s.library.track(&share.id) else {
        return share_not_found(&token);
    };
    match stream_track_bytes(&s, &t).await {
        Ok(bytes) => stream_response(&t.uri, bytes),
        Err(e) => ApiError(e).into_response(),
    }
}

/// Flux audio d'une piste d'un lien « album »/« playlist » : `/share/<token>/stream/<track_id>`.
async fn share_stream_track(
    State(s): State<AppState>,
    Path((token, track_id)): Path<(String, String)>,
) -> Response {
    let Some(share) = s.library.share(&token) else {
        return share_not_found(&token);
    };
    // un lien « piste » n'est servi que par /share/<token>/stream
    if share.kind == ShareKind::Track {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "piste hors du partage" })),
        )
            .into_response();
    }
    let tracks = match share_tracks(&s, &share) {
        Ok(t) => t,
        Err(e) => return ApiError(e).into_response(),
    };
    let Some(t) = tracks.into_iter().find(|t| t.id == track_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "piste hors du partage" })),
        )
            .into_response();
    };
    match stream_track_bytes(&s, &t).await {
        Ok(bytes) => stream_response(&t.uri, bytes),
        Err(e) => ApiError(e).into_response(),
    }
}

/// Mini-page publique façon Nova (sombre, accent vert) pour un lien de partage.
async fn share_page(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let Some(share) = s.library.share(&token) else {
        return share_not_found(&token);
    };
    let payload = match share_payload(&s, &share) {
        Ok(v) => v,
        Err(e) => return ApiError(e).into_response(),
    };
    let title = html_escape(&share.title);
    let kind_label = match share.kind {
        ShareKind::Track => "Piste",
        ShareKind::Album => "Album",
        ShareKind::Playlist => "Playlist",
    };
    let cover = payload["cover_url"]
        .as_str()
        .map(|c| format!("<img class=\"cover\" src=\"{}\" alt=\"\">", html_escape(c)))
        .unwrap_or_default();
    let player = if share.kind == ShareKind::Track {
        let src = format!("/share/{}/stream", share.token);
        format!("<audio controls preload=\"none\" src=\"{src}\"></audio>")
    } else {
        let items: String = payload["tracks"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .map(|(i, t)| {
                        format!(
                            "<li><button data-src=\"{}\">{}. {} — {}</button></li>",
                            html_escape(t["stream_url"].as_str().unwrap_or("")),
                            i + 1,
                            html_escape(t["title"].as_str().unwrap_or("?")),
                            html_escape(t["artist"].as_str().unwrap_or("?")),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        format!(
            "<audio controls preload=\"none\" id=\"a\"></audio><ol class=\"tracks\">{items}</ol>
<script>var a=document.getElementById('a');document.querySelectorAll('.tracks button').forEach(function(b){{b.onclick=function(){{a.src=b.dataset.src;a.play();}};}});</script>"
        )
    };
    let html = format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title} — Auris</title><style>
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0a0a0a;color:#f5f5f5;font-family:-apple-system,'Segoe UI',Roboto,sans-serif}}
.card{{background:#161616;border:1px solid #262626;border-radius:16px;padding:32px;max-width:520px;width:92%;box-shadow:0 12px 40px rgba(0,0,0,.5)}}
.cover{{width:100%;border-radius:12px;margin-bottom:16px}}
h1{{font-size:22px;margin:0 0 4px}}
.kind{{color:#1ED760;font-size:12px;text-transform:uppercase;letter-spacing:.12em;margin-bottom:16px}}
audio{{width:100%;margin:8px 0 16px}}
ol.tracks{{margin:0;padding-left:0;list-style:none;max-height:300px;overflow:auto}}
ol.tracks li{{margin:2px 0}}
ol.tracks button{{width:100%;text-align:left;background:#1f1f1f;color:#f5f5f5;border:1px solid #2a2a2a;border-radius:8px;padding:10px 12px;cursor:pointer;font-size:14px}}
ol.tracks button:hover{{border-color:#1ED760}}
footer{{margin-top:16px;color:#888;font-size:12px;text-align:center}}
</style></head><body><main class=\"card\">{cover}<h1>{title}</h1><div class=\"kind\">{kind_label} partagée avec Auris</div>{player}<footer>Partagé depuis Auris — lecteur audiophile open source (AGPL-3.0)</footer></main></body></html>"
    );
    html_response(html.into_bytes())
}

async fn post_playlist_play(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    let (_, tracks) = s
        .library
        .playlist(&id)
        .ok_or_else(|| AurisError::other(format!("playlist introuvable: {id}")))?;
    if tracks.is_empty() {
        return Err(AurisError::other("playlist vide".to_string()).into());
    }
    s.engine.play_queue(tracks, 0);
    Ok(Json(s.engine.state()))
}

/// Ajoute un URI (fichier ou dossier, joue recursivement) a une playlist.
async fn post_playlist_add(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<QueueAddBody>,
) -> ApiResult<Json<serde_json::Value>> {
    // Resout l'uri en pistes: dossier -> tous ses fichiers audio (recursif),
    // fichier -> la piste (cree/enregistree si hors scan).
    let tracks = resolve_uri_tracks(&s, &b.uri).await?;
    let ids = tracks.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    let p = s
        .library
        .add_to_playlist(&id, ids.clone())
        .map_err(ApiError)?
        .ok_or_else(|| AurisError::other(format!("playlist introuvable: {id}")))?;
    Ok(Json(serde_json::json!({
        "playlist": p,
        "added": ids.len(),
    })))
}

async fn get_devices(State(s): State<AppState>) -> Json<Vec<auris_core::AudioDeviceInfo>> {
    Json(s.list_devices())
}

/// Entrées audio (capture) disponibles — source line-in #75.
async fn get_input_devices() -> Json<serde_json::Value> {
    Json(serde_json::json!(auris_sources::linein::input_devices()
        .into_iter()
        .map(|(name, is_default)| serde_json::json!({ "name": name, "is_default": is_default }))
        .collect::<Vec<_>>()))
}
async fn post_device(
    State(s): State<AppState>,
    Json(b): Json<DeviceBody>,
) -> ApiResult<Json<auris_core::PlayerState>> {
    // Valide le nom contre la liste reelle des peripheriques : un nom inconnu
    // (DAC debranche, faute de frappe) -> 400 avec la liste des noms connus.
    let resolved = auris_player::resolve_device_name(&s.list_devices(), b.name.as_deref())
        .map_err(ApiError)?;
    s.engine.select_device(resolved.clone());
    // Choisir un DAC = lecture locale dessus ; on persiste pour le prochain
    // demarrage (sinon la sortie suivrait le defaut Windows au redemarrage).
    s.persist_player(resolved, OutputMode::Local);
    Ok(Json(s.engine.state()))
}

// ---------- DoP / WASAPI (Windows) : statut + réglage du DAC depuis l'UI ----------

#[derive(Serialize)]
struct DopDeviceStatus {
    name: String,
    /// Mix format live de l'endpoint (ce que le moteur délivre actuellement).
    mix: Option<wasapi_dop::MixFormat>,
    /// Default Format persisté (registre) : sera adopté au prochain
    /// rebranchement du DAC / redémarrage du service audio.
    default_format: Option<wasapi_dop::MixFormat>,
}

#[derive(Serialize)]
struct DopStatus {
    windows: bool,
    current_device: Option<String>,
    devices: Vec<DopDeviceStatus>,
    /// Le device courant accepte-t-il au moins un format en WASAPI exclusif.
    exclusive_supported: bool,
    /// Mode exclusif demandé (config) — appliqué au prochain flux.
    exclusive_enabled: bool,
}

async fn get_dop_status(State(s): State<AppState>) -> Json<DopStatus> {
    let current_device = s.engine.state().audio_device;
    let devices = wasapi_dop::all_mix_formats()
        .into_iter()
        .map(|(name, mix)| DopDeviceStatus {
            default_format: wasapi_dop::device_default_format(Some(&name)),
            mix: Some(mix),
            name,
        })
        .collect();
    let exclusive_supported = wasapi_exclusive::probe_any(current_device.as_deref());
    let exclusive_enabled = s.engine.exclusive_enabled();
    Json(DopStatus {
        windows: cfg!(target_os = "windows"),
        current_device,
        devices,
        exclusive_supported,
        exclusive_enabled,
    })
}

#[derive(Deserialize)]
struct DopSetupBody {
    device: Option<String>,
    rate: u32,
}

#[derive(Serialize)]
struct DopSetupResp {
    ok: bool,
    applied: bool,
    rate: u32,
    device: Option<String>,
    mix: Option<wasapi_dop::MixFormat>,
    message: String,
}

#[derive(Deserialize)]
struct ExclusiveBody {
    enabled: bool,
}

/// Active/désactive le mode WASAPI exclusif (bit-perfect) et le persiste.
async fn post_exclusive(
    State(s): State<AppState>,
    Json(b): Json<ExclusiveBody>,
) -> ApiResult<Json<serde_json::Value>> {
    s.engine.set_exclusive(b.enabled);
    if let Some(sink) = &s.exclusive_sink {
        sink(b.enabled);
    }
    Ok(Json(
        serde_json::json!({ "ok": true, "enabled": b.enabled }),
    ))
}

async fn post_dop_setup(
    State(s): State<AppState>,
    Json(b): Json<DopSetupBody>,
) -> Json<DopSetupResp> {
    let device = b.device.or_else(|| s.engine.state().audio_device);
    let name = device.as_deref();
    match wasapi_dop::ensure_dop_carrier(name, b.rate) {
        Ok(applied) => {
            let mix = wasapi_dop::device_mix_format(name);
            let message = if applied {
                format!(
                    "Default Format écrit en 24 bits / {} Hz — rebranchez le DAC USB ou redémarrez le service Audio Windows pour l'appliquer.",
                    b.rate
                )
            } else {
                format!(
                    "Default Format déjà en 24 bits / {} Hz (mix actuel : {}).",
                    b.rate,
                    mix.map(|m| m.to_string())
                        .unwrap_or_else(|| "inconnu".into())
                )
            };
            Json(DopSetupResp {
                ok: true,
                applied,
                rate: b.rate,
                device,
                mix,
                message,
            })
        }
        Err(e) => Json(DopSetupResp {
            ok: false,
            applied: false,
            rate: b.rate,
            device,
            mix: None,
            message: e,
        }),
    }
}

/// Redémarre le service Audio Windows en élévation (UAC) pour appliquer
/// immédiatement le Default Format persisté, sans rebrancher le DAC.
async fn post_restart_audio() -> Json<serde_json::Value> {
    match wasapi_dop::restart_audio_service() {
        Ok(message) => Json(serde_json::json!({ "ok": true, "message": message })),
        Err(e) => Json(serde_json::json!({ "ok": false, "message": e })),
    }
}

async fn get_system_info(State(s): State<AppState>) -> Json<SystemInfo> {
    let st = s.engine.state();
    Json(SystemInfo {
        name: "Auris",
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        uptime_secs: chrono::Utc::now().timestamp() - s.started_at,
        track_count: s.library.track_count(),
        source_count: s.sources.list().len(),
        audio_device: st.audio_device,
    })
}

async fn get_update(State(s): State<AppState>) -> Json<serde_json::Value> {
    if let Some(i) = s.updater.last_check() {
        return Json(serde_json::to_value(i).unwrap_or_default());
    }
    let updater = s.updater.clone();
    let res = tokio::task::spawn_blocking(move || updater.check()).await;
    match res {
        Ok(Ok(i)) => Json(serde_json::to_value(i).unwrap_or_default()),
        Ok(Err(e)) => Json(serde_json::json!({ "error": e.to_string() })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn post_update(State(s): State<AppState>) -> Json<serde_json::Value> {
    let updater = s.updater.clone();
    let res = tokio::task::spawn_blocking(move || updater.update()).await;
    match res {
        Ok(Ok(i)) => {
            Json(serde_json::json!({ "updated": true, "info": i, "restart_required": true }))
        }
        Ok(Err(e)) => Json(serde_json::json!({ "updated": false, "error": e.to_string() })),
        Err(e) => Json(serde_json::json!({ "updated": false, "error": e.to_string() })),
    }
}

// ---------- Réglages (menu Réglages façon Nova) ----------

async fn get_settings(State(s): State<AppState>) -> Json<Settings> {
    // le token d'accès n'est jamais exposé en clair (#97)
    Json(auth::mask_token_out(s.settings.read().unwrap().clone()))
}

/// État des alarmes (minuterie de sommeil / réveil) pour l'UI.
#[derive(Serialize)]
struct AlarmsInfo {
    /// Réglage courant de la minuterie (minutes), si active.
    sleep_timer: Option<u32>,
    /// Secondes restantes avant la pause automatique, si armée.
    sleep_remaining_secs: Option<i64>,
    /// Heure de réveil « HH:MM », si active.
    wake_time: Option<String>,
}

async fn get_alarms(State(s): State<AppState>) -> Json<AlarmsInfo> {
    let (timer, wake) = {
        let r = s.settings.read().unwrap();
        (r.playback.sleep_timer, r.playback.wake_time.clone())
    };
    let now = chrono::Utc::now().timestamp();
    let remaining = s
        .alarms
        .lock()
        .unwrap()
        .sleep_deadline()
        .map(|d| (d - now).max(0));
    Json(AlarmsInfo {
        sleep_timer: timer,
        sleep_remaining_secs: remaining,
        wake_time: wake,
    })
}

async fn put_settings(
    State(s): State<AppState>,
    Json(new): Json<Settings>,
) -> ApiResult<Json<Settings>> {
    // token d'accès (#97) : "********" = inchangé, "" = suppression
    let old_token = s.settings.read().unwrap().system.access_token.clone();
    let mut new = auth::normalize_token_in(new, old_token);
    if let Err(e) = new.validate() {
        return Err(ApiError(AurisError::other(format!(
            "réglages invalides: {e}"
        ))));
    }
    new.system.device_name = new.system.device_name.trim().to_string();
    // le pont CamillaDSP (#82) ne change que si la section a changé (sinon on
    // resonderait l'instance à chaque sauvegarde de réglages)
    let camilla_changed = {
        let old = s.settings.read().unwrap();
        old.camilla != new.camilla
    };
    {
        let mut w = s.settings.write().unwrap();
        *w = new.clone();
    }
    // applique à chaud au moteur (volume max, courbe, mode bit-perfect)
    s.engine.apply_playback(&new.playback);
    if camilla_changed {
        s.engine.apply_camilla(&new.camilla);
    }
    // le cache MusicBrainz dépend des réglages (api_url) : on l'invalide (#26)
    lastfm::mb_cache_reset(&s);
    s.persist_settings();
    Ok(Json(new))
}

// ---------- Système avancé ----------

#[derive(Serialize)]
struct StorageInfo {
    data_dir: String,
    used_bytes: u64,
    total_bytes: u64,
    free_bytes: u64,
}

async fn get_storage(State(s): State<AppState>) -> Json<StorageInfo> {
    let used: u64 = walkdir::WalkDir::new(&s.data_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    Json(StorageInfo {
        data_dir: s.data_dir.display().to_string(),
        used_bytes: used,
        total_bytes: fs2::total_space(&s.data_dir).unwrap_or(0),
        free_bytes: fs2::available_space(&s.data_dir).unwrap_or(0),
    })
}

/// Retire les séquences d'échappement ANSI (`ESC [ ... m`) d'un texte de log.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // saute jusqu'à la lettre finale de la séquence CSI
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

async fn get_logs(State(s): State<AppState>) -> Response {
    match &s.log_path {
        Some(p) if p.exists() => {
            let content = std::fs::read_to_string(p).unwrap_or_default();
            // retire les séquences ANSI (couleurs) pour un affichage propre dans l'UI
            let clean = strip_ansi(&content);
            let tail: Vec<&str> = clean.lines().rev().take(400).collect();
            let body = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "journalisation fichier inactive" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ResetBody {
    confirm: String,
}

/// Réinitialisation usine : supprime la bibliothèque (pistes + playlists sled),
/// remet les sources et les réglages à zéro. La config réseau (port/host) est
/// conservée. Un redémarrage est conseillé ensuite.
async fn post_reset(
    State(s): State<AppState>,
    Json(b): Json<ResetBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if b.confirm != "RESET" {
        return Err(ApiError(AurisError::other(
            "confirmation requise: envoyer {\"confirm\": \"RESET\"}",
        )));
    }
    let lib = s.data_dir.join("library");
    if lib.exists() {
        std::fs::remove_dir_all(&lib)
            .map_err(|e| ApiError(AurisError::other(format!("reset: {e}"))))?;
    }
    // sources vidées + réglages par défaut, persistés via les sinks
    if let Some(sink) = &s.sources_sink {
        sink(Vec::new());
    }
    {
        let mut w = s.settings.write().unwrap();
        *w = Settings::default();
    }
    s.persist_settings();
    s.engine.apply_playback(&Settings::default().playback);
    Ok(Json(serde_json::json!({
        "ok": true,
        "restart_recommended": true,
        "note": "bibliothèque, playlists, liens de partage, sources et réglages remis à zéro"
    })))
}

type PowerResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

async fn post_reboot(State(s): State<AppState>) -> PowerResult {
    power(s, PowerAction::Reboot).await
}

async fn post_shutdown(State(s): State<AppState>) -> PowerResult {
    power(s, PowerAction::Shutdown).await
}

async fn power(s: AppState, action: PowerAction) -> PowerResult {
    if !s.power_control {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "redémarrer/éteindre est désactivé : passer `power_control = true` dans config.toml"
            })),
        ));
    }
    if let Some(ops) = &s.power_ops {
        ops(action);
    } else {
        real_power(action);
    }
    let name = match action {
        PowerAction::Reboot => "reboot",
        PowerAction::Shutdown => "shutdown",
    };
    Ok(Json(serde_json::json!({ "ok": true, "action": name })))
}

/// Lance la commande d'alimentation réelle de l'OS (détaché).
fn real_power(action: PowerAction) {
    #[cfg(unix)]
    {
        let (prog, args): (&str, &[&str]) = match action {
            PowerAction::Reboot => ("reboot", &[]),
            PowerAction::Shutdown => ("shutdown", &["-h", "now"]),
        };
        let _ = std::process::Command::new(prog).args(args).spawn();
    }
    #[cfg(windows)]
    {
        let args: &[&str] = match action {
            PowerAction::Reboot => &["/r", "/t", "0"],
            PowerAction::Shutdown => &["/s", "/t", "0"],
        };
        let _ = std::process::Command::new("shutdown").args(args).spawn();
    }
}

// ---------- « Jouer ici » : sortie navigateur ----------

#[derive(Deserialize)]
struct OutputBody {
    mode: OutputMode,
}

async fn post_output(
    State(s): State<AppState>,
    Json(b): Json<OutputBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.set_output_mode(b.mode);
    let device = s.engine.state().audio_device;
    s.persist_player(device, b.mode);
    Json(s.engine.state())
}

async fn ws_audio_handler(ws: WebSocketUpgrade, State(s): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_ws_audio(socket, s))
}

/// Flux binaire « Jouer ici » : 1er trame texte = spec JSON, puis PCM f32 LE.
async fn handle_ws_audio(socket: axum::extract::ws::WebSocket, s: AppState) {
    use axum::extract::ws::Message;
    let (mut tx, mut rx) = socket.split();
    let mut frames = s.engine.browser_tx.subscribe();
    // état courant au branchement
    let st = s.engine.state();
    let hello = serde_json::json!({
        "type": "audio_mode",
        "mode": st.output_mode,
        "status": st.status,
    });
    let _ = tx.send(Message::Text(hello.to_string().into())).await;
    if let Some((rate, channels)) = s.engine.browser_spec() {
        let spec =
            serde_json::json!({ "type": "audio_spec", "sample_rate": rate, "channels": channels });
        let _ = tx.send(Message::Text(spec.to_string().into())).await;
    }
    let send_task = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(auris_core::BrowserFrame::Spec { rate, channels }) => {
                    let m = serde_json::json!({ "type": "audio_spec", "sample_rate": rate, "channels": channels });
                    if tx.send(Message::Text(m.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Ok(auris_core::BrowserFrame::Pcm(samples)) => {
                    let mut bytes = Vec::with_capacity(samples.len() * 4);
                    for s in samples.iter() {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    if tx.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let m = serde_json::json!({ "type": "audio_lag", "skipped": n });
                    let _ = tx.send(Message::Text(m.to_string().into())).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    while let Some(msg) = rx.next().await {
        match msg {
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    send_task.abort();
}

// ---------- WebSocket ----------

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, s))
}

async fn handle_ws(socket: axum::extract::ws::WebSocket, s: AppState) {
    let (mut tx, mut rx) = socket.split();
    let mut events = s.engine.events.subscribe();
    // envoi l'etat courant immediatement
    let initial = WsEvent::state(&s.engine.state());
    let _ = tx
        .send(axum::extract::ws::Message::Text(
            serde_json::to_string(&initial).unwrap_or_default().into(),
        ))
        .await;
    let send_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ev) => {
                    let msg = axum::extract::ws::Message::Text(
                        serde_json::to_string(&ev).unwrap_or_default().into(),
                    );
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    // lectures (ping/pong + fermetures); on ignore les commandes entrantes pour l'instant
    while let Some(msg) = rx.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    send_task.abort();
}

// ---------- UI embarquee ----------

async fn index_html() -> Response {
    match UiAssets::get("index.html") {
        Some(f) => html_response(f.data.into_owned()),
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<html><body><h1>Auris</h1><p>UI non compilee: lancez <code>npm run build</code> dans ui/ puis recompilez.</p></body></html>",
        )
            .into_response(),
    }
}

async fn static_asset(Path(path): Path<String>, uri: Uri) -> Response {
    let p = path.trim_start_matches('/');
    if let Some(f) = UiAssets::get(p) {
        let mime = mime_guess::from_path(p).first_or_octet_stream().to_string();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_str())],
            f.data.into_owned(),
        )
            .into_response();
    }
    // fallback SPA
    if !p.contains('.') {
        if let Some(f) = UiAssets::get("index.html") {
            return html_response(f.data.into_owned());
        }
    }
    let _ = uri;
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn html_response(bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        bytes,
    )
        .into_response()
}
