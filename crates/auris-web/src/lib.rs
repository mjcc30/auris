//! Serveur web Auris: API REST + WebSocket temps reel + UI embarquee (rust-embed).
//!
//! L'UI est un build Vite (ui/dist) embarque dans le binaire: le binaire reste
//! autonome et clé en main, aucune installation фронт separee n'est requise.

use std::net::SocketAddr;
use std::sync::Arc;

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

use auris_core::{AurisError, RepeatMode, Source, Track, WsEvent};
use auris_library::Library;
use auris_player::{AudioOutput, PlayerEngine};
use auris_sources::SourcesManager;
use auris_updater::Updater;

#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
struct UiAssets;

/// Etat partagé de l'application web.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<PlayerEngine>,
    pub library: Arc<Library>,
    pub sources: Arc<SourcesManager>,
    pub updater: Arc<Updater>,
    pub started_at: i64,
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
        .route("/api/sources/{id}", delete(delete_source))
        .route("/api/browse", get(get_browse))
        .route("/api/upnp/discover", get(get_upnp_discover))
        // audio
        .route("/api/audio/devices", get(get_devices))
        .route("/api/audio/device", post(post_device))
        // systeme
        .route("/api/system/info", get(get_system_info))
        .route("/api/system/update", get(get_update).post(post_update))
        // websocket
        .route("/ws", get(ws_handler));

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
        s.engine.play_uri(uri);
    } else {
        return Err(AurisError::InvalidUri("uri, track_id ou queue requis".into()).into());
    }
    Ok(Json(s.engine.state()))
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
    Ok(Json(src))
}
async fn delete_source(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    s.sources.remove(&id)?;
    Ok(Json(serde_json::json!({ "removed": id })))
}
async fn get_browse(
    State(s): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<Vec<auris_core::BrowseEntry>>> {
    let entries = s.sources.browse(&q.uri).await?;
    Ok(Json(entries))
}
async fn get_upnp_discover(State(s): State<AppState>) -> Json<serde_json::Value> {
    let devs = s.sources.discover_upnp();
    Json(serde_json::to_value(devs).unwrap_or_default())
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
            initial.payload.to_string().into(),
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
