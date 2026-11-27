//! Source Qobuz (#25) — API communautaire + compte personnel.
//!
//! Authentification : `POST {base}/user/login` (app_id + user + password)
//! renvoie un `user_auth_token` ; les appels suivants passent les en-têtes
//! `X-App-Id`, `X-App-Secret`, `X-User-Auth-Token`. Les app_id/app_secret sont
//! ceux du lecteur web public et peuvent être forcés dans la source.
//!
//! Lecture : `GET {base}/track/getFileUrl` signé (md5) renvoie une URL HTTP
//! temporaire — la lecture passe ensuite par le lecteur HTTP seekable
//! habituel (`HttpSource::open_url`). Les pistes sont adressées par l'URI
//! `src://<id>/track/<track_id>` résolue à la lecture (token rafraîchi si
//! nécessaire).

use std::sync::Mutex;

use async_trait::async_trait;
use auris_core::{AudioFormat, AurisError, BrowseEntry, Result, Source, SourceKind};
use md5::{Digest, Md5};
use serde_json::Value;

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

pub const DEFAULT_API_BASE: &str = "https://www.qobuz.com/api.json/0.2";
/// app_id public du lecteur web Qobuz (surchargeable dans la source).
pub const DEFAULT_APP_ID: &str = "950096963";

/// Format Qobuz demandé selon le réglage qualité.
fn format_id_for(quality: &str) -> u32 {
    match quality.to_lowercase().as_str() {
        "mp3" => 5,  // MP3 320 kbps
        "flac" => 6, // FLAC 16 bits / 44.1 kHz
        _ => 27,     // Hi-Res 24 bits (jusqu'à 24/192 selon le titre)
    }
}

/// Signature md5 du `track/getFileUrl` (formule publique des clients ouverts).
fn file_url_sig(app_secret: &str, format_id: u32, track_id: &str, ts: u64) -> String {
    let mut h = Md5::new();
    h.update(format!(
        "trackgetFileUrlformat_id{format_id}intentstreamtrack_id{track_id}{ts}{app_secret}"
    ));
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| {
        x.as_str()
            .map(|s| s.to_string())
            .or_else(|| x.as_i64().map(|n| n.to_string()))
    })
}

fn get_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| x.as_u64())
}

fn get_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// Client HTTP Qobuz (blocking, comme le reste des sources réseau).
#[derive(Clone)]
struct QobuzClient {
    api_base: String,
    app_id: String,
    app_secret: String,
    user: String,
    password: String,
    format_id: u32,
    /// (user_auth_token, user_id) mis en cache après login.
    session: std::sync::Arc<Mutex<Option<(String, String)>>>,
}

impl QobuzClient {
    fn new(
        api_base: &str,
        app_id: &str,
        app_secret: &str,
        user: &str,
        password: &str,
        quality: &str,
    ) -> Self {
        QobuzClient {
            api_base: api_base.trim_end_matches('/').to_string(),
            app_id: app_id.to_string(),
            app_secret: app_secret.to_string(),
            user: user.to_string(),
            password: password.to_string(),
            format_id: format_id_for(quality),
            session: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    fn http(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    }

    fn login(&self) -> Result<(String, String)> {
        let url = format!("{}/user/login", self.api_base);
        let resp = self
            .http()
            .post(&url)
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "username": self.user,
                "password": self.password,
            }))
            .send()
            .map_err(|e| AurisError::other(format!("qobuz login: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .map_err(|e| AurisError::other(format!("qobuz login (json): {e}")))?;
        if !status.is_success() {
            return Err(AurisError::other(format!(
                "qobuz login refusé (HTTP {status}): {}",
                body.get("message").and_then(|m| m.as_str()).unwrap_or("")
            )));
        }
        let token = get_str(&body, "user_auth_token")
            .ok_or_else(|| AurisError::other("qobuz login: user_auth_token absent"))?;
        let user_id = body
            .get("user")
            .and_then(|u| get_u64(u, "id").map(|n| n.to_string()))
            .or_else(|| get_str(&body, "user_id"))
            .ok_or_else(|| AurisError::other("qobuz login: user.id absent"))?;
        Ok((token, user_id))
    }

    fn ensure_session(&self) -> Result<(String, String)> {
        {
            let guard = self.session.lock().unwrap();
            if let Some(s) = guard.clone() {
                return Ok(s);
            }
        }
        let s = self.login()?;
        *self.session.lock().unwrap() = Some(s.clone());
        Ok(s)
    }

    /// GET authentifié sur l'API ; re-login une fois si le token est refusé.
    fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.api_base, path);
        let mut last_err = String::new();
        for attempt in 0..2 {
            if attempt == 1 {
                // Token refusé : on l'invalide pour forcer un re-login.
                *self.session.lock().unwrap() = None;
            }
            let (token, _) = self.ensure_session()?;
            let resp = self
                .http()
                .get(&url)
                .header("X-App-Id", &self.app_id)
                .header("X-App-Secret", &self.app_secret)
                .header("X-User-Auth-Token", &token)
                .query(params)
                .send()
                .map_err(|e| AurisError::other(format!("qobuz {path}: {e}")))?;
            let status = resp.status();
            let body: Value = resp
                .json()
                .map_err(|e| AurisError::other(format!("qobuz {path} (json): {e}")))?;
            if status == 401 || status == 403 {
                last_err = format!("auth refusée (HTTP {status})");
                continue;
            }
            if !status.is_success() {
                return Err(AurisError::other(format!(
                    "qobuz {path} a échoué (HTTP {status}): {}",
                    body.get("message").and_then(|m| m.as_str()).unwrap_or("")
                )));
            }
            return Ok(body);
        }
        Err(AurisError::other(format!(
            "qobuz {path}: {last_err} après re-login"
        )))
    }

