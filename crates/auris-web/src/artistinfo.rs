//! Page artiste enrichie façon Roon (#102) :
//! - MusicBrainz (gratuit, rate-limit 1 req/s côté `lastfm::MbClient`) : MBID,
//!   fiche artiste (tags, note communautaire, relations dont Wikipedia),
//!   discographie (release-groups + pochettes Cover Art Archive), artistes
//!   similaires (artistes partageant le tag dominant) ;
//! - bio + image Wikipedia (cache sled `bio_get`/`bio_set` partagé avec
//!   `/api/infos`) ;
//! - galerie : image Deezer + image Wikipedia + pochettes CAA des albums MB.
//!
//! Réponses mises en cache en mémoire 10 min (négatifs inclus), clé = nom
//! d'artiste normalisé. Si MusicBrainz est désactivé dans les réglages, on
//! renvoie quand même bio + images.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};

use auris_core::AurisError;

use crate::{lastfm, ApiResult, AppState};

const TTL_SECS: i64 = 600;
/// Nombre de pochettes CAA proposées dans la galerie.
const GALLERY_CAA: usize = 6;

static CACHE: OnceLock<Mutex<HashMap<String, (i64, Value)>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, (i64, Value)>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// GET /api/library/artist_info/{artist_id}
pub async fn get_artist_info(
    State(s): State<AppState>,
    Path(artist_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let (artist, _albums) = s
        .library
        .artist(&artist_id)
        .ok_or_else(|| AurisError::ArtistNotFound(artist_id.clone()))?;
    let key = norm(&artist.name);
    let now = chrono::Utc::now().timestamp();
    if let Some((ts, v)) = cache().lock().unwrap().get(&key) {
        if now - *ts < TTL_SECS {
            return Ok(Json(v.clone()));
        }
    }

    let mut info = json!({
        "artist_id": artist_id,
        "name": artist.name,
        "mbid": Value::Null,
        "mb": Value::Null,
        "similar": Vec::<Value>::new(),
        "discography": Vec::<Value>::new(),
        "bio": Value::Null,
        "bio_image": Value::Null,
        "images": Vec::<Value>::new(),
        "source": "none",
    });

    // --- MusicBrainz ---
    if lastfm::mb_enabled(&s) {
        let mb = lastfm::mb_cache(&s);
        let search = match mb.client.artist_search(&artist.name).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("musicbrainz artist_search {:?}: {e}", artist.name);
                Value::Null
            }
        };
        if let Some((mbid, _score)) = pick_artist(&search, &artist.name) {
            info["mbid"] = json!(mbid);
            if let Ok(fiche) = mb.client.artist(&mbid).await {
                if !fiche.is_null() {
                    let sum = summarize_artist(&fiche);
                    // tag dominant -> artistes similaires
                    if let Some(tag) = sum
                        .get("tags")
                        .and_then(|t| t.get(0))
                        .and_then(|t| t.get("name"))
                        .and_then(|n| n.as_str())
                        .map(str::to_string)
                    {
                        match mb.client.artists_by_tag(&tag).await {
                            Ok(v) => {
                                info["similar"] = json!(summarize_similar(&v, &artist.name));
                            }
                            Err(e) => tracing::warn!("musicbrainz artists_by_tag: {e}"),
                        }
                    }
                    info["mb"] = sum;
                }
            }
            if let Ok(rgs) = mb.client.release_groups(&mbid).await {
                info["discography"] = json!(summarize_release_groups(&rgs));
            }
            info["source"] = json!("musicbrainz");
        }
    }

    // --- bio + image Wikipedia (cache sled partagé avec /api/infos) ---
    let bio_key = artist.name.to_lowercase();
    let (bio, bio_image) = match s.library.bio_get(&bio_key) {
        Some(b) => (Some(b), None),
        None => match wikipedia_summary(&s, &artist.name).await {
            Some((text, img)) => {
                let _ = s.library.bio_set(&bio_key, &text);
                (Some(text), img)
            }
            None => (None, None),
        },
    };
    if let Some(b) = bio {
        info["bio"] = json!(b);
        info["source"] = if info["source"] == "none" {
            json!("wikipedia")
        } else {
            info["source"].clone()
        };
    }
    if let Some(img) = bio_image {
        info["bio_image"] = json!(img);
    }

    // --- galerie : Deezer + Wikipedia + pochettes CAA des release-groups ---
    let mut images: Vec<String> = Vec::new();
    if let Some(u) = crate::artwork::artist_image_url(&artist.name).await {
        images.push(u);
    }
    if let Some(u) = info["bio_image"].as_str() {
        images.push(u.to_string());
    }
    if let Some(disc) = info["discography"].as_array() {
        for rg in disc.iter().take(GALLERY_CAA) {
            if let Some(c) = rg.get("cover").and_then(|c| c.as_str()) {
                images.push(c.to_string());
            }
        }
    }
    images.dedup();
    info["images"] = json!(images);

    if let Ok(mut m) = cache().lock() {
        m.insert(key, (now, info.clone()));
    }
    Ok(Json(info))
}

