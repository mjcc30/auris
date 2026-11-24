//! Sources musicales: local, SMB (NAS), HTTP, UPnP/DLNA.

pub mod http;
pub mod http_raw;
pub mod local;
pub mod manager;
pub mod smb;
pub mod upnp;

pub use manager::SourcesManager;

use async_trait::async_trait;
use auris_core::{BrowseEntry, Result};
use std::io::{Read, Seek};

/// Flux binaire seekable (implante Read + Seek + Send + Sync).
pub trait SeekRead: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> SeekRead for T {}

/// Cree un URI de source: `src://<source_id>/<chemin relatif>`.
pub fn make_uri(source_id: &str, rel_path: &str) -> String {
    format!("src://{}/{}", source_id, rel_path.trim_start_matches('/'))
}

/// Parse un URI `src://<source_id>/<chemin>`.
pub fn parse_src_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("src://")?;
    let (id, path) = rest.split_once('/')?;
    Some((id.to_string(), path.to_string()))
}

pub fn is_src_uri(uri: &str) -> bool {
    uri.starts_with("src://")
}

pub fn is_http_uri(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// Reference resolue d'un URI de lecture.
#[derive(Debug, Clone)]
pub enum UriRef {
    /// Fichier dans une source configuree.
    Src { source_id: String, path: String },
    /// URL HTTP(S) directe.
    Http(String),
}

pub fn resolve_uri(uri: &str) -> Result<UriRef> {
    if let Some((id, path)) = parse_src_uri(uri) {
        return Ok(UriRef::Src {
            source_id: id,
            path,
        });
    }
    if is_http_uri(uri) {
        return Ok(UriRef::Http(uri.to_string()));
    }
    Err(auris_core::AurisError::InvalidUri(uri.to_string()))
}

/// Backend d'une source: listage + ouverture de fichiers.
#[async_trait]
pub trait SourceBackend: Send + Sync {
    fn source(&self) -> &auris_core::Source;
    /// Liste un dossier relatif a la racine de la source ("" = racine).
    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>>;
    /// Ouvre un fichier relatif a la racine de la source.
    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>>;
    /// Taille d'un fichier, si connue.
    async fn size(&self, _rel_path: &str) -> Result<Option<u64>> {
        Ok(None)
    }
}
