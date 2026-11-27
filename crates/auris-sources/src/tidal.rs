//! Source Tidal (#25) — API v1 (`api.tidal.com/v1`) + token bearer de session.
//!
//! Authentification : `Authorization: Bearer <token>` (JWT de session Tidal,
//! récupérable depuis le lecteur web / l'app). La lecture passe par
//! `GET /tracks/{id}/playbackinfopostpaywall` (param `sessionId` = cookie `sid`)
//! qui renvoie un **manifest base64** ; quand `encryptionKey` est vide (flux
//! non chiffré, ex. LOSSLESS/HI_RES_LOSSLESS servis en clair), `urls[0]` est
//! lisible directement en HTTP avec Range — on la confie au lecteur HTTP
//! seekable habituel. Les pistes sont adressées `src://<id>/track/<track_id>`
//! et résolues à la lecture.

use async_trait::async_trait;
use auris_core::{AudioFormat, AurisError, BrowseEntry, Result, Source, SourceKind};
use serde_json::Value;

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

pub const DEFAULT_API_BASE: &str = "https://api.tidal.com/v1";

/// Qualité Tidal demandée selon le réglage.
fn playback_quality(quality: &str) -> &'static str {
    match quality.to_lowercase().as_str() {
        "hires" => "HI_RES_LOSSLESS",
        _ => "LOSSLESS",
    }
}

/// Format audio approximatif selon la qualité Tidal.
fn format_for_quality(sound_quality: &str) -> AudioFormat {
    match sound_quality {
        "LOSSLESS" => AudioFormat::Pcm {
            sample_rate: 44100,
            bits: 16,
            channels: 2,
        },
        // HI_RES / HI_RES_LOSSLESS : FLAC 24 bits jusqu'à 192 kHz (MQA inclus).
        _ => AudioFormat::Pcm {
            sample_rate: 192000,
            bits: 24,
            channels: 2,
        },
    }
}

/// Décodeur base64 minimal (alphabet standard + URL-safe, padding optionnel).
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut table = [255u8; 256];
    for (i, b) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        table[*b as usize] = i as u8;
    }
    table[b'-' as usize] = 62;
    table[b'_' as usize] = 63;
    let bytes: Vec<u8> = s.bytes().filter(|b| !b" \r\n\t=\r".contains(b)).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut nbits = 0;
    for b in bytes {
        let v = table[b as usize];
        if v == 255 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    Some(out)
}

/// Extrait l'id utilisateur d'un JWT de session Tidal (payload, claim `userId`
/// ou `user.id`) — repli si `/users/me` ne répond pas.
fn user_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64_decode(payload)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("userId")
        .and_then(|x| x.as_u64().map(|n| n.to_string()))
        .or_else(|| {
            v.get("user")
                .and_then(|u| u.get("id"))
                .and_then(|x| x.as_u64().map(|n| n.to_string()))
        })
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| {
        x.as_str()
            .map(|s| s.to_string())
            .or_else(|| x.as_u64().map(|n| n.to_string()))
    })
}

fn get_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| x.as_u64())
}

fn get_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// Piste Tidal (éventuellement enveloppée dans `item`, ex. playlists/albums).
fn unwrap_item(t: &Value) -> &Value {
    t.get("item").filter(|i| i.is_object()).unwrap_or(t)
}

