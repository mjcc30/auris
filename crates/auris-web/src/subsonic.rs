//! API Subsonic (serveur), périmètre v1 **lecture seule** (#79) :
//! `GET|POST /rest/{method}[.view]` adossé à la bibliothèque, pour que les
//! clients Subsonic existants (DSub, Ultrasonic, Strawberry, play:Sub…) se
//! connectent à Auris comme à un serveur perso.
//!
//! - Réponses JSON uniquement (`f=json`, défaut) : `{"subsonic-response": {…}}`.
//!   En cas d'erreur applicative, HTTP 200 + `status:"failed"` (convention
//!   Subsonic). `stream` et `getCoverArt` répondent en binaire.
//! - Authentification : aucune en v1 (le serveur Auris n'a pas de token) —
//!   `u`/`p`/`t`/`s` sont acceptés et ignorés.
//! - Écriture (scrobble, star, createPlaylist…) : v2 — répond `failed` code 0.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::{stream_track_bytes, AppState};

const SUBSONIC_VERSION: &str = "1.16.1";

pub fn router() -> Router<AppState> {
    Router::new().route("/rest/{method}", get(handle).post(handle))
}

fn ok(payload: Value) -> Response {
    let mut inner = json!({ "status": "ok", "version": SUBSONIC_VERSION });
    if let (Some(obj), Some(p)) = (inner.as_object_mut(), payload.as_object()) {
        for (k, v) in p {
            obj.insert(k.clone(), v.clone());
        }
    }
    Json(json!({ "subsonic-response": inner })).into_response()
}

fn fail(code: u32, message: &str) -> Response {
    Json(json!({
        "subsonic-response": {
            "status": "failed",
            "version": SUBSONIC_VERSION,
            "error": { "code": code, "message": message }
        }
    }))
    .into_response()
}

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
}