/// Résumé Wikipedia (fr puis en) : (extrait, image originale si présente).
async fn wikipedia_summary(s: &AppState, name: &str) -> Option<(String, Option<String>)> {
    let enc = lastfm::urlencode(name);
    for lang in ["fr", "en"] {
        let url = format!("https://{lang}.wikipedia.org/api/rest_v1/page/summary/{enc}");
        if let Some(body) = crate::http_get_async(s, &url).await {
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                let text = v
                    .get("extract")
                    .and_then(|x| x.as_str())
                    .filter(|x| !x.trim().is_empty())
                    .map(str::to_string);
                let img = v
                    .get("originalimage")
                    .and_then(|o| o.get("source"))
                    .and_then(|x| x.as_str())
                    .or_else(|| {
                        v.get("thumbnail")
                            .and_then(|t| t.get("source"))
                            .and_then(|x| x.as_str())
                    })
                    .map(str::to_string);
                if let Some(t) = text {
                    return Some((t, img));
                }
            }
        }
    }
    None
}

/// Choisit le meilleur artiste d'une recherche MB par nom (correspondance
/// normalisée d'abord, sinon premier résultat avec son score).
pub fn pick_artist(search: &Value, name: &str) -> Option<(String, f64)> {
    let arr = search.get("artists")?.as_array()?;
    let want = norm(name);
    let best = arr
        .iter()
        .find(|a| {
            a.get("name").and_then(|n| n.as_str()).map(norm).as_deref() == Some(want.as_str())
        })
        .or_else(|| arr.first())?;
    let mbid = best.get("id")?.as_str()?.to_string();
    let score = best.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
    Some((mbid, score))
}

/// Fiche artiste « propre » extraite de la réponse WS2 `/artist/{mbid}`.
pub fn summarize_artist(v: &Value) -> Value {
    if v.is_null() {
        return Value::Null;
    }
    let tags: Vec<Value> = v
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            let mut v: Vec<Value> = a
                .iter()
                .filter_map(|t| {
                    let name = t.get("name")?.as_str()?;
                    let count = t.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                    Some(json!({ "name": name, "count": count }))
                })
                .collect();
            v.sort_by_key(|t| {
                std::cmp::Reverse(t.get("count").and_then(|c| c.as_u64()).unwrap_or(0))
            });
            v.truncate(10);
            v
        })
        .unwrap_or_default();
    let genres: Vec<String> = v
        .get("genres")
        .and_then(|g| g.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|g| g.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let urls: Vec<Value> = v
        .get("relations")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter(|r| r.get("target-type").and_then(|t| t.as_str()) == Some("url"))
                .filter_map(|r| {
                    let url = r
                        .get("url")
                        .and_then(|u| u.get("resource"))
                        .and_then(|x| x.as_str())?;
                    let rel = r.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    Some(json!({ "type": rel, "url": url }))
                })
                .collect()
        })
        .unwrap_or_default();
    let wikipedia = urls
        .iter()
        .find(|u| u.get("type").and_then(|t| t.as_str()) == Some("wikipedia"))
        .and_then(|u| u.get("url").cloned());
    let life = v.get("life-span");
    json!({
        "mbid": v.get("id").cloned().unwrap_or(Value::Null),
        "name": v.get("name").cloned().unwrap_or(Value::Null),
        "sort_name": v.get("sort-name").cloned().unwrap_or(Value::Null),
        "type": v.get("type").cloned().unwrap_or(Value::Null),
        "country": v.get("country").cloned().unwrap_or(Value::Null),
        "area": v.get("area").and_then(|a| a.get("name")).cloned().unwrap_or(Value::Null),
        "begin": life.and_then(|l| l.get("begin")).cloned().unwrap_or(Value::Null),
        "end": life.and_then(|l| l.get("end")).cloned().unwrap_or(Value::Null),
        "disambiguation": v.get("disambiguation").cloned().unwrap_or(Value::Null),
        "tags": tags,
        "genres": genres,
        "rating": v.get("rating").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null),
        "rating_count": v.get("rating-count").cloned().unwrap_or(Value::Null),
        "urls": urls,
        "wikipedia": wikipedia.unwrap_or(Value::Null),
        "url": v.get("id").and_then(|i| i.as_str()).map(|i| format!("https://musicbrainz.org/artist/{i}")),
    })
}

