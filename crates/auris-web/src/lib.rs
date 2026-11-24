//! Serveur web Auris: API REST + WebSocket temps reel + UI embarquee (rust-embed).
//!
//! L'UI est un build Vite (ui/dist) embarque dans le binaire: le binaire reste
//! autonome et clé en main, aucune installation фронт separee n'est requise.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::{
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
    AurisError, BrowseEntry, OutputMode, Playlist, RepeatMode, Settings, Source, Track, WsEvent,
    ZipWriter,
};
use auris_library::Library;
use auris_player::{AudioOutput, PlayerEngine};
use auris_sources::{discover_removable, SourcesManager};
use auris_updater::Updater;

#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
struct UiAssets;

/// Sink de persistance des réglages (config.toml).
pub type SettingsSink = Arc<dyn Fn(&Settings) + Send + Sync>;

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
    /// Répertoire de données (stockage, logs, reset usine).
    pub data_dir: PathBuf,
    /// Autorise redémarrer/éteindre depuis l'UI (config `power_control`).
    pub power_control: bool,
    /// Exécute une action d'alimentation. `None` = commande système réelle.
    pub power_ops: Option<Arc<dyn Fn(PowerAction) + Send + Sync>>,
    /// Fichier de logs si la journalisation fichier est active.
    pub log_path: Option<PathBuf>,
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
            data_dir,
            power_control: false,
            power_ops: None,
            log_path: None,
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
}

/// Erreur API -> JSON {"error": ...}.
struct ApiError(AurisError);
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
type ApiResult<T> = Result<T, ApiError>;

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
    let api = Router::new()
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
        .route("/api/library/albums", get(get_albums))
        .route("/api/library/albums/{id}", get(get_album))
        .route("/api/library/artists", get(get_artists))
        .route("/api/library/artists/{id}", get(get_artist))
        .route("/api/library/search", get(get_search))
        .route("/api/library/cover/{track_id}", get(get_cover))
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
            get(get_playlist).delete(delete_playlist),
        )
        .route("/api/playlists/{id}/play", post(post_playlist_play))
        // audio
        .route("/api/audio/devices", get(get_devices))
        .route("/api/audio/device", post(post_device))
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
        // websocket
        .route("/ws", get(ws_handler))
        .route("/ws/audio", get(ws_audio_handler))
        .route("/api/player/output", post(post_output));

    Router::new()
        .merge(api)
        .route("/", get(index_html))
        .route("/{*path}", get(static_asset))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
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
        s.engine.play_queue(queue, idx);
    } else if let Some(track_id) = body.track_id {
        let t = s
            .library
            .track(&track_id)
            .ok_or_else(|| AurisError::TrackNotFound(track_id.clone()))?;
        s.engine.play_uri(t.uri);
    } else if let Some(uri) = body.uri {
        // un dossier est joue en "tout lire": file remplie de ses fichiers audio
        if let Ok(entry) = s.sources.stat(&uri).await {
            if entry.is_dir {
                let tracks = browse_tracks(&s.sources, &uri).await?;
                if tracks.is_empty() {
                    return Err(AurisError::other("dossier sans fichier audio".to_string()).into());
                }
                s.engine.play_queue(tracks, 0);
                return Ok(Json(s.engine.state()));
            }
        }
        s.engine.play_uri(uri);
    } else {
        return Err(AurisError::InvalidUri("uri, track_id ou queue requis".into()).into());
    }
    Ok(Json(s.engine.state()))
}

/// Construit des pistes jouables a partir des fichiers audio d'un dossier.
async fn browse_tracks(sources: &SourcesManager, dir_uri: &str) -> ApiResult<Vec<Track>> {
    let entries = sources.walk_audio(dir_uri).await?;
    let mut out = Vec::new();
    for e in entries {
        let source_id = auris_sources::parse_src_uri(&e.uri)
            .map(|(id, _)| id)
            .unwrap_or_default();
        let mut t = Track::new(e.uri.clone(), source_id);
        t.title = e.name.clone();
        t.size_bytes = e.size;
        t.finalize_keys();
        out.push(t);
    }
    Ok(out)
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
    if let Ok(entry) = s.sources.stat(&b.uri).await {
        if entry.is_dir {
            for t in browse_tracks(&s.sources, &b.uri).await? {
                s.engine.enqueue(t);
            }
            return Ok(Json(s.engine.state()));
        }
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
async fn delete_queue_item(
    State(s): State<AppState>,
    Path(i): Path<usize>,
) -> Json<auris_core::PlayerState> {
    s.engine.remove_queue_item(i);
    Json(s.engine.state())
}

async fn get_tracks(State(s): State<AppState>) -> Json<Vec<Track>> {
    Json(s.library.tracks())
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
    Ok(Json(
        serde_json::json!({ "artist": artist, "albums": albums }),
    ))
}
async fn get_search(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Vec<Track>> {
    let query = q.get("q").cloned().unwrap_or_default();
    Json(s.library.search(&query))
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
    let entries = s.sources.browse(&q.uri).await?;
    Ok(Json(entries))
}

/// Parcours recursif: tous les fichiers audio sous un dossier (ou le fichier
/// lui-meme si l'URI pointe sur un fichier audio).
async fn get_browse_tree(
    State(s): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<Vec<BrowseEntry>>> {
    let entry = s.sources.stat(&q.uri).await?;
    let out = if entry.is_dir {
        s.sources.walk_audio(&q.uri).await?
    } else if entry.is_audio {
        vec![entry]
    } else {
        Vec::new()
    };
    Ok(Json(out))
}

async fn get_upnp_discover(State(s): State<AppState>) -> Json<serde_json::Value> {
    let devs = s.sources.discover_upnp();
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

async fn get_playlists(State(s): State<AppState>) -> Json<Vec<Playlist>> {
    Json(s.library.playlists())
}

async fn post_playlist(
    State(s): State<AppState>,
    Json(b): Json<PlaylistBody>,
) -> ApiResult<Json<Playlist>> {
    let ids = match b.track_ids {
        Some(ids) => ids,
        None => {
            // sauvegarde de la file d'attente: les pistes hors scan y sont indexees
            let queue = s.engine.state().queue;
            for t in &queue {
                s.library.upsert_track(t)?;
            }
            queue.iter().map(|t| t.id.clone()).collect()
        }
    };
    let p = s.library.create_playlist(&b.name, ids)?;
    Ok(Json(p))
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

async fn get_devices() -> Json<Vec<auris_core::AudioDeviceInfo>> {
    Json(AudioOutput::devices())
}
async fn post_device(
    State(s): State<AppState>,
    Json(b): Json<DeviceBody>,
) -> Json<auris_core::PlayerState> {
    s.engine.select_device(b.name);
    Json(s.engine.state())
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
    Json(s.settings.read().unwrap().clone())
}

async fn put_settings(
    State(s): State<AppState>,
    Json(mut new): Json<Settings>,
) -> ApiResult<Json<Settings>> {
    if let Err(e) = new.validate() {
        return Err(ApiError(AurisError::other(format!(
            "réglages invalides: {e}"
        ))));
    }
    new.system.device_name = new.system.device_name.trim().to_string();
    {
        let mut w = s.settings.write().unwrap();
        *w = new.clone();
    }
    // applique à chaud au moteur (volume max, courbe, mode bit-perfect)
    s.engine.apply_playback(&new.playback);
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
        "note": "bibliothèque, playlists, sources et réglages remis à zéro"
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
