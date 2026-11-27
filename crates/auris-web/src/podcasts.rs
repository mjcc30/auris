//! Podcasts (#81) : recherche via l'API iTunes, flux RSS (crate `rss`),
//! abonnements persistés dans `<data-dir>/podcasts.json`, et lecture des
//! épisodes via le moteur (URL http directe, comme les radios).
//!
//! - `GET    /api/podcasts?q=&limit=`        recherche iTunes (ou abonnements si q vide)
//! - `GET    /api/podcasts/feed?url=`         épisodes d'un flux RSS (cache 300 s)
//! - `GET    /api/podcasts/subscriptions`     abonnements
//! - `POST   /api/podcasts/subscribe`         abonnement {feed_url, title, artwork, author}
//! - `DELETE /api/podcasts/subscriptions/{id}`
//! - `POST   /api/podcasts/play`              lecture d'un épisode {url, title, show, duration_secs}

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

use auris_core::{AurisError, PlayerState, Track};

use crate::{ApiResult, AppState};

/// Une émission de podcast (résultat de recherche iTunes ou abonnement).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastShow {
    pub id: String,
    pub title: String,
    pub author: String,
    pub feed_url: String,
    pub artwork: String,
    pub description: String,
    #[serde(default)]
    pub subscribed: bool,
}

/// Un épisode de podcast (item RSS).
#[derive(Debug, Clone, Serialize)]
pub struct PodcastEpisode {
    pub id: String,
    pub title: String,
    pub url: String,
    pub pub_date: String,
    pub duration_secs: Option<u64>,
    pub description: String,
    pub image: String,
    pub length_bytes: Option<u64>,
}

/// Flux RSS complet d'une émission.
#[derive(Debug, Serialize)]
pub struct PodcastFeed {
    pub title: String,
    pub author: String,
    pub artwork: String,
    pub description: String,
    pub feed_url: String,
    pub episodes: Vec<PodcastEpisode>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SubsFile {
    #[serde(default)]
    shows: Vec<PodcastShow>,
}

fn subs_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("podcasts.json")
}

fn load_subs(data_dir: &Path) -> Vec<PodcastShow> {
    let Ok(txt) = std::fs::read_to_string(subs_path(data_dir)) else {
        return Vec::new();
    };
    serde_json::from_str::<SubsFile>(&txt)
        .map(|f| f.shows)
        .unwrap_or_default()
}

fn save_subs(data_dir: &Path, shows: &[PodcastShow]) -> Result<(), AurisError> {
    let txt = serde_json::to_string_pretty(&SubsFile {
        shows: shows.to_vec(),
    })
    .map_err(|e| AurisError::other(format!("podcasts.json: {e}")))?;
    std::fs::write(subs_path(data_dir), txt)
        .map_err(|e| AurisError::other(format!("podcasts.json: {e}")))
}

fn show_id(feed_url: &str) -> String {
    let mut h = DefaultHasher::new();
    feed_url.hash(&mut h);
    format!("pod-{:016x}", h.finish())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Percent-encoding minimal pour les query strings.
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

/// Cache mémoire des réponses HTTP (url -> (ts, body)).
static POD_CACHE: LazyLock<Mutex<HashMap<String, (i64, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// GET bloquant avec timeouts (à appeler via `crate::offload_io`).
fn http_get(url: &str, ua: &str) -> auris_core::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(ua)
        .build()
        .map_err(|e| AurisError::other(format!("client http: {e}")))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| AurisError::other(format!("GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AurisError::other(format!(
            "GET {url}: HTTP {}",
            resp.status()
        )));
    }
    resp.text()
        .map_err(|e| AurisError::other(format!("GET {url}: {e}")))
}