/// Client HTTP Tidal (blocking).
#[derive(Clone)]
struct TidalClient {
    api_base: String,
    token: String,
    session_id: String,
    quality: &'static str,
    user_id: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl TidalClient {
    fn new(api_base: &str, token: &str, session_id: &str, quality: &str) -> Self {
        TidalClient {
            api_base: api_base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            session_id: session_id.to_string(),
            quality: playback_quality(quality),
            user_id: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    }

    /// GET authentifié ; 401 = token expiré (pas de re-login possible ici).
    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.api_base, path);
        let resp = self
            .http()
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .query(params)
            .send()
            .map_err(|e| AurisError::other(format!("tidal {path}: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .map_err(|e| AurisError::other(format!("tidal {path} (json): {e}")))?;
        if status == 401 || status == 403 {
            return Err(AurisError::other(format!(
                "tidal {path}: token refusé (HTTP {status}) — regénérez le token de session"
            )));
        }
        if !status.is_success() {
            return Err(AurisError::other(format!(
                "tidal {path} a échoué (HTTP {status}): {}",
                body.get("userMessage")
                    .and_then(|m| m.as_str())
                    .or_else(|| body.get("message").and_then(|m| m.as_str()))
                    .unwrap_or("")
            )));
        }
        Ok(body)
    }

    /// Id utilisateur du compte (API `/users/me`, repli claim `userId` du JWT).
    fn user_id(&self) -> Result<String> {
        if let Some(id) = self.user_id.lock().unwrap().clone() {
            return Ok(id);
        }
        let api_res = self.get("/users/me", &[]);
        if let Ok(body) = &api_res {
            if let Some(id) = get_u64(body, "id").map(|n| n.to_string()) {
                *self.user_id.lock().unwrap() = Some(id.clone());
                return Ok(id);
            }
        }
        if let Some(id) = user_id_from_jwt(&self.token) {
            *self.user_id.lock().unwrap() = Some(id.clone());
            return Ok(id);
        }
        // Erreur de l'API prioritaire (ex. 401 token refusé), sinon JWT illisible.
        Err(api_res.err().unwrap_or_else(|| {
            AurisError::other("tidal: id utilisateur introuvable (users/me + JWT)")
        }))
    }

    /// URL de flux d'une piste : manifest base64 de playbackinfopostpaywall,
    /// jouable directement quand `encryptionKey` est vide.
    fn track_url(&self, track_id: &str) -> Result<(String, Option<AudioFormat>)> {
        let body = self.get(
            &format!("/tracks/{track_id}/playbackinfopostpaywall"),
            &[
                ("sessionId", self.session_id.clone()),
                ("playbackQuality", self.quality.to_string()),
                ("assetPresentation", "AUDIO".to_string()),
            ],
        )?;
        let audio_quality = get_str(&body, "audioQuality").unwrap_or_else(|| "LOSSLESS".into());
        let fmt = Some(format_for_quality(&audio_quality));
        let manifest_b64 = get_str(&body, "manifest")
            .ok_or_else(|| AurisError::other("tidal playbackinfo: manifest absent"))?;
        let manifest_bytes = b64_decode(&manifest_b64)
            .ok_or_else(|| AurisError::other("tidal: manifest base64 invalide"))?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| AurisError::other(format!("tidal: manifest json: {e}")))?;
        let encrypted = manifest
            .get("encryptionKey")
            .and_then(|k| k.as_str())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if encrypted {
            return Err(AurisError::other(
                "tidal: flux chiffré (encryptionKey présent) — lecture DRM non supportée",
            ));
        }
        let url = manifest
            .get("urls")
            .and_then(|u| u.as_array())
            .and_then(|a| a.first())
            .and_then(|u| u.as_str())
            .ok_or_else(|| AurisError::other("tidal: manifest sans urls"))?
            .to_string();
        Ok((url, fmt))
    }
}

// ---------- helpers de navigation (purs, testables) ----------

fn dir(src_id: &str, rel: &str, name: &str) -> BrowseEntry {
    BrowseEntry {
        name: name.to_string(),
        uri: make_uri(src_id, rel),
        is_dir: true,
        size: None,
        format: None,
        is_audio: false,
    }
}

fn track_name(t: &Value) -> String {
    let t = unwrap_item(t);
    let artist = t
        .get("artist")
        .and_then(|a| get_str(a, "name"))
        .unwrap_or_default();
    let title = get_str(t, "title").unwrap_or_default();
    let dur = get_f64(t, "duration").unwrap_or(0.0);
    format!(
        "{}. {} — {} ({:02}:{:02})",
        get_u64(t, "trackNumber").unwrap_or(0),
        artist,
        title,
        (dur / 60.0) as u32,
        (dur % 60.0) as u32
    )
}

fn track_entry(src_id: &str, t: &Value) -> Option<BrowseEntry> {
    let t = unwrap_item(t);
    let id = get_u64(t, "id").map(|n| n.to_string())?;
    let quality = get_str(t, "audioQuality").unwrap_or_else(|| "LOSSLESS".into());
    Some(BrowseEntry {
        name: track_name(t),
        uri: make_uri(src_id, &format!("track/{id}")),
        is_dir: false,
        size: None,
        format: Some(format_for_quality(&quality)),
        is_audio: true,
    })
}

fn album_dir(src_id: &str, album: &Value) -> BrowseEntry {
    let id = get_u64(album, "id")
        .map(|n| n.to_string())
        .unwrap_or_default();
    let title = get_str(album, "title").unwrap_or_else(|| "Album".into());
    let artist = album
        .get("artist")
        .and_then(|a| get_str(a, "name"))
        .unwrap_or_default();
    let name = if artist.is_empty() {
        title
    } else {
        format!("{title} — {artist}")
    };
    BrowseEntry {
        name,
        uri: make_uri(src_id, &format!("album/{id}")),
        is_dir: true,
        size: None,
        format: None,
        is_audio: false,
    }
}

fn artist_dir(src_id: &str, artist: &Value) -> BrowseEntry {
    let id = get_u64(artist, "id")
        .map(|n| n.to_string())
        .unwrap_or_default();
    let name = get_str(artist, "name").unwrap_or_else(|| "Artiste".into());
    BrowseEntry {
        name,
        uri: make_uri(src_id, &format!("artist/{id}")),
        is_dir: true,
        size: None,
        format: None,
        is_audio: false,
    }
}

/// Collection d'items : `{"albums": {"items": [...]}}` (search/favoris) ou
/// `{"items": [...]}` direct (albums/{id}/items, playlists/{uuid}/items).
fn items_of<'a>(body: &'a Value, collection: &str) -> &'a [Value] {
    match body.get(collection) {
        Some(Value::Array(a)) => a.as_slice(),
        Some(c) => c
            .get("items")
            .and_then(|i| i.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]),
        None => &[],
    }
}

