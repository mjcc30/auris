//! Images externes — artistes & pochettes d'albums — via l'API publique Deezer
//! (sans clé). Repli quand les tags des fichiers n'embarquent pas d'artwork :
//! l'UI tente d'abord `/api/library/cover/{track_id}` (pochette embarquée), puis
//! ces endpoints (redirection 303 vers le CDN Deezer), puis une icône.
//!
//! Résultats mis en cache en mémoire (clé = requête normalisée), y compris les
//! négatifs, pour ne pas spammer l'API à chaque rendu de grille.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::AppState;

static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(concat!(
            "auris/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/mjcc30/auris)"
        ))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Normalise pour la comparaison : casse + espaces.
fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `picture_xl` du meilleur artiste correspondant au nom demandé.
pub fn parse_artist_image(body: &Value, name: &str) -> Option<String> {
    let data = body.get("data")?.as_array()?;
    if data.is_empty() {
        return None;
    }
    let want = norm(name);
    let pick = data
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()).map(norm) == Some(want.clone()))
        .or_else(|| data.first())?;
    pick.get("picture_xl")
        .or_else(|| pick.get("picture_big"))
        .or_else(|| pick.get("picture_medium"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// `cover_xl` du meilleur album correspondant au titre + artiste demandés.
pub fn parse_album_image(body: &Value, title: &str, artist: &str) -> Option<String> {
    let data = body.get("data")?.as_array()?;
    if data.is_empty() {
        return None;
    }
    let want_t = norm(title);
    let want_a = norm(artist);
    let title_match =
        |a: &Value| a.get("title").and_then(|t| t.as_str()).map(norm) == Some(want_t.clone());
    let artist_match = |a: &Value| {
        let ar = a
            .get("artist")
            .and_then(|x| x.get("name"))
            .and_then(|n| n.as_str())
            .map(norm);
        match ar {
            Some(ar) => {
                want_a.is_empty()
                    || ar == want_a
                    || ar.starts_with(&want_a)
                    || want_a.starts_with(&ar)
            }
            None => want_a.is_empty(),
        }
    };
    let pick = data
        .iter()
        .find(|a| title_match(a) && artist_match(a))
        .or_else(|| data.iter().find(|a| title_match(a)))
        .or_else(|| data.first())?;
    pick.get("cover_xl")
        .or_else(|| pick.get("cover_big"))
        .or_else(|| pick.get("cover_medium"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

async fn fetch_parse(url: &str, f: impl FnOnce(&Value) -> Option<String>) -> Option<String> {
    let resp = http().get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    f(&body)
}

/// Image d'un artiste par nom (cache mémoire, négatif inclus).
pub async fn artist_image_url(name: &str) -> Option<String> {
    let key = format!("artist:{}", norm(name));
    if let Some(v) = cache().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return v;
    }
    let url = format!(
        "https://api.deezer.com/search/artist?q={}&limit=8",
        urlencode(name)
    );
    let res = fetch_parse(&url, |b| parse_artist_image(b, name)).await;
    if let Ok(mut m) = cache().lock() {
        m.insert(key, res.clone());
    }
    res
}

/// Pochette d'un album par titre + artiste (cache mémoire, négatif inclus).
pub async fn album_image_url(title: &str, artist: &str) -> Option<String> {
    let key = format!("album:{}|{}", norm(title), norm(artist));
    if let Some(v) = cache().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return v;
    }
    // Requête texte simple : la syntaxe avancée album:"" artist:"" renvoie une
    // liste vide via l'API Deezer. Le filtrage fin (titre exact + artiste) est
    // fait dans parse_album_image.
    let q = format!("{} {}", title.replace('"', " "), artist.replace('"', " "));
    let url = format!(
        "https://api.deezer.com/search/album?q={}&limit=8",
        urlencode(&q)
    );
    let res = fetch_parse(&url, |b| parse_album_image(b, title, artist)).await;
    if let Ok(mut m) = cache().lock() {
        m.insert(key, res.clone());
    }
    res
}

/// `GET /api/library/artist_image/{artist_id}` — redirection vers la photo de
/// l'artiste (Deezer), 404 si rien trouvé.
pub async fn get_artist_image(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let Some((a, _)) = s.library.artist(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "artiste inconnu" })),
        )
            .into_response();
    };
    match artist_image_url(&a.name).await {
        Some(url) => Redirect::to(&url).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "pas d'image externe" })),
        )
            .into_response(),
    }
}

/// `GET /api/artist_image/name/{name}` — redirection vers la photo d'un artiste
/// par nom (Deezer), utile pour les artistes « similaires » qui ne sont pas
/// dans la bibliothèque (#102).
pub async fn get_artist_image_by_name(Path(name): Path<String>) -> Response {
    match artist_image_url(&name).await {
        Some(url) => Redirect::to(&url).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "pas d'image externe" })),
        )
            .into_response(),
    }
}

/// `GET /api/library/album_image/{album_id}` — redirection vers la pochette de
/// l'album (Deezer), 404 si rien trouvé.
pub async fn get_album_image(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let Some((a, _)) = s.library.album(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "album inconnu" })),
        )
            .into_response();
    };
    match album_image_url(&a.title, &a.artist).await {
        Some(url) => Redirect::to(&url).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "pas de pochette externe" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_image_prefere_correspondance_exacte() {
        let body = json!({ "data": [
            { "name": "2Pac Tribute", "picture_xl": "https://x/tribute.jpg" },
            { "name": "2Pac", "picture_xl": "https://x/2pac.jpg", "picture_big": "https://x/2pac-big.jpg" }
        ]});
        assert_eq!(
            parse_artist_image(&body, "2pac").as_deref(),
            Some("https://x/2pac.jpg")
        );
        // sans correspondance exacte : premier résultat + fallback picture_big
        let body2 = json!({ "data": [ { "name": "50 Cent", "picture_big": "https://x/50.jpg" } ]});
        assert_eq!(
            parse_artist_image(&body2, "50 cent").as_deref(),
            Some("https://x/50.jpg")
        );
        assert_eq!(parse_artist_image(&json!({"data": []}), "x"), None);
    }

    #[test]
    fn album_image_filtre_titre_et_artiste() {
        let body = json!({ "data": [
            { "title": "21", "artist": { "name": "Someone Else" }, "cover_xl": "https://x/wrong.jpg" },
            { "title": "21", "artist": { "name": "Adele" }, "cover_xl": "https://x/21.jpg" }
        ]});
        assert_eq!(
            parse_album_image(&body, "21", "Adele").as_deref(),
            Some("https://x/21.jpg")
        );
        // artiste partiel accepté
        assert_eq!(
            parse_album_image(&body, "21", "ad").as_deref(),
            Some("https://x/21.jpg")
        );
        // titre seul si aucun artiste ne matche
        let body2 = json!({ "data": [ { "title": "Rocks", "artist": { "name": "X" }, "cover_big": "https://x/rocks.jpg" } ]});
        assert_eq!(
            parse_album_image(&body2, "rocks", "Prism").as_deref(),
            Some("https://x/rocks.jpg")
        );
        assert_eq!(parse_album_image(&json!({"data": []}), "a", "b"), None);
    }

    #[test]
    fn urlencode_basique() {
        assert_eq!(urlencode("AC/DC"), "AC%2FDC");
        assert_eq!(urlencode("Beyoncé"), "Beyonc%C3%A9");
        assert_eq!(urlencode("a b"), "a%20b");
    }
}
