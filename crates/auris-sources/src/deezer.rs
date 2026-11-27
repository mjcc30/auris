//! Source Deezer — #85 : extraits 30 s via l'API publique `api.deezer.com`,
//! sans clé ni compte (CORS ouverte). La lecture joue le champ `preview`
//! (MP3 30 s) via le lecteur HTTP seekable.
//!
//! - Recherche : `GET api.deezer.com/search?q=<q>`
//! - Racine : top du chart `GET api.deezer.com/chart/0/tracks`
//! - Lecture : `GET api.deezer.com/track/<id>` -> champ `preview`.

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};
use serde_json::Value;

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

const API: &str = "https://api.deezer.com";

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("auris/", env!("CARGO_PKG_VERSION"), " (deezer)"))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

fn get_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| x.as_u64())
}

/// Une piste Deezer = une entrée jouable (`track/<id>`), extrait 30 s à l'ouverture.
fn track_entry(src_id: &str, t: &Value) -> Option<BrowseEntry> {
    let id = get_u64(t, "id")?.to_string();
    let title = get_str(t, "title").unwrap_or("Piste");
    let artist = t
        .get("artist")
        .and_then(|a| get_str(a, "name"))
        .unwrap_or("");
    let name = if artist.is_empty() {
        title.to_string()
    } else {
        format!("{artist} — {title}")
    };
    Some(BrowseEntry {
        name,
        uri: make_uri(src_id, &format!("track/{id}")),
        is_dir: false,
        size: None,
        format: None,
        is_audio: true,
    })
}

fn get_json(url: &str) -> Result<Value> {
    let resp = http()
        .get(url)
        .send()
        .map_err(|e| AurisError::other(format!("deezer GET {url}: {e}")))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| AurisError::other(format!("deezer GET {url} (json): {e}")))?;
    if !status.is_success() {
        return Err(AurisError::other(format!(
            "deezer GET {url}: HTTP {status} {}",
            body.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("")
        )));
    }
    Ok(body)
}

fn tracks_of(body: &Value) -> Vec<Value> {
    // /search -> {"data": [...]} ; /chart/0/tracks -> {"data": [...]} ; /track/<id> n'a pas de data
    body.get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default()
}

fn search_sync(src_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let body = get_json(&format!("{API}/search?q={}&limit=30", pct(q)))?;
    Ok(tracks_of(&body)
        .iter()
        .filter_map(|t| track_entry(src_id, t))
        .collect())
}

fn list_sync(src_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        // racine = top du chart mondial
        "" => {
            let body = get_json(&format!("{API}/chart/0/tracks?limit=50"))?;
            Ok(tracks_of(&body)
                .iter()
                .filter_map(|t| track_entry(src_id, t))
                .collect())
        }
        other => {
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(src_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin deezer inconnu: {rel_path}"
            )))
        }
    }
}

/// Source Deezer : chart + recherche + lecture des extraits 30 s.
pub struct DeezerSource {
    source: Source,
    http: HttpSource,
}

impl DeezerSource {
    pub fn new(source: Source) -> Result<Self> {
        if !matches!(source.kind, SourceKind::Deezer) {
            return Err(AurisError::other("source non deezer"));
        }
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(DeezerSource { source, http })
    }
}

#[async_trait]
impl SourceBackend for DeezerSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&src_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("deezer list (join): {e}")))?
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let rel = rel_path.trim_matches('/').to_string();
        let track_id = rel
            .strip_prefix("track/")
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?;
        let body = tokio::task::spawn_blocking({
            let id = track_id.to_string();
            move || get_json(&format!("{API}/track/{id}"))
        })
        .await
        .map_err(|e| AurisError::other(format!("deezer open (join): {e}")))??;
        let preview = get_str(&body, "preview")
            .ok_or_else(|| AurisError::other("deezer: pas d'extrait (preview) pour ce titre"))?;
        Ok(Box::new(self.http.open_url(preview)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&src_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("deezer search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn track_entry_nom_artiste_titre() {
        let t = json!({"id": 3135556, "title": "Harder, Better, Faster, Stronger", "artist": {"name": "Daft Punk"}});
        let e = track_entry("dz", &t).unwrap();
        assert!(e.is_audio);
        assert!(!e.is_dir);
        assert_eq!(e.name, "Daft Punk — Harder, Better, Faster, Stronger");
        assert_eq!(e.uri, "src://dz/track/3135556");
    }

    #[test]
    fn track_entry_sans_id_rejetee() {
        assert!(track_entry("dz", &json!({"title": "x"})).is_none());
    }

    #[test]
    fn tracks_of_data() {
        let body = json!({"data": [{"id": 1, "title": "a"}, {"id": 2, "title": "b"}]});
        assert_eq!(tracks_of(&body).len(), 2);
        assert!(tracks_of(&json!({"error": {}})).is_empty());
    }

    #[test]
    fn source_kind_serde() {
        let src: Source =
            serde_json::from_str(r#"{"id":"d","name":"Deezer","enabled":true,"kind":"deezer"}"#)
                .unwrap();
        assert!(matches!(src.kind, SourceKind::Deezer));
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"deezer""#), "{s}");
    }

    #[test]
    fn racine_ou_chemin_inconnu() {
        // "" passera par le réseau dans list_sync ; on teste juste le routage d'erreur
        assert!(list_sync("dz", "nimportequoi").is_err());
    }
}