/// Navigation Tidal (synchrone, déportée en spawn_blocking par le backend).
fn list_sync(client: &TidalClient, src_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        "" => Ok(vec![
            dir(src_id, "favorites", "Favoris"),
            dir(src_id, "playlists", "Mes playlists"),
        ]),
        "favorites" => Ok(vec![
            dir(src_id, "favorites/albums", "Albums"),
            dir(src_id, "favorites/tracks", "Pistes"),
            dir(src_id, "favorites/artists", "Artistes"),
        ]),
        "favorites/albums" => {
            let uid = client.user_id()?;
            let body = client.get(
                &format!("/users/{uid}/favorites/albums"),
                &[("limit", "100".into()), ("offset", "0".into())],
            )?;
            // /users/{uid}/favorites/* renvoie {"items": [...]} directement.
            Ok(items_of(&body, "items")
                .iter()
                .map(|a| album_dir(src_id, a))
                .collect())
        }
        "favorites/tracks" => {
            let uid = client.user_id()?;
            let body = client.get(
                &format!("/users/{uid}/favorites/tracks"),
                &[("limit", "100".into()), ("offset", "0".into())],
            )?;
            Ok(items_of(&body, "items")
                .iter()
                .filter_map(|t| track_entry(src_id, t))
                .collect())
        }
        "favorites/artists" => {
            let uid = client.user_id()?;
            let body = client.get(
                &format!("/users/{uid}/favorites/artists"),
                &[("limit", "100".into()), ("offset", "0".into())],
            )?;
            Ok(items_of(&body, "items")
                .iter()
                .map(|a| artist_dir(src_id, a))
                .collect())
        }
        "playlists" => {
            let uid = client.user_id()?;
            let body = client.get(
                &format!("/users/{uid}/playlists"),
                &[("limit", "100".into()), ("offset", "0".into())],
            )?;
            // /users/{uid}/playlists renvoie {"items": [...]} directement.
            Ok(items_of(&body, "items")
                .iter()
                .filter_map(|p| {
                    let uuid = get_str(p, "uuid")?;
                    let name = get_str(p, "title").unwrap_or_else(|| "Playlist".into());
                    Some(dir(src_id, &format!("playlist/{uuid}"), &name))
                })
                .collect())
        }
        other => {
            if let Some(id) = other.strip_prefix("album/") {
                let body = client.get(
                    &format!("/albums/{id}/items"),
                    &[("limit", "100".into()), ("offset", "0".into())],
                )?;
                return Ok(items_of(&body, "items")
                    .iter()
                    .filter_map(|t| track_entry(src_id, t))
                    .collect());
            }
            if let Some(id) = other.strip_prefix("artist/") {
                let body = client.get(
                    &format!("/artists/{id}/albums"),
                    &[("limit", "100".into()), ("offset", "0".into())],
                )?;
                return Ok(items_of(&body, "items")
                    .iter()
                    .map(|a| album_dir(src_id, a))
                    .collect());
            }
            if let Some(uuid) = other.strip_prefix("playlist/") {
                let body = client.get(
                    &format!("/playlists/{uuid}/items"),
                    &[("limit", "200".into()), ("offset", "0".into())],
                )?;
                return Ok(items_of(&body, "items")
                    .iter()
                    .filter_map(|t| track_entry(src_id, t))
                    .collect());
            }
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(client, src_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin tidal inconnu: {rel_path}"
            )))
        }
    }
}

/// Recherche Tidal (albums + artistes + pistes).
fn search_sync(client: &TidalClient, src_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let body = client.get(
        "/search",
        &[
            ("query", q.to_string()),
            ("types", "TRACKS,ALBUMS,ARTISTS".into()),
            ("limit", "20".into()),
        ],
    )?;
    let mut out = Vec::new();
    out.extend(
        items_of(&body, "albums")
            .iter()
            .map(|a| album_dir(src_id, a)),
    );
    out.extend(
        items_of(&body, "artists")
            .iter()
            .map(|a| artist_dir(src_id, a)),
    );
    out.extend(
        items_of(&body, "tracks")
            .iter()
            .filter_map(|t| track_entry(src_id, t)),
    );
    Ok(out)
}