/// Release-groups -> liste propre, avec pochette Cover Art Archive (front-250).
pub fn summarize_release_groups(v: &Value) -> Vec<Value> {
    let Some(arr) = v.get("release-groups").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = arr
        .iter()
        .filter_map(|rg| {
            let mbid = rg.get("id")?.as_str()?.to_string();
            Some(json!({
                "mbid": mbid,
                "title": rg.get("title").cloned().unwrap_or(Value::Null),
                "date": rg.get("first-release-date").cloned().unwrap_or(Value::Null),
                "primary_type": rg.get("primary-type").cloned().unwrap_or(Value::Null),
                "secondary_types": rg.get("secondary-types").cloned().unwrap_or(json!([])),
                // 404 si pas de pochette : l'UI a son fallback (Deezer / icône)
                "cover": format!("https://coverartarchive.org/release-group/{mbid}/front-250"),
            }))
        })
        .collect();
    out.sort_by_key(|rg| {
        rg.get("date")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string()
    });
    out.reverse(); // plus récent d'abord
    out
}

/// Artistes d'une recherche par tag -> {name, mbid, score}, soi-même exclu.
pub fn summarize_similar(v: &Value, exclude: &str) -> Vec<Value> {
    let Some(arr) = v.get("artists").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    let want = norm(exclude);
    arr.iter()
        .filter(|a| {
            a.get("name").and_then(|n| n.as_str()).map(norm).as_deref() != Some(want.as_str())
        })
        .map(|a| {
            json!({
                "name": a.get("name").cloned().unwrap_or(Value::Null),
                "mbid": a.get("id").cloned().unwrap_or(Value::Null),
                "score": a.get("score").cloned().unwrap_or(Value::Null),
            })
        })
        .take(10)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_artist_prefere_nom_exact() {
        let body = json!({ "artists": [
            { "id": "1", "name": "2Pac Tribute", "score": 100 },
            { "id": "2", "name": "2Pac", "score": 90 }
        ]});
        assert_eq!(pick_artist(&body, "2pac").map(|x| x.0), Some("2".into()));
        // sans correspondance exacte : premier résultat
        assert_eq!(pick_artist(&body, "pac").map(|x| x.0), Some("1".into()));
        assert_eq!(pick_artist(&json!({}), "x"), None);
    }

    #[test]
    fn summarize_artist_extrait_fiche() {
        let v = json!({
            "id": "mbid-1",
            "name": "Daft Punk",
            "sort-name": "Daft Punk",
            "type": "Group",
            "country": "FR",
            "area": { "name": "France" },
            "life-span": { "begin": "1993", "end": null },
            "tags": [
                { "name": "electronic", "count": 42 },
                { "name": "house", "count": 10 },
                { "name": "french", "count": 5 }
            ],
            "genres": [{ "name": "electronic" }],
            "rating": { "value": 4.5 },
            "rating-count": 12,
            "relations": [
                { "type": "wikipedia", "target-type": "url", "url": { "resource": "https://fr.wikipedia.org/wiki/Daft_Punk" } },
                { "type": "official homepage", "target-type": "url", "url": { "resource": "https://daftpunk.com" } },
                { "type": "collaboration", "target-type": "artist", "artist": { "id": "x" } }
            ]
        });
        let s = summarize_artist(&v);
        assert_eq!(s["mbid"], "mbid-1");
        assert_eq!(s["country"], "FR");
        assert_eq!(s["area"], "France");
        assert_eq!(s["begin"], "1993");
        assert_eq!(s["tags"][0], json!({ "name": "electronic", "count": 42 }));
        assert_eq!(s["rating"], 4.5);
        assert_eq!(s["rating_count"], 12);
        // seules les relations url sont gardées
        assert_eq!(s["urls"].as_array().unwrap().len(), 2);
        assert_eq!(s["wikipedia"], "https://fr.wikipedia.org/wiki/Daft_Punk");
        assert_eq!(s["url"], "https://musicbrainz.org/artist/mbid-1");
        assert_eq!(summarize_artist(&Value::Null), Value::Null);
    }

    #[test]
    fn summarize_release_groups_trie_par_date() {
        let v = json!({ "release-groups": [
            { "id": "a", "title": "Old", "first-release-date": "1997-03-04", "primary-type": "Album" },
            { "id": "b", "title": "New", "first-release-date": "2013-12-09", "primary-type": "Album",
              "secondary-types": ["Remaster"] }
        ]});
        let out = summarize_release_groups(&v);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["title"], "New"); // plus récent d'abord
        assert_eq!(
            out[0]["cover"],
            "https://coverartarchive.org/release-group/b/front-250"
        );
        assert_eq!(out[1]["secondary_types"], json!([]));
        assert!(summarize_release_groups(&json!({})).is_empty());
    }

    #[test]
    fn summarize_similar_exclut_soi_meme() {
        let v = json!({ "artists": [
            { "id": "1", "name": "Daft Punk", "score": 100 },
            { "id": "2", "name": "Justice", "score": 80 }
        ]});
        let out = summarize_similar(&v, "daft punk");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "Justice");
        assert!(summarize_similar(&json!({}), "x").is_empty());
    }
}
