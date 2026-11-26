//! Radios web : favoris persistés (`<data-dir>/radios.json`), radios intégrées
//! à Auris et sources `kind = "radio"` de la configuration, + lecture directe.
//!
//! La lecture d'une radio passe par l'URI HTTP du flux : `SourcesManager`
//! bascule automatiquement sur le lecteur de flux live quand il n'y a pas de
//! `Content-Length` (voir `auris_sources::live`), et le moteur marque la piste
//! `is_live` (durée inconnue, lecture infinie, métadonnées ICY `StreamTitle`).

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use auris_core::{AurisError, PlayerState, RadioEntry, RadioOrigin, SourceKind, Track};

use crate::{ApiResult, AppState};

/// Radios fournies avec Auris : (id, nom, URL du flux).
pub const BUILTIN: &[(&str, &str, &str)] = &[
    ("fip", "FIP", "https://icecast.radiofrance.fr/fip-hifi.aac"),
    (
        "france-inter",
        "France Inter",
        "https://icecast.radiofrance.fr/franceinter-hifi.aac",
    ),
    (
        "france-musique",
        "France Musique",
        "https://icecast.radiofrance.fr/francemusique-hifi.aac",
    ),
    (
        "radio-paradise",
        "Radio Paradise (MP3 128)",
        "https://stream.radioparadise.com/mp3-128",
    ),
    (
        "bbc-radio-3",
        "BBC Radio 3",
        "https://stream.live.vc.bbcmedia.co.uk/bbc_radio_three",
    ),
    (
        "somafm-groove-salad",
        "SomaFM Groove Salad",
        "https://ice1.somafm.com/groovesalad-128-mp3",
    ),
];

/// Réponse de `GET /api/radios`.
#[derive(Debug, Serialize, Deserialize)]
pub struct RadioList {
    pub favorites: Vec<RadioEntry>,
    pub builtin: Vec<RadioEntry>,
    pub sources: Vec<RadioEntry>,
}

/// Fichier de persistance des favoris (`<data-dir>/radios.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct FavoritesFile {
    #[serde(default)]
    radios: Vec<RadioEntry>,
}

fn favorites_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("radios.json")
}

fn load_favorites(data_dir: &Path) -> Vec<RadioEntry> {
    let Ok(txt) = std::fs::read_to_string(favorites_path(data_dir)) else {
        return Vec::new();
    };
    serde_json::from_str::<FavoritesFile>(&txt)
        .map(|f| f.radios)
        .unwrap_or_default()
}

fn save_favorites(data_dir: &Path, radios: &[RadioEntry]) -> Result<(), AurisError> {
    let f = FavoritesFile {
        radios: radios.to_vec(),
    };
    let txt = serde_json::to_string_pretty(&f)
        .map_err(|e| AurisError::other(format!("radios.json: {e}")))?;
    std::fs::write(favorites_path(data_dir), txt)
        .map_err(|e| AurisError::other(format!("radios.json: {e}")))
}

/// Liste toutes les radios : favoris + intégrées + sources radio de la config.
pub fn list_radios(s: &AppState) -> RadioList {
    let favorites = load_favorites(&s.data_dir);
    let builtin = BUILTIN
        .iter()
        .map(|(id, name, url)| RadioEntry {
            id: format!("builtin-{id}"),
            name: name.to_string(),
            url: url.to_string(),
            origin: RadioOrigin::Builtin,
        })
        .collect();
    let sources = s
        .sources
        .list()
        .into_iter()
        .filter_map(|src| {
            if let SourceKind::Radio { url } = &src.kind {
                Some(RadioEntry {
                    id: format!("src-{}", src.id),
                    name: src.name.clone(),
                    url: url.clone(),
                    origin: RadioOrigin::Source,
                })
            } else {
                None
            }
        })
        .collect();
    RadioList {
        favorites,
        builtin,
        sources,
    }
}

fn find_radio(s: &AppState, id: &str) -> Option<RadioEntry> {
    let l = list_radios(s);
    l.favorites
        .iter()
        .chain(&l.builtin)
        .chain(&l.sources)
        .find(|r| r.id == id)
        .cloned()
}

pub async fn get_radios(State(s): State<AppState>) -> Json<RadioList> {
    Json(list_radios(&s))
}

#[derive(Debug, Deserialize)]
pub struct NewRadio {
    pub name: String,
    pub url: String,
}

