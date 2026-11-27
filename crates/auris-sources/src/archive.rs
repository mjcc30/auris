//! Source Internet Archive (archive.org) — #84 : catalogue audio gratuit, sans compte.
//!
//! - Recherche : `GET archive.org/advancedsearch.php?q=<q> AND mediatype:audio`
//! - Métadonnées d'un item : `GET archive.org/metadata/<identifier>`
//! - Lecture : `GET archive.org/download/<identifier>/<fichier>` (HTTP seekable).
//!
//! Arborescence : racine = Populaires / Ajouts récents / Recherche ; chaque item
//! est un dossier listant ses fichiers audio jouables un par un.

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};
use serde_json::Value;

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

const API: &str = "https://archive.org";

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!(
            "auris/",
            env!("CARGO_PKG_VERSION"),
            " (archive.org)"
        ))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

/// Percent-encoding minimal pour les query strings et les noms de fichiers.
fn pct(s: &str) -> String {
    pct_impl(s, false)
}

/// Percent-encoding qui conserve les '/' (chemin de fichier download/<id>/<sous-dossier>/<fichier>).
fn pct_path(s: &str) -> String {
    pct_impl(s, true)
}

fn pct_impl(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && b == b'/')
        {
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

/// Extensions audio jouables (le reste est ignoré dans le listage d'un item).
fn is_audio_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        ".mp3", ".flac", ".ogg", ".opus", ".m4a", ".wav", ".wma", ".aac",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
}

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

/// Un item archive.org = un dossier (`item/<identifier>`).
fn item_dir(src_id: &str, doc: &Value) -> Option<BrowseEntry> {
    let id = get_str(doc, "identifier")?;
    let title = get_str(doc, "title").unwrap_or("Item sans titre");
    let creator = get_str(doc, "creator").unwrap_or("");
    let name = if creator.is_empty() {
        title.to_string()
    } else {
        format!("{title} — {creator}")
    };
    Some(dir(src_id, &format!("item/{id}"), &name))
}

fn get_json(url: &str) -> Result<Value> {
    let resp = http()
        .get(url)
        .send()
        .map_err(|e| AurisError::other(format!("archive.org GET {url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AurisError::other(format!(
            "archive.org GET {url}: HTTP {status}"
        )));
    }
    resp.json()
        .map_err(|e| AurisError::other(format!("archive.org GET {url} (json): {e}")))
}

/// `advancedsearch.php` : q + tri + rows -> docs.
fn search_api(src_id: &str, q: &str, sort: &str, rows: u32) -> Result<Vec<BrowseEntry>> {
    let url = format!(
        "{API}/advancedsearch.php?q={}&fl[]=identifier&fl[]=title&fl[]=creator&rows={rows}&output=json{}",
        pct(q),
        if sort.is_empty() {
            String::new()
        } else {
            format!("&sort[]={}", pct(sort))
        }
    );
    let body = get_json(&url)?;
    let docs = body
        .get("response")
        .and_then(|r| r.get("docs"))
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(docs.iter().filter_map(|d| item_dir(src_id, d)).collect())
}

fn search_sync(src_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    search_api(src_id, &format!("{q} AND mediatype:audio"), "", 30)
}

/// Fichiers audio d'un item (métadonnées `metadata/<id>`).
fn item_files(src_id: &str, id: &str) -> Result<Vec<BrowseEntry>> {
    let body = get_json(&format!("{API}/metadata/{}", pct(id)))?;
    let files = body
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for f in &files {
        let name = match get_str(f, "name") {
            Some(n) if is_audio_file(n) => n,
            _ => continue,
        };
        let size = f
            .get("size")
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<u64>().ok());
        out.push(BrowseEntry {
            name: name.to_string(),
            uri: make_uri(src_id, &format!("file/{id}/{name}")),
            is_dir: false,
            size,
            format: None,
            is_audio: true,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn list_sync(src_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        "" => Ok(vec![
            dir(src_id, "popular", "Populaires"),
            dir(src_id, "new", "Ajouts récents"),
        ]),
        "popular" => search_api(src_id, "mediatype:audio", "downloads desc", 50),
        "new" => search_api(src_id, "mediatype:audio", "addeddate desc", 50),
        other => {
            if let Some(id) = other.strip_prefix("item/") {
                return item_files(src_id, id);
            }
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(src_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin archive inconnu: {rel_path}"
            )))
        }
    }
}

/// Source Internet Archive : navigation + lecture HTTP seekable des fichiers.
pub struct ArchiveSource {
    source: Source,
    http: HttpSource,
}

impl ArchiveSource {
    pub fn new(source: Source) -> Result<Self> {
        if !matches!(source.kind, SourceKind::Archive) {
            return Err(AurisError::other("source non archive"));
        }
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(ArchiveSource { source, http })
    }
}

#[async_trait]
impl SourceBackend for ArchiveSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&src_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("archive list (join): {e}")))?
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let rel = rel_path.trim_matches('/').to_string();
        let file = rel
            .strip_prefix("file/")
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?;
        let (id, name) = file
            .split_once('/')
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?;
        let url = format!("{API}/download/{}/{}", pct(id), pct_path(name));
        Ok(Box::new(self.http.open_url(&url)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&src_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("archive search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_dir_nomme_avec_createur() {
        let doc =
            json!({"identifier": "KindOfBlue", "title": "Kind of Blue", "creator": "Miles Davis"});
        let e = item_dir("arc", &doc).unwrap();
        assert!(e.is_dir);
        assert_eq!(e.name, "Kind of Blue — Miles Davis");
        assert_eq!(e.uri, "src://arc/item/KindOfBlue");
    }

    #[test]
    fn item_dir_sans_createur() {
        let doc = json!({"identifier": "x", "title": "Solo"});
        assert_eq!(item_dir("arc", &doc).unwrap().name, "Solo");
        assert!(item_dir("arc", &json!({"title": "no id"})).is_none());
    }

    #[test]
    fn audio_files_filtre_extensions() {
        assert!(is_audio_file("01 - So What.mp3"));
        assert!(is_audio_file("track.FLAC"));
        assert!(is_audio_file("a.ogg"));
        assert!(!is_audio_file("cover.jpg"));
        assert!(!is_audio_file("notes.txt"));
        assert!(!is_audio_file("archive.m3u"));
    }

    #[test]
    fn pct_encode() {
        assert_eq!(pct("My Song.mp3"), "My%20Song.mp3");
        assert_eq!(pct("a+b&c"), "a%2Bb%26c");
        assert_eq!(pct("déjà.flac"), "d%C3%A9j%C3%A0.flac");
        // les sous-dossiers d'un fichier archive.org gardent leur '/'
        assert_eq!(pct_path("Mishary/01.mp3"), "Mishary/01.mp3");
        assert_eq!(pct_path("A B/C d.flac"), "A%20B/C%20d.flac");
    }

    #[test]
    fn source_kind_serde() {
        let src: Source =
            serde_json::from_str(r#"{"id":"a","name":"Archive","enabled":true,"kind":"archive"}"#)
                .unwrap();
        assert!(matches!(src.kind, SourceKind::Archive));
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"archive""#), "{s}");
    }

    #[test]
    fn racine_listing() {
        let entries = list_sync("arc", "").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.is_dir));
    }
}