    /// URL de flux signée d'une piste (temporaire, résolue à la lecture).
    fn track_url(&self, track_id: &str) -> Result<(String, Option<AudioFormat>)> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let sig = file_url_sig(&self.app_secret, self.format_id, track_id, ts);
        let body = self.get(
            "/track/getFileUrl",
            &[
                ("track_id", track_id.to_string()),
                ("format_id", self.format_id.to_string()),
                ("intent", "stream".to_string()),
                ("request_ts", ts.to_string()),
                ("request_sig", sig),
            ],
        )?;
        let url = get_str(&body, "url")
            .ok_or_else(|| AurisError::other("qobuz getFileUrl: url absente"))?;
        let format = match (get_f64(&body, "sampling_rate"), get_f64(&body, "bit_depth")) {
            (Some(sr), Some(bits)) if sr > 0.0 && bits > 0.0 => Some(AudioFormat::Pcm {
                sample_rate: sr as u32,
                bits: bits as u8,
                channels: 2,
            }),
            _ => None,
        };
        Ok((url, format))
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

fn track_entry(
    src_id: &str,
    track_id: &str,
    name: &str,
    format: Option<AudioFormat>,
) -> BrowseEntry {
    BrowseEntry {
        name: name.to_string(),
        uri: make_uri(src_id, &format!("track/{track_id}")),
        is_dir: false,
        size: None,
        format,
        is_audio: true,
    }
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

/// Format audio approximatif d'un album (d'après ses métadonnées).
fn album_format(album: &Value) -> Option<AudioFormat> {
    let sr = get_f64(album, "maximum_sampling_rate").unwrap_or(44100.0);
    let bits = get_f64(album, "maximum_bit_depth").unwrap_or(16.0);
    if sr <= 0.0 {
        return None;
    }
    Some(AudioFormat::Pcm {
        sample_rate: sr as u32,
        bits: bits as u8,
        channels: 2,
    })
}

/// Pistes d'un album (réponse /album/get avec extra=tracks).
fn album_tracks(src_id: &str, album: &Value) -> Vec<BrowseEntry> {
    let fmt = album_format(album);
    album
        .get("tracks")
        .and_then(|t| t.get("items"))
        .and_then(|i| i.as_array())
        .map(|tracks| {
            tracks
                .iter()
                .filter_map(|t| {
                    let id = get_u64(t, "id").map(|n| n.to_string())?;
                    let title = get_str(t, "title").unwrap_or_else(|| "Piste".into());
                    let artist = t
                        .get("artist")
                        .and_then(|a| get_str(a, "name"))
                        .unwrap_or_default();
                    let dur = get_f64(t, "duration").unwrap_or(0.0);
                    let name = format!(
                        "{}. {} — {} ({:02}:{:02})",
                        get_u64(t, "track_number").unwrap_or(0),
                        artist,
                        title,
                        (dur / 60.0) as u32,
                        (dur % 60.0) as u32
                    );
                    Some(track_entry(src_id, &id, &name, fmt))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn track_name(t: &Value) -> String {
    format!(
        "{} — {}",
        t.get("artist")
            .and_then(|a| get_str(a, "name"))
            .unwrap_or_default(),
        get_str(t, "title").unwrap_or_default()
    )
}

/// Navigation Qobuz (synchrone, déportée en spawn_blocking par le backend).
fn list_sync(client: &QobuzClient, src_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        "" => Ok(vec![
            dir(src_id, "favorites", "Favoris"),
            dir(src_id, "playlists", "Mes playlists"),
            dir(src_id, "new", "Nouveautés & éditorial"),
        ]),
        "favorites" => Ok(vec![
            dir(src_id, "favorites/albums", "Albums"),
            dir(src_id, "favorites/artists", "Artistes"),
            dir(src_id, "favorites/tracks", "Pistes"),
        ]),
        "favorites/albums" => {
            let (_, uid) = client.ensure_session()?;
            let body = client.get(
                "/favorite/get",
                &[
                    ("user_id", uid),
                    ("type", "albums".into()),
                    ("limit", "200".into()),
                ],
            )?;
            Ok(body
                .get("albums")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
                .map(|items| items.iter().map(|a| album_dir(src_id, a)).collect())
                .unwrap_or_default())
        }
        "favorites/artists" => {
            let (_, uid) = client.ensure_session()?;
            let body = client.get(
                "/favorite/get",
                &[
                    ("user_id", uid),
                    ("type", "artists".into()),
                    ("limit", "200".into()),
                ],
            )?;
            Ok(body
                .get("artists")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
                .map(|items| items.iter().map(|a| artist_dir(src_id, a)).collect())
                .unwrap_or_default())
        }
        "favorites/tracks" => {
            let (_, uid) = client.ensure_session()?;
            let body = client.get(
                "/favorite/get",
                &[
                    ("user_id", uid),
                    ("type", "tracks".into()),
                    ("limit", "200".into()),
                ],
            )?;
            Ok(body
                .get("tracks")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|t| {
                            let id = get_u64(t, "id").map(|n| n.to_string())?;
                            Some(track_entry(src_id, &id, &track_name(t), None))
                        })
                        .collect()
                })
                .unwrap_or_default())
        }
        "new" => {
            let body = client.get(
                "/album/getFeatured",
                &[
                    ("type", "new-releases".into()),
                    ("limit", "50".into()),
                    ("offset", "0".into()),
                ],
            )?;
            Ok(body
                .get("albums")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
                .map(|items| items.iter().map(|a| album_dir(src_id, a)).collect())
                .unwrap_or_default())
        }
        "playlists" => {
            let (_, uid) = client.ensure_session()?;
            let body = client.get(
                "/playlist/getUserPlaylists",
                &[
                    ("user_id", uid),
                    ("limit", "100".into()),
                    ("offset", "0".into()),
                ],
            )?;
            Ok(body
                .get("playlists")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|p| {
                            let id = get_u64(p, "id").map(|n| n.to_string())?;
                            let name = get_str(p, "name").unwrap_or_else(|| "Playlist".into());
                            Some(dir(src_id, &format!("playlist/{id}"), &name))
                        })
                        .collect()
                })
                .unwrap_or_default())
        }
        other => {
            if let Some(id) = other.strip_prefix("album/") {
                let body = client.get(
                    "/album/get",
                    &[
                        ("album_id", id.to_string()),
                        ("extra", "tracks".into()),
                        ("limit", "200".into()),
                    ],
                )?;
                return Ok(album_tracks(src_id, &body));
            }
            if let Some(id) = other.strip_prefix("artist/") {
                let body = client.get(
                    "/artist/get",
                    &[
                        ("artist_id", id.to_string()),
                        ("extra", "albums".into()),
                        ("limit", "100".into()),
                    ],
                )?;
                return Ok(body
                    .get("albums")
                    .and_then(|a| a.get("items"))
                    .and_then(|i| i.as_array())
                    .map(|items| items.iter().map(|a| album_dir(src_id, a)).collect())
                    .unwrap_or_default());
            }
            if let Some(id) = other.strip_prefix("playlist/") {
                let body = client.get(
                    "/playlist/get",
                    &[
                        ("playlist_id", id.to_string()),
                        ("extra", "tracks".into()),
                        ("limit", "500".into()),
                    ],
                )?;
                return Ok(body
                    .get("tracks")
                    .and_then(|t| t.get("items"))
                    .and_then(|i| i.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|t| {
                                // pistes de playlist : à plat ou imbriquées dans "track"
                                let t = t.get("track").unwrap_or(t);
                                let id = get_u64(t, "id").map(|n| n.to_string())?;
                                Some(track_entry(src_id, &id, &track_name(t), None))
                            })
                            .collect()
                    })
                    .unwrap_or_default());
            }
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(client, src_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin qobuz inconnu: {rel_path}"
            )))
        }
    }
}