fn get_usize(params: &HashMap<String, String>, key: &str, default: usize) -> usize {
    param(params, key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .min(10_000)
}

/// Extension du fichier d'une URI `src://…` ou http(s), en minuscules.
fn suffix(uri: &str) -> String {
    uri.rsplit(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn child(t: &auris_core::Track) -> Value {
    let ext = suffix(&t.uri);
    json!({
        "id": t.id,
        "parent": t.album_key,
        "title": t.title,
        "album": t.album,
        "artist": t.artist,
        "track": t.track_number.unwrap_or(0),
        "discNumber": t.disc_number.unwrap_or(1),
        "year": t.year.unwrap_or(0),
        "genre": t.genre,
        "duration": t.duration_secs.unwrap_or(0.0).round() as u64,
        "size": t.size_bytes.unwrap_or(0),
        "suffix": ext,
        "contentType": crate::audio_content_type(&t.uri),
        "path": t.uri.clone(),
        "isDir": false,
        "type": "music",
        "albumId": t.album_key,
        "artistId": t.artist_key,
        "coverArt": if t.has_cover { json!(t.id) } else { Value::Null },
        "created": t.added_at,
    })
}

fn album_json(a: &auris_core::Album, songs: Option<Vec<Value>>) -> Value {
    let mut v = json!({
        "id": a.id,
        "name": a.title,
        "artist": a.artist,
        "artistId": a.artist.to_lowercase(),
        "coverArt": a.cover_track_id,
        "songCount": a.track_count,
        "duration": a.duration_secs.round() as u64,
        "year": a.year.unwrap_or(0),
        "created": 0,
    });
    if let Some(songs) = songs {
        v["song"] = json!(songs);
    }
    v
}

fn artist_json(a: &auris_core::Artist, albums: Option<Vec<Value>>) -> Value {
    let mut v = json!({
        "id": a.id,
        "name": a.name,
        "albumCount": a.album_count,
    });
    if let Some(albums) = albums {
        v["album"] = json!(albums);
    }
    v
}

fn playlist_json(p: &auris_core::Playlist, entries: Option<Vec<Value>>) -> Value {
    let mut v = json!({
        "id": p.id,
        "name": p.name,
        "songCount": p.track_ids.len(),
        "duration": 0,
        "owner": "auris",
        "public": false,
        "created": p.created_at,
    });
    if let Some(entries) = entries {
        v["entry"] = json!(entries);
    }
    v
}

/// Trie les albums selon le `type` de getAlbumList/getAlbumList2.
fn sort_albums(
    mut albums: Vec<auris_core::Album>,
    r#type: &str,
    genre: Option<&str>,
    lib: &auris_library::Library,
) -> Vec<auris_core::Album> {
    match r#type {
        "alphabeticalByArtist" => albums.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        "byGenre" => {
            if let Some(g) = genre {
                let g = g.to_lowercase();
                albums.retain(|a| {
                    lib.album(&a.id)
                        .map(|(_, ts)| {
                            ts.iter().any(|t| {
                                t.genre.as_deref().map(|x| x.to_lowercase()) == Some(g.clone())
                            })
                        })
                        .unwrap_or(false)
                });
            }
            albums.sort_by_key(|a| a.title.to_lowercase());
        }
        "recent" | "byYear" => {
            albums.sort_by(|a, b| b.year.cmp(&a.year).then(a.title.cmp(&b.title)))
        }
        "random" => {
            // tirage simple déterministe par id pour rester reproductible
            albums.sort_by(|a, b| {
                let ha =
                    a.id.bytes()
                        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
                let hb =
                    b.id.bytes()
                        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
                ha.cmp(&hb)
            });
        }
        _ => albums.sort_by_key(|a| a.title.to_lowercase()), // alphabeticalByName + défaut
    }
    albums
}

fn album_list(s: &AppState, params: &HashMap<String, String>) -> Response {
    let r#type = param(params, "type").unwrap_or("alphabeticalByName");
    if r#type == "starred" || r#type == "frequent" {
        return ok(json!({ "albumList2": { "album": [] } }));
    }
    let offset = get_usize(params, "offset", 0);
    let size = get_usize(params, "size", 500);
    let genre = param(params, "genre");
    let mut albums = s.library.albums();
    albums = sort_albums(albums, r#type, genre, &s.library);
    let albums: Vec<Value> = albums
        .into_iter()
        .skip(offset)
        .take(size)
        .map(|a| album_json(&a, None))
        .collect();
    ok(json!({ "albumList2": { "album": albums } }))
}

async fn handle(
    State(s): State<AppState>,
    Path(method): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    http_method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // fusion query string + corps form-urlencoded (POST)
    let mut params = query;
    if http_method == Method::POST {
        let ct = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if ct.starts_with("application/x-www-form-urlencoded") {
            for pair in String::from_utf8_lossy(&body).split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    params.insert(urldecode(k), urldecode(v));
                }
            }
        }
    }
    // `/rest/ping.view` -> méthode « ping »
    let method = method.split('.').next().unwrap_or("").to_string();
    if param(&params, "f").map(|f| f != "json").unwrap_or(false) {
        return fail(0, "seul f=json est supporté en v1");
    }
    let lib = s.library.clone();
    match method.as_str() {
        "ping" => ok(json!({})),
        "getLicense" => ok(json!({ "license": { "valid": true, "email": "auris@localhost" } })),
        "getMusicFolders" => ok(json!({
            "musicFolders": { "musicFolder": [ { "id": 0, "name": "Bibliothèque" } ] }
        })),
        "getScanStatus" => {
            let st = lib.scan_status();
            ok(json!({
                "scanStatus": { "scanning": st.running, "count": lib.track_count() }
            }))
        }
        "getGenres" => {
            let genres: Vec<Value> = lib
                .genres()
                .iter()
                .map(|g| json!({ "value": g.name, "albumCount": g.album_count, "songCount": g.track_count }))
                .collect();
            ok(json!({ "genres": { "genre": genres } }))
        }
        "getArtists" => {
            let artists = lib.artists();
            let mut by_index: HashMap<String, Vec<Value>> = HashMap::new();
            for a in &artists {
                let c = a
                    .name
                    .trim_start()
                    .chars()
                    .next()
                    .map(|c| c.to_ascii_uppercase().to_string())
                    .unwrap_or_else(|| "#".into());
                let key = if c
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphabetic())
                    .unwrap_or(false)
                {
                    c
                } else {
                    "#".into()
                };
                by_index.entry(key).or_default().push(artist_json(a, None));
            }
            let mut index: Vec<Value> = by_index
                .into_iter()
                .map(|(name, artist)| json!({ "name": name, "artist": artist }))
                .collect();
            index.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            ok(json!({ "artists": { "ignoredArticles": "", "index": index } }))
        }
        "getArtist" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            let Some((a, albums)) = lib.artist(id) else {
                return fail(30, "artiste introuvable");
            };
            let albums: Vec<Value> = albums.iter().map(|a| album_json(a, None)).collect();
            ok(json!({ "artist": artist_json(&a, Some(albums)) }))
        }
        "getAlbumList" | "getAlbumList2" => album_list(&s, &params),
        "getAlbum" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            let Some((a, tracks)) = lib.album(id) else {
                return fail(30, "album introuvable");
            };
            let mut songs: Vec<Value> = tracks.iter().map(child).collect();
            songs.sort_by_key(|c| {
                (
                    c["discNumber"].as_u64().unwrap_or(1),
                    c["track"].as_u64().unwrap_or(0),
                )
            });
            ok(json!({ "album": album_json(&a, Some(songs)) }))
        }
        "getSong" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            let Some(t) = lib.track(id) else {
                return fail(30, "piste introuvable");
            };
            ok(json!({ "song": child(&t) }))
        }
        "search3" => {
            let q = param(&params, "query").unwrap_or("");
            let res = lib.search_grouped(q, None);
            let ac = get_usize(&params, "artistCount", 20);
            let alc = get_usize(&params, "albumCount", 20);
            let sc = get_usize(&params, "songCount", 50);
            ok(json!({
                "searchResult3": {
                    "artist": res.artists.items.into_iter().take(ac).map(|a| artist_json(&a, None)).collect::<Vec<_>>(),
                    "album": res.albums.items.into_iter().take(alc).map(|a| album_json(&a, None)).collect::<Vec<_>>(),
                    "song": res.tracks.items.into_iter().take(sc).map(|t| child(&t)).collect::<Vec<_>>(),
                }
            }))
        }
        "getSongsByGenre" => {
            let genre = param(&params, "genre").unwrap_or("");
            let offset = get_usize(&params, "offset", 0);
            let count = get_usize(&params, "count", 50);
            let tracks = lib.tracks_filtered(Some(genre), None);
            let songs: Vec<Value> = tracks
                .into_iter()
                .skip(offset)
                .take(count)
                .map(|t| child(&t))
                .collect();
            ok(json!({ "songsByGenre": { "song": songs } }))
        }
        "getPlaylists" => {
            let pls: Vec<Value> = lib
                .playlists()
                .iter()
                .map(|p| playlist_json(p, None))
                .collect();
            ok(json!({ "playlists": { "playlist": pls } }))
        }
        "getPlaylist" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            let Some((p, tracks)) = lib.playlist(id) else {
                return fail(30, "playlist introuvable");
            };
            let entries: Vec<Value> = tracks.iter().map(child).collect();
            ok(json!({ "playlist": playlist_json(&p, Some(entries)) }))
        }
        "getNowPlaying" => ok(json!({ "nowPlaying": { "entry": [] } })),
        "getCoverArt" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            match lib.cover(id) {
                Some(bytes) => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "image/jpeg"),
                        (header::CONTENT_LENGTH, bytes.len().to_string().as_str()),
                    ],
                    bytes,
                )
                    .into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "subsonic-response": { "status": "failed", "version": SUBSONIC_VERSION, "error": { "code": 30, "message": "pas de pochette" } } })),
                )
                    .into_response(),
            }
        }
        "stream" | "download" => {
            let Some(id) = param(&params, "id") else {
                return fail(10, "paramètre id requis");
            };
            let Some(t) = lib.track(id) else {
                return fail(30, "piste introuvable");
            };
            return match stream_track_bytes(&s, &t).await {
                Ok(bytes) => {
                    let ct = crate::audio_content_type(&t.uri);
                    let len = bytes.len() as u64;
                    if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
                        if let Some((start, end)) = parse_range(range, len) {
                            let end = end.min(len.saturating_sub(1));
                            let start = start.min(end);
                            let slice = bytes[start as usize..=(end as usize)].to_vec();
                            return (
                                StatusCode::PARTIAL_CONTENT,
                                [
                                    (header::CONTENT_TYPE, ct),
                                    (header::CONTENT_LENGTH, slice.len().to_string().as_str()),
                                    (
                                        header::CONTENT_RANGE,
                                        format!("bytes {start}-{end}/{len}").as_str(),
                                    ),
                                    (header::ACCEPT_RANGES, "bytes"),
                                ],
                                slice,
                            )
                                .into_response();
                        }
                    }
                    (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, ct),
                            (header::CONTENT_LENGTH, len.to_string().as_str()),
                            (header::ACCEPT_RANGES, "bytes"),
                        ],
                        bytes,
                    )
                        .into_response()
                }
                Err(e) => fail(70, &format!("lecture impossible: {e}")),
            };
        }
        other => fail(0, &format!("méthode Subsonic non supportée en v1: {other}")),
    }
}

fn parse_range(range: &str, len: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    let (a, b) = range.split_once('-')?;
    let start = a.parse::<u64>().ok()?;
    let end = if b.is_empty() {
        len.saturating_sub(1)
    } else {
        b.parse::<u64>().ok()?
    };
    Some((start, end))
}

fn urldecode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(h) => out.push(h as char),
                    Err(_) => out.push('%'),
                }
                i += 3;
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_bytes() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=0-", 0), Some((0, 0)));
        assert_eq!(parse_range("octets=0-1", 10), None);
    }

    #[test]
    fn urldecode_utile() {
        assert_eq!(urldecode("a+b%20c"), "a b c");
        assert_eq!(urldecode("100%25"), "100%");
    }
}
