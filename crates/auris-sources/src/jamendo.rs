//! Source Jamendo — #86 : musique Creative Commons via l'API v3.0
//! `api.jamendo.com/v3.0` (nécessite un `client_id`, inscription gratuite sur
//! developer.jamendo.com). Sans `client_id` configuré, la source refuse de
//! lister/chercher avec un message clair.
//!
//! - Racine : titres populaires du mois `GET /v3.0/tracks/?order=popularity_month`
//! - Recherche : `search/<q>` -> `GET /v3.0/tracks/?search=<q>`
//! - Lecture : `track/<id>` -> `GET /v3.0/tracks/?id=<id>` -> champ `audio`
//!   (flux MP3 complet, lisible via le lecteur HTTP seekable).

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};
use serde_json::Value;

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

const API: &str = "https://api.jamendo.com/v3.0";

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("auris/", env!("CARGO_PKG_VERSION"), " (jamendo)"))
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

fn missing_client_id() -> AurisError {
    AurisError::other(
        "Jamendo: client_id manquant — inscription gratuite sur developer.jamendo.com, \
         puis `client_id = \"…\"` dans la source (kind = \"jamendo\") de config.toml",
    )
}

/// Une piste Jamendo = une entrée jouable (`track/<id>`), flux MP3 complet à l'ouverture.
fn track_entry(src_id: &str, t: &Value) -> Option<BrowseEntry> {
    let id = get_str(t, "id")?;
    let title = get_str(t, "name").unwrap_or("Piste");
    let artist = get_str(t, "artist_name").unwrap_or("");
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

fn get_json(client_id: &str, url: &str) -> Result<Value> {
    let url = format!("{url}&client_id={}", pct(client_id));
    let resp = http()
        .get(&url)
        .send()
        .map_err(|e| AurisError::other(format!("jamendo GET {url}: {e}")))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| AurisError::other(format!("jamendo GET {url} (json): {e}")))?;
    if !status.is_success() {
        return Err(AurisError::other(format!(
            "jamendo GET {url}: HTTP {status} {}",
            body.get("error").and_then(|m| m.as_str()).unwrap_or("")
        )));
    }
    if let Some(err) = body.get("error").and_then(|e| e.as_str()) {
        if !err.is_empty() {
            return Err(AurisError::other(format!("jamendo: {err}")));
        }
    }
    Ok(body)
}

fn results_of(body: &Value) -> Vec<Value> {
    // /v3.0/* -> {"headers": …, "results": [...]}
    body.get("results")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default()
}

fn search_sync(src_id: &str, client_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let body = get_json(
        client_id,
        &format!("{API}/tracks/?format=json&limit=30&search={}", pct(q)),
    )?;
    Ok(results_of(&body)
        .iter()
        .filter_map(|t| track_entry(src_id, t))
        .collect())
}

fn list_sync(src_id: &str, client_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        // racine = titres populaires du mois
        "" => {
            let body = get_json(
                client_id,
                &format!("{API}/tracks/?format=json&limit=50&order=popularity_month"),
            )?;
            Ok(results_of(&body)
                .iter()
                .filter_map(|t| track_entry(src_id, t))
                .collect())
        }
        other => {
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(src_id, client_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin jamendo inconnu: {rel_path}"
            )))
        }
    }
}

/// Source Jamendo : populaires du mois + recherche + lecture complète (MP3, CC).
pub struct JamendoSource {
    source: Source,
    http: HttpSource,
}

impl JamendoSource {
    pub fn new(source: Source) -> Result<Self> {
        let SourceKind::Jamendo { .. } = &source.kind else {
            return Err(AurisError::other("source non jamendo"));
        };
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(JamendoSource { source, http })
    }

    fn client_id(&self) -> Result<&str> {
        match &self.source.kind {
            SourceKind::Jamendo {
                client_id: Some(id),
            } if !id.trim().is_empty() => Ok(id.trim()),
            _ => Err(missing_client_id()),
        }
    }
}

#[async_trait]
impl SourceBackend for JamendoSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let client_id = self.client_id()?.to_string();
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&src_id, &client_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("jamendo list (join): {e}")))?
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let client_id = self.client_id()?.to_string();
        let rel = rel_path.trim_matches('/').to_string();
        let track_id = rel
            .strip_prefix("track/")
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?
            .to_string();
        let body = tokio::task::spawn_blocking(move || {
            get_json(
                &client_id,
                &format!("{API}/tracks/?format=json&limit=1&id={}", pct(&track_id)),
            )
        })
        .await
        .map_err(|e| AurisError::other(format!("jamendo open (join): {e}")))??;
        let first = results_of(&body)
            .into_iter()
            .next()
            .ok_or_else(|| AurisError::other(format!("jamendo: piste inconnue {rel_path}")))?;
        let audio = get_str(&first, "audio")
            .ok_or_else(|| AurisError::other("jamendo: pas de flux audio pour ce titre"))?;
        Ok(Box::new(self.http.open_url(audio)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let client_id = self.client_id()?.to_string();
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&src_id, &client_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("jamendo search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn track_entry_nom_artiste_titre() {
        let t = json!({"id": "1871631", "name": "Sunrise", "artist_name": "Some Artist"});
        let e = track_entry("jm", &t).unwrap();
        assert!(e.is_audio);
        assert_eq!(e.name, "Some Artist — Sunrise");
        assert_eq!(e.uri, "src://jm/track/1871631");
    }

    #[test]
    fn track_entry_sans_id_rejetee() {
        assert!(track_entry("jm", &json!({"name": "x"})).is_none());
    }

    #[test]
    fn results_of_results() {
        let body = json!({"headers": {}, "results": [{"id": "1"}, {"id": "2"}]});
        assert_eq!(results_of(&body).len(), 2);
        assert!(results_of(&json!({"error": "bad client_id"})).is_empty());
    }

    #[test]
    fn source_sans_client_id_message_clair() {
        let src = Source {
            id: "jm".into(),
            name: "Jamendo".into(),
            kind: SourceKind::Jamendo { client_id: None },
            enabled: true,
        };
        let s = JamendoSource::new(src).unwrap();
        let err = s.client_id().unwrap_err().to_string();
        assert!(err.contains("client_id manquant"), "{err}");
        assert!(err.contains("developer.jamendo.com"), "{err}");
    }

    #[test]
    fn source_kind_serde() {
        let src: Source = serde_json::from_str(
            r#"{"id":"j","name":"Jamendo","enabled":true,"kind":"jamendo","client_id":"abc123"}"#,
        )
        .unwrap();
        assert!(matches!(
            src.kind,
            SourceKind::Jamendo {
                client_id: Some(ref id)
            } if id == "abc123"
        ));
        // sans client_id -> None (rétrocompatible)
        let src2: Source =
            serde_json::from_str(r#"{"id":"j","name":"Jamendo","enabled":true,"kind":"jamendo"}"#)
                .unwrap();
        assert!(matches!(src2.kind, SourceKind::Jamendo { client_id: None }));
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"jamendo""#), "{s}");
    }

    #[test]
    fn chemin_inconnu() {
        assert!(list_sync("jm", "cid", "nimportequoi").is_err());
    }
}