/// Recherche Qobuz (albums + artistes + pistes).
fn search_sync(client: &QobuzClient, src_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let body = client.get(
        "/search",
        &[
            ("query", q.to_string()),
            ("limit", "20".into()),
            ("type", "albums,artists,tracks".into()),
        ],
    )?;
    let mut out = Vec::new();
    if let Some(albums) = body
        .get("albums")
        .and_then(|a| a.get("items"))
        .and_then(|i| i.as_array())
    {
        out.extend(albums.iter().map(|a| album_dir(src_id, a)));
    }
    if let Some(artists) = body
        .get("artists")
        .and_then(|a| a.get("items"))
        .and_then(|i| i.as_array())
    {
        out.extend(artists.iter().map(|a| artist_dir(src_id, a)));
    }
    if let Some(tracks) = body
        .get("tracks")
        .and_then(|a| a.get("items"))
        .and_then(|i| i.as_array())
    {
        out.extend(tracks.iter().filter_map(|t| {
            let id = get_u64(t, "id").map(|n| n.to_string())?;
            Some(track_entry(src_id, &id, &track_name(t), None))
        }));
    }
    Ok(out)
}

/// Source Qobuz : navigation + lecture des pistes résolues à la volée.
pub struct QobuzSource {
    source: Source,
    client: QobuzClient,
    /// Lecteur HTTP seekable pour les flux Qobuz (URL signées).
    http: HttpSource,
}

