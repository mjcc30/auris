//! API REST v1 façon Volumio (#76) — couche de compatibilité pour les clients
//! tiers (apps mobiles Volumio, scripts, intégrations domotique) :
//!
//! - `GET  /api/v1/ping`
//! - `GET  /api/v1/getState`
//! - `GET  /api/v1/getQueue`
//! - `GET|POST /api/v1/commands?cmd=play|pause|stop|next|previous|seek|volume|toggleMute[&value=…]`
//!   (`seek` en **millisecondes**, comme Volumio ; `volume` 0–100 ou `mute`/`unmute`)
//! - `GET|POST /api/v1/replaceAndPlay?uri=…` (POST JSON `{"uri": "…", "name": "…"}`)
//! - `GET  /api/v1/addToQueue?uri=…`
//! - `GET  /api/v1/getBrowseSources`
//! - `GET  /api/v1/browse?uri=…`
//! - `GET  /api/v1/search?q=…[&type=tracks|albums|artists|playlists]`
//!
//! Les réponses suivent la forme Volumio (champs `status`, `title`, `artist`,
//! `uri`, `seek` en ms, `samplerate`, `bitdepth`…) sans viser l'identique
//! bit à bit.

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use auris_core::{BrowseEntry, PlayStatus, Track};

use crate::{offload_io, ApiResult, AppState};