pub async fn post_radio(
    State(s): State<AppState>,
    Json(b): Json<NewRadio>,
) -> ApiResult<Json<RadioList>> {
    let name = b.name.trim().to_string();
    let url = b.url.trim().to_string();
    if name.is_empty() {
        return Err(AurisError::InvalidUri("nom de la radio requis".into()).into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(
            AurisError::InvalidUri(format!("URL invalide (http(s) attendu) : {url}")).into(),
        );
    }
    let mut favs = load_favorites(&s.data_dir);
    if favs.iter().any(|r| r.url == url) {
        return Err(AurisError::other("radio déjà dans les favoris").into());
    }
    // id déterministe dérivé de l'URL (les doublons d'URL sont refusés plus haut)
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    favs.push(RadioEntry {
        id: format!("fav-{:016x}", h.finish()),
        name,
        url,
        origin: RadioOrigin::Favorite,
    });
    save_favorites(&s.data_dir, &favs)?;
    Ok(Json(list_radios(&s)))
}

pub async fn delete_radio(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<RadioList>> {
    let mut favs = load_favorites(&s.data_dir);
    let before = favs.len();
    favs.retain(|r| r.id != id);
    if favs.len() == before {
        return Err(
            AurisError::other(format!("radio introuvable parmi les favoris : {id}")).into(),
        );
    }
    save_favorites(&s.data_dir, &favs)?;
    Ok(Json(list_radios(&s)))
}

/// Construit la piste « radio » pour une URL de flux (pas d'upsert bibliothèque).
fn radio_track(name: &str, url: &str) -> Track {
    let source_id = auris_sources::parse_src_uri(url)
        .map(|(id, _)| id)
        .unwrap_or_else(|| "radio".to_string());
    let mut t = Track::new(url.to_string(), source_id);
    t.title = name.to_string();
    t.artist = "Radio web".into();
    t.album = name.to_string();
    t.album_artist = "Radio web".into();
    t.finalize_keys();
    t
}

/// Lance la lecture d'une radio (remplace la file par cette seule piste).
pub async fn post_radio_play(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<PlayerState>> {
    let r = find_radio(&s, &id)
        .ok_or_else(|| AurisError::other(format!("radio introuvable : {id}")))?;
    let t = radio_track(&r.name, &r.url);
    s.engine.play_queue(vec![t], 0);
    Ok(Json(s.engine.state()))
}

// ---------- annuaire radio-browser.info ----------

/// Serveur de l'annuaire public radio-browser.info (miroir de1).
const DIR_BASE: &str = "https://de1.api.radio-browser.info/json";

/// Entrées de l'annuaire exposées à l'UI.
#[derive(Debug, Serialize)]
pub struct DirTag {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct DirCountry {
    pub name: String,
    pub code: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct DirStation {
    pub id: String,
    pub name: String,
    pub url: String,
    pub favicon: String,
    pub bitrate: i64,
    pub codec: String,
    pub tags: String,
    pub country: String,
    pub votes: i64,
}

// formes brutes de l'API radio-browser (champs optionnels -> default)
#[derive(Debug, Deserialize)]
struct RawTag {
    name: String,
    #[serde(default)]
    stationcount: i64,
}

#[derive(Debug, Deserialize)]
struct RawCountry {
    name: String,
    #[serde(default)]
    iso_3166_1: String,
    #[serde(default)]
    stationcount: i64,
}

#[derive(Debug, Deserialize)]
struct RawStation {
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    favicon: String,
    #[serde(default)]
    bitrate: i64,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    votes: i64,
}

/// Cache mémoire des réponses de l'annuaire : chemin -> (timestamp, corps).
static DIR_CACHE: LazyLock<Mutex<HashMap<String, (i64, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Percent-encoding minimal pour les valeurs dans les query strings.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Requête GET bloquante sur l'annuaire (à appeler via `crate::offload_io`).
fn dir_fetch(path: &str) -> auris_core::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            "auris/",
            env!("CARGO_PKG_VERSION"),
            " (radio-browser.info)"
        ))
        .build()
        .map_err(|e| AurisError::other(format!("client radio-browser: {e}")))?;
    let url = format!("{DIR_BASE}{path}");
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| AurisError::other(format!("radio-browser {path}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AurisError::other(format!(
            "radio-browser {path}: HTTP {}",
            resp.status()
        )));
    }
    resp.text()
        .map_err(|e| AurisError::other(format!("radio-browser {path}: {e}")))
}

/// GET sur l'annuaire avec cache mémoire (TTL en secondes).
async fn dir_get(path: &str, ttl: i64) -> ApiResult<String> {
    if let Some((ts, body)) = DIR_CACHE.lock().unwrap().get(path) {
        if now_secs() - *ts < ttl {
            return Ok(body.clone());
        }
    }
    let p = path.to_string();
    let body = crate::offload_io(move || dir_fetch(&p)).await?;
    DIR_CACHE
        .lock()
        .unwrap()
        .insert(path.to_string(), (now_secs(), body.clone()));
    Ok(body)
}

/// `GET /api/radios/dir/genres` — tags triés par nombre de stations.
pub async fn get_dir_genres() -> ApiResult<Json<Vec<DirTag>>> {
    let body = dir_get("/tags?order=stationcount&reverse=true&limit=150", 900).await?;
    let raw: Vec<RawTag> = serde_json::from_str(&body)
        .map_err(|e| AurisError::other(format!("radio-browser tags: {e}")))?;
    Ok(Json(
        raw.into_iter()
            .filter(|t| !t.name.trim().is_empty())
            .map(|t| DirTag {
                name: t.name,
                count: t.stationcount,
            })
            .collect(),
    ))
}

/// `GET /api/radios/dir/countries` — pays triés par nombre de stations.
pub async fn get_dir_countries() -> ApiResult<Json<Vec<DirCountry>>> {
    let body = dir_get("/countries?order=stationcount&reverse=true&limit=250", 900).await?;
    let raw: Vec<RawCountry> = serde_json::from_str(&body)
        .map_err(|e| AurisError::other(format!("radio-browser countries: {e}")))?;
    Ok(Json(
        raw.into_iter()
            .filter(|c| !c.iso_3166_1.trim().is_empty())
            .map(|c| DirCountry {
                name: c.name,
                code: c.iso_3166_1,
                count: c.stationcount,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct DirStationsQuery {
    kind: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/radios/dir/stations?kind=top|popular|genre|country|search&value=…&limit=…`
pub async fn get_dir_stations(
    Query(q): Query<DirStationsQuery>,
) -> ApiResult<Json<Vec<DirStation>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let v = q.value.trim();
    let path = match q.kind.as_str() {
        "top" => format!("/stations/topvote/{limit}?hidebroken=true&order=votes&reverse=true"),
        "popular" => {
            format!("/stations/search?hidebroken=true&order=clickcount&reverse=true&limit={limit}")
        }
        "genre" => {
            if v.is_empty() {
                return Err(AurisError::InvalidUri("value (genre) requis".into()).into());
            }
            format!(
                "/stations/bytagexact/{}?hidebroken=true&order=clickcount&reverse=true&limit={limit}",
                pct(v)
            )
        }
        "country" => {
            if v.is_empty() {
                return Err(AurisError::InvalidUri("value (code pays) requis".into()).into());
            }
            format!(
                "/stations/search?countrycode={}&hidebroken=true&order=clickcount&reverse=true&limit={limit}",
                pct(v)
            )
        }
        "search" => {
            if v.is_empty() {
                return Err(AurisError::InvalidUri("value (recherche) requis".into()).into());
            }
            format!(
                "/stations/search?name={}&hidebroken=true&order=clickcount&reverse=true&limit={limit}",
                pct(v)
            )
        }
        other => {
            return Err(AurisError::InvalidUri(format!(
                "kind inconnu (top|popular|genre|country|search) : {other}"
            ))
            .into());
        }
    };
    let body = dir_get(&path, 300).await?;
    let raw: Vec<RawStation> = serde_json::from_str(&body)
        .map_err(|e| AurisError::other(format!("radio-browser stations: {e}")))?;
    Ok(Json(
        raw.into_iter()
            .filter_map(|s| {
                let url = if s.url_resolved.trim().is_empty() {
                    s.url
                } else {
                    s.url_resolved
                };
                if s.name.trim().is_empty() || url.trim().is_empty() {
                    return None;
                }
                Some(DirStation {
                    id: s.stationuuid,
                    name: s.name,
                    url,
                    favicon: s.favicon,
                    bitrate: s.bitrate,
                    codec: s.codec,
                    tags: s.tags,
                    country: s.country,
                    votes: s.votes,
                })
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct PlayUrlBody {
    pub name: String,
    pub url: String,
}

/// `POST /api/radios/play-url` — lance directement une URL de flux (annuaire…).
pub async fn post_play_url(
    State(s): State<AppState>,
    Json(b): Json<PlayUrlBody>,
) -> ApiResult<Json<PlayerState>> {
    let name = b.name.trim().to_string();
    let url = b.url.trim().to_string();
    if name.is_empty() {
        return Err(AurisError::InvalidUri("nom de la radio requis".into()).into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(
            AurisError::InvalidUri(format!("URL invalide (http(s) attendu) : {url}")).into(),
        );
    }
    let t = radio_track(&name, &url);
    s.engine.play_queue(vec![t], 0);
    Ok(Json(s.engine.state()))
}