/// Source Tidal : navigation + lecture des pistes résolues à la volée.
pub struct TidalSource {
    source: Source,
    client: TidalClient,
    /// Lecteur HTTP seekable pour les flux Tidal (URL du manifest).
    http: HttpSource,
}

impl TidalSource {
    pub fn new(source: Source) -> Result<Self> {
        let (token, session_id, quality, api_base) = match &source.kind {
            SourceKind::Tidal {
                token,
                session_id,
                quality,
                api_base,
            } => (
                token.clone(),
                session_id.clone(),
                quality.clone(),
                api_base.clone(),
            ),
            _ => return Err(AurisError::other("source non tidal")),
        };
        if token.trim().is_empty() {
            return Err(AurisError::other(
                "source Tidal: renseignez le token bearer de session",
            ));
        }
        let client = TidalClient::new(
            api_base.as_deref().unwrap_or(DEFAULT_API_BASE),
            &token,
            session_id.as_deref().unwrap_or(""),
            &quality,
        );
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(TidalSource {
            source,
            client,
            http,
        })
    }
}

#[async_trait]
impl SourceBackend for TidalSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let client = self.client.clone();
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&client, &src_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("tidal list (join): {e}")))?
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let rel = rel_path.trim_matches('/').to_string();
        let track_id = rel
            .strip_prefix("track/")
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?
            .to_string();
        let client = self.client.clone();
        let url = tokio::task::spawn_blocking(move || client.track_url(&track_id).map(|(u, _)| u))
            .await
            .map_err(|e| AurisError::other(format!("tidal open (join): {e}")))??;
        Ok(Box::new(self.http.open_url(&url)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let client = self.client.clone();
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&client, &src_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("tidal search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> TidalClient {
        TidalClient::new("http://localhost", "tok", "SID", "hires")
    }

    #[test]
    fn b64_roundtrip() {
        // "hello" en base64 standard et url-safe
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(
            b64_decode("eyJ1c2VySWQiOjQyfQ").unwrap(),
            br#"{"userId":42}"#
        );
        assert!(b64_decode("!!!").is_none());
    }

    #[test]
    fn jwt_user_id() {
        // payload {"userId": 4242} en base64url sans padding
        let tok = format!("aaa.{}.ccc", base64url(br#"{"userId": 4242}"#));
        assert_eq!(user_id_from_jwt(&tok).as_deref(), Some("4242"));
        assert!(user_id_from_jwt("pas.un.jwt").is_none());
    }

    fn base64url(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(A[(n >> 6) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                out.push(A[n as usize & 63] as char);
            }
        }
        out
    }

    #[test]
    fn quality_mapping() {
        assert_eq!(playback_quality("hires"), "HI_RES_LOSSLESS");
        assert_eq!(playback_quality("lossless"), "LOSSLESS");
        assert_eq!(playback_quality(""), "LOSSLESS");
        match format_for_quality("LOSSLESS") {
            AudioFormat::Pcm {
                sample_rate, bits, ..
            } => {
                assert_eq!((sample_rate, bits), (44100, 16));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_album_items_wrapper() {
        // /albums/{id}/items enveloppe chaque piste dans {"item": {...}}
        let body = json!({"items": [
            {"type": "track", "item": {"id": 101, "title": "So What", "trackNumber": 1,
                "duration": 545.0, "artist": {"name": "Miles Davis"}, "audioQuality": "LOSSLESS"}},
            {"id": 102, "title": "Direct", "trackNumber": 2, "duration": 10.0, "audioQuality": "HI_RES"}
        ]});
        let tracks: Vec<_> = items_of(&body, "items")
            .iter()
            .filter_map(|t| track_entry("t1", t))
            .collect();
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0].uri.ends_with("/track/101"));
        assert!(tracks[0].name.contains("Miles Davis"));
        assert!(tracks[0].name.contains("09:05"));
        match tracks[1].format.unwrap() {
            AudioFormat::Pcm {
                sample_rate, bits, ..
            } => assert_eq!((sample_rate, bits), (192000, 24)),
            _ => panic!(),
        }
    }

    #[test]
    fn root_listing_dirs() {
        let entries = list_sync(&client(), "t1", "").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.is_dir));
    }

    #[test]
    fn unknown_path_err() {
        assert!(list_sync(&client(), "t1", "nimportequoi").is_err());
    }

    #[test]
    fn source_kind_serde() {
        let src: Source = serde_json::from_str(
            r#"{"id":"t","name":"Tidal","enabled":true,"kind":"tidal","token":"abc"}"#,
        )
        .unwrap();
        match &src.kind {
            SourceKind::Tidal {
                quality,
                session_id,
                api_base,
                ..
            } => {
                assert_eq!(quality, "lossless");
                assert!(session_id.is_none());
                assert!(api_base.is_none());
            }
            _ => panic!("attendu Tidal"),
        }
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"tidal""#), "{s}");
    }
}
