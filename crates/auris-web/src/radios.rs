//! Radios web : favoris persistés (`<data-dir>/radios.json`), radios intégrées
//! à Auris et sources `kind = "radio"` de la configuration, + lecture directe.
//!
//! La lecture d'une radio passe par l'URI HTTP du flux : `SourcesManager`
//! bascule automatiquement sur le lecteur de flux live quand il n'y a pas de
//! `Content-Length` (voir `auris_sources::live`), et le moteur marque la piste
//! `is_live` (durée inconnue, lecture infinie, métadonnées ICY `StreamTitle`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use axum::{
    extract::{Path as AxumPath, State},
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

/// Lance la lecture d'une radio (remplace la file par cette seule piste).
pub async fn post_radio_play(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<PlayerState>> {
    let r = find_radio(&s, &id)
        .ok_or_else(|| AurisError::other(format!("radio introuvable : {id}")))?;
    let source_id = auris_sources::parse_src_uri(&r.url)
        .map(|(id, _)| id)
        .unwrap_or_else(|| "radio".to_string());
    let mut t = Track::new(r.url.clone(), source_id);
    t.title = r.name.clone();
    t.artist = "Radio web".into();
    t.album = r.name;
    t.album_artist = "Radio web".into();
    t.finalize_keys();
    // pas d'upsert bibliothèque : une radio n'est pas un fichier scanné
    s.engine.play_queue(vec![t], 0);
    Ok(Json(s.engine.state()))
}