/// GET avec cache mémoire (TTL en secondes).
fn pod_get_sync(url: &str, ttl: i64) -> auris_core::Result<String> {
    if let Some((ts, body)) = POD_CACHE.lock().unwrap().get(url) {
        if now_secs() - *ts < ttl {
            return Ok(body.clone());
        }
    }
    let body = http_get(
        url,
        concat!("auris/", env!("CARGO_PKG_VERSION"), " (podcasts)"),
    )?;
    POD_CACHE
        .lock()
        .unwrap()
        .insert(url.to_string(), (now_secs(), body.clone()));
    Ok(body)
}

async fn pod_get(url: String, ttl: i64) -> ApiResult<String> {
    crate::offload_io(move || pod_get_sync(&url, ttl)).await
}

// ---------- Recherche iTunes ----------

#[derive(Debug, Deserialize)]
struct ItunesResp {
    #[serde(default)]
    results: Vec<ItunesItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesItem {
    #[serde(default)]
    collection_name: Option<String>,
    #[serde(default)]
    artist_name: Option<String>,
    #[serde(default)]
    feed_url: Option<String>,
    #[serde(default)]
    artwork_url100: Option<String>,
    #[serde(default)]
    artwork_url600: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PodcastsQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/podcasts?q=…&limit=…` — q vide = abonnements ; sinon recherche iTunes.
pub async fn get_podcasts(
    State(s): State<AppState>,
    Query(q): Query<PodcastsQuery>,
) -> ApiResult<Json<Vec<PodcastShow>>> {
    let term = q.q.trim().to_string();
    if term.is_empty() {
        let mut subs = load_subs(&s.data_dir);
        for x in subs.iter_mut() {
            x.subscribed = true;
        }
        return Ok(Json(subs));
    }
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcast&term={}&limit={}",
        pct(&term),
        limit
    );
    let body = pod_get(url, 300).await?;
    let resp: ItunesResp = serde_json::from_str(&body)
        .map_err(|e| AurisError::other(format!("iTunes search: {e}")))?;
    let subs = load_subs(&s.data_dir);
    let sub_urls: std::collections::HashSet<&str> =
        subs.iter().map(|x| x.feed_url.as_str()).collect();
    let shows = resp
        .results
        .into_iter()
        .filter_map(|it| {
            let feed_url = it.feed_url.unwrap_or_default();
            if feed_url.is_empty() {
                return None;
            }
            let title = it.collection_name.unwrap_or_default();
            if title.trim().is_empty() {
                return None;
            }
            let artwork = it.artwork_url600.or(it.artwork_url100).unwrap_or_default();
            Some(PodcastShow {
                id: show_id(&feed_url),
                title,
                author: it.artist_name.unwrap_or_default(),
                artwork,
                description: String::new(),
                subscribed: sub_urls.contains(feed_url.as_str()),
                feed_url,
            })
        })
        .collect();
    Ok(Json(shows))
}

// ---------- Flux RSS ----------

/// Parse une durée iTunes (`HH:MM:SS`, `MM:SS` ou secondes) en secondes.
fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    let mut secs = 0u64;
    for p in parts {
        secs = secs.checked_mul(60)?.checked_add(p.trim().parse().ok()?)?;
    }
    Some(secs)
}

/// `GET /api/podcasts/feed?url=…` — émissions + épisodes d'un flux RSS/Atom.
pub async fn get_feed(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> ApiResult<Json<PodcastFeed>> {
    let url = q.get("url").cloned().unwrap_or_default();
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AurisError::InvalidUri(format!("feed_url invalide : {url}")).into());
    }
    let body = pod_get(url.clone(), 300).await?;
    let feed = parse_feed(&url, &body)?;
    Ok(Json(feed))
}

fn parse_feed(feed_url: &str, body: &str) -> ApiResult<PodcastFeed> {
    let channel = rss::Channel::read_from(body.as_bytes())
        .map_err(|e| AurisError::other(format!("RSS {feed_url}: {e}")))?;
    let itunes_ch = channel.itunes_ext();
    let artwork = channel
        .image()
        .map(|i| i.url.clone())
        .or_else(|| itunes_ch.and_then(|x| x.image.clone()))
        .unwrap_or_default();
    let author = itunes_ch
        .and_then(|x| x.author.clone())
        .or_else(|| channel.managing_editor().map(|s| s.to_string()))
        .unwrap_or_default();
    let description = {
        let d = channel.description().trim();
        if d.is_empty() {
            itunes_ch
                .and_then(|x| x.summary.clone())
                .unwrap_or_default()
        } else {
            d.to_string()
        }
    };
    let episodes = channel
        .items()
        .iter()
        .filter_map(|it| {
            let enc = it.enclosure()?;
            let url = enc.url.trim().to_string();
            if url.is_empty() {
                return None;
            }
            let title = it
                .title()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| url.clone());
            let it_ext = it.itunes_ext();
            let duration_secs = it_ext
                .and_then(|x| x.duration.as_deref())
                .and_then(parse_duration)
                .or_else(|| enc.length.parse::<u64>().ok().filter(|_| false));
            let image = it_ext.and_then(|x| x.image.clone()).unwrap_or_default();
            let description = it
                .description()
                .map(|s| s.trim().to_string())
                .or_else(|| it_ext.and_then(|x| x.summary.clone()))
                .unwrap_or_default();
            Some(PodcastEpisode {
                id: show_id(&url),
                title,
                url,
                pub_date: it.pub_date().unwrap_or("").to_string(),
                duration_secs,
                description,
                image,
                length_bytes: enc.length.parse::<u64>().ok(),
            })
        })
        .collect();
    Ok(PodcastFeed {
        title: channel.title().trim().to_string(),
        author,
        artwork,
        description,
        feed_url: feed_url.to_string(),
        episodes,
    })
}

// ---------- Abonnements ----------

/// `GET /api/podcasts/subscriptions` — émissions suivies.
pub async fn get_subscriptions(State(s): State<AppState>) -> Json<Vec<PodcastShow>> {
    let mut subs = load_subs(&s.data_dir);
    for x in subs.iter_mut() {
        x.subscribed = true;
    }
    Json(subs)
}

#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub feed_url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artwork: String,
    #[serde(default)]
    pub author: String,
}

/// `POST /api/podcasts/subscribe` — suit une émission (idempotent sur le feed_url).
pub async fn post_subscribe(
    State(s): State<AppState>,
    Json(b): Json<SubscribeBody>,
) -> ApiResult<Json<Vec<PodcastShow>>> {
    let feed_url = b.feed_url.trim().to_string();
    if !(feed_url.starts_with("http://") || feed_url.starts_with("https://")) {
        return Err(AurisError::InvalidUri(format!("feed_url invalide : {feed_url}")).into());
    }
    let mut subs = load_subs(&s.data_dir);
    let id = show_id(&feed_url);
    let title = if b.title.trim().is_empty() {
        feed_url.clone()
    } else {
        b.title.trim().to_string()
    };
    let show = PodcastShow {
        id,
        title,
        author: b.author.trim().to_string(),
        feed_url,
        artwork: b.artwork.trim().to_string(),
        description: String::new(),
        subscribed: true,
    };
    subs.retain(|x| x.id != show.id);
    subs.insert(0, show);
    save_subs(&s.data_dir, &subs)?;
    Ok(Json(subs))
}

/// `DELETE /api/podcasts/subscriptions/{id}` — retire un abonnement.
pub async fn delete_subscription(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<Vec<PodcastShow>>> {
    let mut subs = load_subs(&s.data_dir);
    let before = subs.len();
    subs.retain(|x| x.id != id);
    if subs.len() == before {
        return Err(AurisError::other(format!("abonnement introuvable : {id}")).into());
    }
    save_subs(&s.data_dir, &subs)?;
    Ok(Json(subs))
}

// ---------- Lecture ----------

#[derive(Debug, Deserialize)]
pub struct PlayEpisodeBody {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub show: String,
    #[serde(default)]
    pub duration_secs: Option<f64>,
}

/// `POST /api/podcasts/play` — lance un épisode (remplace la file par cette piste).
pub async fn post_play(
    State(s): State<AppState>,
    Json(b): Json<PlayEpisodeBody>,
) -> ApiResult<Json<PlayerState>> {
    let url = b.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AurisError::InvalidUri(format!("URL invalide : {url}")).into());
    }
    let title = if b.title.trim().is_empty() {
        url.clone()
    } else {
        b.title.trim().to_string()
    };
    let show = if b.show.trim().is_empty() {
        "Podcast".to_string()
    } else {
        b.show.trim().to_string()
    };
    let mut t = Track::new(url, "podcast".to_string());
    t.title = title;
    t.artist = show.clone();
    t.album = show;
    t.album_artist = "Podcast".into();
    t.duration_secs = b.duration_secs.filter(|d| *d > 0.0);
    t.genre = Some("Podcast".into());
    t.finalize_keys();
    s.engine.play_queue(vec![t], 0);
    Ok(Json(s.engine.state()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_formats() {
        assert_eq!(parse_duration("3600"), Some(3600));
        assert_eq!(parse_duration("01:00:00"), Some(3600));
        assert_eq!(parse_duration("12:34"), Some(754));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn parse_feed_rss() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>Mon Podcast</title>
    <description>Un test</description>
    <image><url>https://example.com/cover.jpg</url></image>
    <itunes:author>Auteur</itunes:author>
    <item>
      <title>Épisode 1</title>
      <pubDate>Mon, 01 Jan 2026 10:00:00 GMT</pubDate>
      <description>Premier épisode</description>
      <enclosure url="https://example.com/ep1.mp3" length="123456" type="audio/mpeg"/>
      <itunes:duration>00:10:00</itunes:duration>
      <itunes:image href="https://example.com/ep1.jpg"/>
    </item>
    <item>
      <title>Épisode 2</title>
      <enclosure url="https://example.com/ep2.mp3" length="42" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"##;
        let feed = parse_feed("https://example.com/feed.xml", xml)
            .unwrap_or_else(|_| panic!("parse_feed a echoue"));
        assert_eq!(feed.title, "Mon Podcast");
        assert_eq!(feed.author, "Auteur");
        assert_eq!(feed.artwork, "https://example.com/cover.jpg");
        assert_eq!(feed.episodes.len(), 2);
        assert_eq!(feed.episodes[0].title, "Épisode 1");
        assert_eq!(feed.episodes[0].url, "https://example.com/ep1.mp3");
        assert_eq!(feed.episodes[0].duration_secs, Some(600));
        assert_eq!(feed.episodes[0].image, "https://example.com/ep1.jpg");
        assert_eq!(feed.episodes[0].length_bytes, Some(123456));
        assert_eq!(feed.episodes[1].duration_secs, None);
    }

    #[test]
    fn show_id_stable() {
        assert_eq!(show_id("https://x/feed"), show_id("https://x/feed"));
        assert_ne!(show_id("https://x/a"), show_id("https://x/b"));
    }
}

#[cfg(test)]
mod itunes_parse_tests {
    #[test]
    fn itunes_camelcase_fields_parse() {
        let body = r#"{"resultCount":1,"results":[{"wrapperType":"track","kind":"podcast","collectionName":"Le code a changé","artistName":"France Inter","feedUrl":"https://radiofrance-podcast.net/podcast09/rss_12621.xml","artworkUrl100":"https://x/100.png","artworkUrl600":"https://x/600.png"}]}"#;
        let resp: super::ItunesResp = serde_json::from_str(body).unwrap();
        assert_eq!(resp.results.len(), 1);
        let it = &resp.results[0];
        assert_eq!(it.collection_name.as_deref(), Some("Le code a changé"));
        assert_eq!(
            it.feed_url.as_deref(),
            Some("https://radiofrance-podcast.net/podcast09/rss_12621.xml")
        );
        assert_eq!(it.artwork_url600.as_deref(), Some("https://x/600.png"));
    }
}
