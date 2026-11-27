//! Sources musicales: local, USB, SMB (NAS), HTTP, UPnP/DLNA, webradios.

pub mod http;
pub mod http_raw;
pub mod live;
pub mod local;
pub mod manager;
pub mod radio;
pub mod smb;
pub mod transfer;
pub mod upnp;
pub mod usb;

pub use live::{open_live, radio_title, LiveStream};
pub use manager::SourcesManager;
pub use transfer::CopyReport;
pub use usb::{discover_removable, MountInfo};

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result};
use std::io::{Read, Seek};
use std::path::PathBuf;

/// Flux binaire seekable (implante Read + Seek + Send + Sync).
pub trait SeekRead: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> SeekRead for T {}

/// Cree un URI de source: `src://<source_id>/<chemin relatif>`.
pub fn make_uri(source_id: &str, rel_path: &str) -> String {
    format!("src://{}/{}", source_id, rel_path.trim_start_matches('/'))
}

/// Decode percent-encoding (`%20` -> espace, etc.). Pure Rust, sans dependance.
/// Un `%` non suivi de deux chiffres hexadecimaux est conserve tel quel.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = u8::from_str_radix(&s[i + 1..i + 3], 16);
            if let Ok(v) = h {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    // reconstruit en UTF-8 lossy: un octet invalide devient U+FFFD
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse un URI `src://<source_id>/<chemin>` (le chemin est percent-decode,
/// donc `Black%20Sabbath` et `Black Sabbath` designent le meme dossier).
pub fn parse_src_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("src://")?;
    let (id, path) = rest.split_once('/')?;
    Some((id.to_string(), percent_decode(path)))
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

/// Backend d'une source: listage + ouverture + (optionnel) ecriture.
#[async_trait]
pub trait SourceBackend: Send + Sync {
    fn source(&self) -> &auris_core::Source;
    /// Liste un dossier relatif a la racine de la source ("" = racine).
    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>>;
    /// Ouvre un fichier relatif a la racine de la source.
    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>>;
    /// Chemin local absolu du fichier, si la source est un montage local/USB
    /// (`None` pour SMB/HTTP/UPnP/radio). Sert au repli ffmpeg : avec un path,
    /// `ffmpeg -ss <s> -i <path>` seek nativement dans le conteneur (#54) ;
    /// sans path, le flux est pipe et le seek re-decode depuis le debut.
    fn local_path(&self, _rel_path: &str) -> Option<PathBuf> {
        None
    }
    /// Taille d'un fichier, si connue.
    async fn size(&self, _rel_path: &str) -> Result<Option<u64>> {
        Ok(None)
    }

    // ---------- ecriture (import / copie vers la source) ----------

    /// La source accepte-t-elle l'ecriture (copie vers cette source) ?
    fn writable(&self) -> bool {
        false
    }
    /// Cree un repertoire (et ses parents) relatif a la racine.
    async fn mkdir_p(&self, _rel_path: &str) -> Result<()> {
        Err(AurisError::other("source en lecture seule"))
    }
    /// Ecrit un fichier (ecrasement) depuis un flux. Les parents doivent exister.
    async fn put(
        &self,
        _rel_path: &str,
        _reader: Box<dyn std::io::Read + Send>,
        _size: Option<u64>,
    ) -> Result<u64> {
        Err(AurisError::other("source en lecture seule"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_basique() {
        assert_eq!(percent_decode("Black%20Sabbath"), "Black Sabbath");
        assert_eq!(percent_decode("100%25.flac"), "100%.flac");
        assert_eq!(percent_decode("100%.flac"), "100%.flac"); // % non hexa conserve
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("rien"), "rien");
        assert_eq!(percent_decode("%2Fetc"), "/etc");
    }

    #[test]
    fn parse_src_uri_decode_le_chemin() {
        let (id, p) = parse_src_uri("src://nas/1970/Black%20Sabbath").unwrap();
        assert_eq!(id, "nas");
        assert_eq!(p, "1970/Black Sabbath");
        // l'espace brut fonctionne aussi
        let (_, p2) = parse_src_uri("src://nas/1970/Black Sabbath").unwrap();
        assert_eq!(p2, "1970/Black Sabbath");
        assert!(parse_src_uri("http://x/y").is_none());
    }
}