/// Attend que l'état du moteur satisfasse `pred` (la lecture démarre en
/// asynchrone : sans attente, un client qui lit la réponse aussitôt verrait
/// encore l'ancien état — ex. `replaceAndPlay` renverrait `"status":"stop"`).
async fn settle(
    s: &AppState,
    pred: impl Fn(&auris_core::PlayerState) -> bool,
    tries: u32,
) -> auris_core::PlayerState {
    for _ in 0..tries {
        let st = s.engine.state();
        if pred(&st) {
            return st;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    s.engine.state()
}

/// Piste au format Volumio v1.
fn track_v1(t: &Track) -> Value {
    json!({
        "id": t.id,
        "type": "song",
        "service": "auris",
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "albumartist": t.album_artist,
        "uri": t.uri,
        "duration": t.duration_secs.map(|d| d.round() as u64).unwrap_or(0),
        "trackNumber": t.track_number,
        "discNumber": t.disc_number,
        "year": t.year,
        "genre": t.genre,
        "trackType": match t.format {
            Some(auris_core::AudioFormat::Dsd { .. }) => "dsd",
            _ => "pcm",
        },
        "samplerate": t.format.map(|f| match f {
            auris_core::AudioFormat::Pcm { sample_rate, .. } => sample_rate.to_string(),
            auris_core::AudioFormat::Dsd { sample_rate, .. } => sample_rate.to_string(),
        }),
        "bitdepth": t.format.map(|f| match f {
            auris_core::AudioFormat::Pcm { bits, .. } => bits.to_string(),
            auris_core::AudioFormat::Dsd { .. } => "dsd".to_string(),
        }),
        "channels": t.format.map(|f| match f {
            auris_core::AudioFormat::Pcm { channels, .. } => channels,
            auris_core::AudioFormat::Dsd { channels, .. } => channels,
        }),
        "albumart": if t.has_cover {
            format!("/api/library/cover/{}", t.id)
        } else {
            String::new()
        },
    })
}

/// État au format Volumio v1 (`seek` en millisecondes).
fn state_v1(s: &auris_core::PlayerState) -> Value {
    let cur = s.current.as_ref();
    json!({
        "status": match s.status {
            PlayStatus::Playing => "play",
            PlayStatus::Paused => "pause",
            PlayStatus::Stopped => "stop",
        },
        "title": cur.map(|t| t.title.clone()).unwrap_or_default(),
        "artist": cur.map(|t| t.artist.clone()).unwrap_or_default(),
        "album": cur.map(|t| t.album.clone()).unwrap_or_default(),
        "albumart": cur
            .filter(|t| t.has_cover)
            .map(|t| format!("/api/library/cover/{}", t.id))
            .unwrap_or_default(),
        "uri": cur.map(|t| t.uri.clone()).unwrap_or_default(),
        "service": "auris",
        "trackType": cur.and_then(|t| t.format).map(|f| match f {
            auris_core::AudioFormat::Dsd { .. } => "dsd",
            _ => "pcm",
        }).unwrap_or(""),
        "samplerate": cur.and_then(|t| t.format).map(|f| match f {
            auris_core::AudioFormat::Pcm { sample_rate, .. } => sample_rate,
            auris_core::AudioFormat::Dsd { sample_rate, .. } => sample_rate,
        }).map(|r| r.to_string()).unwrap_or_default(),
        "bitdepth": cur.and_then(|t| t.format).map(|f| match f {
            auris_core::AudioFormat::Pcm { bits, .. } => bits.to_string(),
            auris_core::AudioFormat::Dsd { .. } => "dsd".to_string(),
        }).unwrap_or_default(),
        "channels": cur.and_then(|t| t.format).map(|f| match f {
            auris_core::AudioFormat::Pcm { channels, .. } => channels,
            auris_core::AudioFormat::Dsd { channels, .. } => channels,
        }),
        "duration": if s.duration_secs > 0.0 { s.duration_secs.round() as u64 } else { 0 },
        "seek": (s.position_secs * 1000.0).round() as u64,
        "position": s.position_secs.round() as u64,
        "volume": s.volume,
        "mute": s.muted,
        "repeat": match s.repeat {
            auris_core::RepeatMode::Off => 0,
            auris_core::RepeatMode::All => 1,
            auris_core::RepeatMode::One => 2,
        },
        "random": s.shuffle,
        "device": s.audio_device,
        "bitrate": "",
    })
}

async fn ping() -> Json<Value> {
    Json(json!({ "status": "OK", "service": "auris", "version": env!("CARGO_PKG_VERSION") }))
}

async fn get_state_v1(State(s): State<AppState>) -> Json<Value> {
    Json(state_v1(&s.engine.state()))
}

async fn get_queue_v1(State(s): State<AppState>) -> Json<Value> {
    let st = s.engine.state();
    Json(json!({
        "queue": st.queue.iter().map(track_v1).collect::<Vec<_>>(),
        "index": st.queue_index,
    }))
}

/// `GET|POST /api/v1/commands?cmd=…&value=…` — commandes Volumio.
async fn commands(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let cmd = q.get("cmd").cloned().unwrap_or_default();
    let value = q.get("value").cloned().unwrap_or_default();
    // `Some(status)` : attendre ce statut ; `Seek(ms)` : attendre cette position.
    enum Target {
        Status(PlayStatus),
        Position(f64),
    }
    let target = match cmd.as_str() {
        "play" => {
            s.engine.resume();
            Some(Target::Status(PlayStatus::Playing))
        }
        "pause" => {
            s.engine.pause();
            Some(Target::Status(PlayStatus::Paused))
        }
        "stop" => {
            s.engine.stop();
            Some(Target::Status(PlayStatus::Stopped))
        }
        "next" => {
            s.engine.next();
            None
        }
        "previous" | "prev" => {
            s.engine.prev();
            None
        }
        "seek" => {
            // Volumio envoie des millisecondes.
            let ms: f64 = value.parse().unwrap_or(0.0);
            s.engine.seek(ms / 1000.0);
            Some(Target::Position(ms))
        }
        "volume" => {
            if value == "mute" {
                s.engine.set_muted(true);
            } else if value == "unmute" {
                s.engine.set_muted(false);
            } else if let Ok(v) = value.parse::<u8>() {
                s.engine.set_volume(v);
            }
            None
        }
        "toggleMute" => {
            let st = s.engine.state();
            s.engine.set_muted(!st.muted);
            None
        }
        other => {
            return Err(crate::ApiError(auris_core::AurisError::other(format!(
                "commande v1 inconnue: {other}"
            ))))
        }
    };
    let st = match target {
        Some(Target::Status(want)) => settle(&s, |st| st.status == want, 30).await,
        Some(Target::Position(ms)) => {
            // position lue en secondes ; marge de 100 ms pour l'arrondi
            settle(&s, |st| (st.position_secs * 1000.0) >= ms - 100.0, 30).await
        }
        None => s.engine.state(),
    };
    Ok(Json(state_v1(&st)))
}

/// `GET /api/v1/replaceAndPlay?uri=…` ou POST `{"uri": "…"}`.
async fn replace_and_play(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    body: Option<Json<Value>>,
) -> Json<Value> {
    let uri = body
        .and_then(|Json(b)| b.get("uri").and_then(|u| u.as_str()).map(|s| s.to_string()))
        .or_else(|| q.get("uri").cloned())
        .unwrap_or_default();
    if !uri.is_empty() {
        s.engine.play_uri(uri.clone());
        // attend que la piste soit effectivement en cours (démarrage async)
        let st = settle(
            &s,
            |st| {
                st.status != PlayStatus::Stopped
                    && st.current.as_ref().map(|t| t.uri == uri).unwrap_or(false)
            },
            40,
        )
        .await;
        return Json(state_v1(&st));
    }
    Json(state_v1(&s.engine.state()))
}

/// `GET /api/v1/addToQueue?uri=…` — ajoute la piste/URI à la file sans lancer.
async fn add_to_queue(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let uri = q
        .get("uri")
        .cloned()
        .ok_or_else(|| crate::ApiError(auris_core::AurisError::other("paramètre uri requis")))?;
    // Résout l'URI en entrée de piste (stat direct si src://, URL http telle quelle).
    let entry = match auris_sources::resolve_uri(&uri) {
        Ok(auris_sources::UriRef::Src { source_id, path }) => {
            let sources = s.sources.clone();
            let handle = tokio::runtime::Handle::current();
            offload_io(move || {
                handle.block_on(async move {
                    let b = sources
                        .backend(&source_id)
                        .ok_or_else(|| auris_core::AurisError::SourceNotFound(source_id.clone()))?;
                    b.stat(&path).await
                })
            })
            .await?
        }
        Ok(auris_sources::UriRef::Http(_)) => None, // webradio: piste directe ci-dessous
        Err(e) => return Err(crate::ApiError(e)),
    };
    let mut track = Track::new(uri.clone(), String::new());
    if let Some(e) = entry {
        if !e.is_audio {
            return Err(crate::ApiError(auris_core::AurisError::other(
                "uri non résolvable en piste audio (dossier ?)",
            )));
        }
        track.title = e.name;
        track.format = e.format;
        track.size_bytes = e.size;
        if let Ok(auris_sources::UriRef::Src { source_id, .. }) = auris_sources::resolve_uri(&uri) {
            track.source_id = source_id;
        }
    }
    // enfile sans relancer la lecture en cours (commande async : attendre
    // que la file reflète l'ajout avant de répondre)
    s.engine.enqueue(track);
    let st = settle(&s, |st| st.queue.iter().any(|t| t.uri == uri), 30).await;
    Ok(Json(json!({
        "queue": st.queue.iter().map(track_v1).collect::<Vec<_>>(),
        "index": st.queue_index,
    })))
}

/// `GET /api/v1/getBrowseSources` — liste des sources façon Volumio.
async fn get_browse_sources(State(s): State<AppState>) -> Json<Value> {
    let sources = s.sources.list();
    Json(json!({
        "navigation": {
            "lists": [{
                "title": "Sources",
                "available": true,
                "items": sources.iter().map(|src| json!({
                    "type": "source",
                    "title": src.name,
                    "uri": format!("src://{}", src.id),
                    "icon": "folder",
                    "service": "auris",
                    "enabled": src.enabled,
                })).collect::<Vec<_>>(),
            }]
        }
    }))
}

/// `GET /api/v1/browse?uri=…` — contenu d'un dossier, format Volumio.
async fn browse(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let uri = q
        .get("uri")
        .cloned()
        .ok_or_else(|| crate::ApiError(auris_core::AurisError::other("paramètre uri requis")))?;
    let sources = s.sources.clone();
    let handle = tokio::runtime::Handle::current();
    let uri2 = uri.clone();
    let entries = offload_io(move || handle.block_on(sources.browse(&uri2))).await?;
    Ok(Json(browse_v1(&uri, entries)))
}

fn browse_v1(uri: &str, entries: Vec<BrowseEntry>) -> Value {
    json!({
        "navigation": {
            "lists": [{
                "title": uri,
                "available": true,
                "items": entries.iter().map(|e| {
                    if e.is_dir {
                        json!({ "type": "folder", "title": e.name, "uri": e.uri, "service": "auris" })
                    } else {
                        json!({
                            "type": "song",
                            "title": e.name,
                            "uri": e.uri,
                            "service": "auris",
                            "samplerate": e.format.map(|f| match f {
                                auris_core::AudioFormat::Pcm { sample_rate, .. } => sample_rate.to_string(),
                                auris_core::AudioFormat::Dsd { sample_rate, .. } => sample_rate.to_string(),
                            }),
                            "bitdepth": e.format.map(|f| match f {
                                auris_core::AudioFormat::Pcm { bits, .. } => bits.to_string(),
                                auris_core::AudioFormat::Dsd { .. } => "dsd".to_string(),
                            }),
                        })
                    }
                }).collect::<Vec<_>>(),
            }]
        }
    })
}

/// `GET /api/v1/search?q=…[&type=…]` — recherche bibliothèque + sources externes.
async fn search(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let query = q.get("q").cloned().unwrap_or_default();
    let type_filter = q.get("type").cloned();
    let mut res = s.library.search_grouped(&query, None);
    let src_items = s.sources.search_sources(&query).await;
    res.sources = auris_core::SearchGroup {
        total: src_items.len(),
        items: src_items,
    };
    let include = |t: &str| type_filter.as_deref().map(|f| f == t).unwrap_or(true);
    let mut out = json!({});
    if include("tracks") {
        out["tracks"] = json!(res.tracks.items.iter().map(track_v1).collect::<Vec<_>>());
    }
    if include("albums") {
        out["albums"] =
            json!(res.albums.items.iter().map(|a| json!({
            "type": "album", "title": a.title, "artist": a.artist, "uri": a.id,
            "albumart": a.cover_track_id.as_ref().map(|c| format!("/api/library/cover/{c}")),
        })).collect::<Vec<_>>());
    }
    if include("artists") {
        out["artists"] = json!(res
            .artists
            .items
            .iter()
            .map(|a| json!({
                "type": "artist", "title": a.name, "uri": a.id,
            }))
            .collect::<Vec<_>>());
    }
    if include("playlists") {
        out["playlists"] = json!(res
            .playlists
            .items
            .iter()
            .map(|p| json!({
                "type": "playlist", "title": p.name, "uri": p.id,
            }))
            .collect::<Vec<_>>());
    }
    if include("sources") {
        out["sources"] = json!(res
            .sources
            .items
            .iter()
            .map(|e| json!({
                "type": if e.is_dir { "folder" } else { "song" },
                "title": e.name, "uri": e.uri, "service": "auris",
            }))
            .collect::<Vec<_>>());
    }
    Ok(Json(out))
}

/// Routeur `/api/v1/*` à merger dans le routeur principal.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/ping", get(ping))
        .route("/api/v1/getState", get(get_state_v1))
        .route("/api/v1/getQueue", get(get_queue_v1))
        .route("/api/v1/commands", get(commands).post(commands))
        .route(
            "/api/v1/replaceAndPlay",
            get(replace_and_play).post(replace_and_play),
        )
        .route("/api/v1/addToQueue", get(add_to_queue))
        .route("/api/v1/getBrowseSources", get(get_browse_sources))
        .route("/api/v1/browse", get(browse))
        .route("/api/v1/search", get(search))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::{AudioFormat, PlayStatus, PlayerState, RepeatMode, Track};

    fn track() -> Track {
        let mut t = Track::new("src://nas/Kind of Blue/01.flac".into(), "nas".into());
        t.id = "t1".into();
        t.title = "So What".into();
        t.artist = "Miles Davis".into();
        t.album = "Kind of Blue".into();
        t.album_artist = "Miles Davis".into();
        t.duration_secs = Some(545.0);
        t.track_number = Some(1);
        t.year = Some(1959);
        t.genre = Some("Jazz".into());
        t.format = Some(AudioFormat::Pcm {
            sample_rate: 192000,
            bits: 24,
            channels: 2,
        });
        t.size_bytes = Some(123_456);
        t.has_cover = true;
        t
    }

    #[test]
    fn state_v1_shape() {
        let st = PlayerState {
            status: PlayStatus::Playing,
            current: Some(track()),
            position_secs: 61.5,
            duration_secs: 545.0,
            volume: 80,
            muted: false,
            repeat: RepeatMode::All,
            shuffle: false,
            queue: vec![track()],
            queue_index: Some(0),
            audio_device: Some("DAC".into()),
            current_format: Some(AudioFormat::Pcm {
                sample_rate: 192000,
                bits: 24,
                channels: 2,
            }),
            max_volume: 100,
            volume_mode: "software".into(),
            output_mode: Default::default(),
            output_backend: "cpal".into(),
            is_live: false,
            stream_title: None,
        };
        let v = state_v1(&st);
        assert_eq!(v["status"], "play");
        assert_eq!(v["title"], "So What");
        assert_eq!(v["seek"], 61500u64, "seek en millisecondes");
        assert_eq!(v["duration"], 545u64);
        assert_eq!(v["samplerate"], "192000");
        assert_eq!(v["bitdepth"], "24");
        assert_eq!(v["repeat"], 1);
        assert_eq!(v["albumart"], "/api/library/cover/t1");
    }

    #[test]
    fn track_v1_shape() {
        let v = track_v1(&track());
        assert_eq!(v["type"], "song");
        assert_eq!(v["title"], "So What");
        assert_eq!(v["duration"], 545u64);
        assert_eq!(v["samplerate"], "192000");
    }

    #[test]
    fn browse_v1_folders_and_songs() {
        let entries = vec![
            BrowseEntry {
                name: "Rock".into(),
                uri: "src://nas/Rock".into(),
                is_dir: true,
                size: None,
                format: None,
                is_audio: false,
            },
            BrowseEntry {
                name: "01.flac".into(),
                uri: "src://nas/Rock/01.flac".into(),
                is_dir: false,
                size: Some(1),
                format: Some(AudioFormat::Pcm {
                    sample_rate: 44100,
                    bits: 16,
                    channels: 2,
                }),
                is_audio: true,
            },
        ];
        let v = browse_v1("src://nas", entries);
        let items = &v["navigation"]["lists"][0]["items"];
        assert_eq!(items[0]["type"], "folder");
        assert_eq!(items[1]["type"], "song");
        assert_eq!(items[1]["bitdepth"], "16");
    }
}