impl QobuzSource {
    pub fn new(source: Source) -> Result<Self> {
        let (user, password, quality, app_id, app_secret, api_base) = match &source.kind {
            SourceKind::Qobuz {
                user,
                password,
                quality,
                app_id,
                app_secret,
                api_base,
            } => (
                user.clone(),
                password.clone(),
                quality.clone(),
                app_id.clone(),
                app_secret.clone(),
                api_base.clone(),
            ),
            _ => return Err(AurisError::other("source non qobuz")),
        };
        if user.trim().is_empty() || password.is_empty() {
            return Err(AurisError::other(
                "source Qobuz: renseignez user et password (compte Qobuz)",
            ));
        }
        let client = QobuzClient::new(
            api_base.as_deref().unwrap_or(DEFAULT_API_BASE),
            app_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_APP_ID),
            app_secret.as_deref().unwrap_or(""),
            &user,
            &password,
            &quality,
        );
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(QobuzSource {
            source,
            client,
            http,
        })
    }
}

#[async_trait]
impl SourceBackend for QobuzSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let client = self.client.clone();
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&client, &src_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("qobuz list (join): {e}")))?
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
            .map_err(|e| AurisError::other(format!("qobuz open (join): {e}")))??;
        Ok(Box::new(self.http.open_url(&url)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let client = self.client.clone();
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&client, &src_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("qobuz search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> QobuzClient {
        QobuzClient::new("http://localhost", "1", "SECRET", "u", "p", "hires")
    }

    #[test]
    fn format_id_mapping() {
        assert_eq!(format_id_for("mp3"), 5);
        assert_eq!(format_id_for("flac"), 6);
        assert_eq!(format_id_for("hires"), 27);
        assert_eq!(format_id_for(""), 27);
    }

    #[test]
    fn file_url_sig_stable() {
        let sig = file_url_sig("SECRET", 27, "12345", 1700000000);
        assert_eq!(sig.len(), 32);
        assert_eq!(sig, file_url_sig("SECRET", 27, "12345", 1700000000));
        assert_ne!(sig, file_url_sig("SECRET", 6, "12345", 1700000000));
    }

    #[test]
    fn parse_album_tracks() {
        let album = json!({
            "id": 999,
            "title": "Kind of Blue",
            "maximum_sampling_rate": 192000.0,
            "maximum_bit_depth": 24.0,
            "tracks": {"items": [
                {"id": 1, "title": "So What", "track_number": 1, "duration": 545.0,
                 "artist": {"name": "Miles Davis"}},
                {"id": 2, "title": "Freddie Freeloader", "track_number": 2, "duration": 592.0}
            ]}
        });
        let tracks = album_tracks("q1", &album);
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0].uri.ends_with("/track/1"), "{}", tracks[0].uri);
        assert!(tracks[0].name.contains("Miles Davis"));
        assert!(tracks[0].name.contains("09:05"));
        assert!(tracks[0].is_audio);
        match tracks[0].format.unwrap() {
            AudioFormat::Pcm {
                sample_rate, bits, ..
            } => {
                assert_eq!(sample_rate, 192000);
                assert_eq!(bits, 24);
            }
            _ => panic!("attendu PCM"),
        }
    }

    #[test]
    fn root_listing_dirs() {
        let entries = list_sync(&client(), "q1", "").unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.is_dir));
        assert!(entries[0].uri.ends_with("/favorites"));
    }

    #[test]
    fn unknown_path_err() {
        assert!(list_sync(&client(), "q1", "nimportequoi").is_err());
    }

    #[test]
    fn source_kind_serde() {
        let src: Source = serde_json::from_str(
            r#"{"id":"q","name":"Qobuz","enabled":true,"kind":"qobuz","user":"u","password":"p"}"#,
        )
        .unwrap();
        match &src.kind {
            SourceKind::Qobuz {
                quality, api_base, ..
            } => {
                assert_eq!(quality, "hires", "quality par défaut");
                assert!(api_base.is_none());
            }
            _ => panic!("attendu Qobuz"),
        }
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"qobuz""#), "{s}");
    }
}
