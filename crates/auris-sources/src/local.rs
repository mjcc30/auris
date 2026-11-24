//! Source locale: repertoire monte (NAS en NFS/SMB monte par l'OS, disque USB...).

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use auris_core::{is_audio_file, AurisError, BrowseEntry, Result, Source};

use crate::{make_uri, SeekRead, SourceBackend};

pub struct LocalSource {
    source: Source,
    root: PathBuf,
}

impl LocalSource {
    pub fn new(source: Source) -> Result<Self> {
        let path = match &source.kind {
            auris_core::SourceKind::Local { path } => PathBuf::from(path),
            _ => return Err(AurisError::other("source non locale")),
        };
        Self::with_root(source, path)
    }

    /// Construit une source locale racinee sur `root` quel que soit le kind
    /// d'origine (utilise pour les sources USB: meme acces, racine = montage).
    pub fn with_root(source: Source, root: PathBuf) -> Result<Self> {
        if !root.is_dir() {
            return Err(AurisError::other(format!(
                "dossier introuvable: {}",
                root.display()
            )));
        }
        Ok(LocalSource { source, root })
    }

    /// Chemin absolu d'un chemin relatif, en refusant toute sortie de la racine
    /// (`..`, chemins absolus, prefixes Windows).
    fn abs(&self, rel: &str) -> Result<PathBuf> {
        let rel = rel.trim_start_matches('/');
        let p = Path::new(rel);
        for c in p.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                _ => {
                    return Err(AurisError::InvalidUri(format!(
                        "chemin relatif invalide: {rel}"
                    )))
                }
            }
        }
        Ok(self.root.join(rel))
    }
}

#[async_trait]
impl SourceBackend for LocalSource {
    fn source(&self) -> &Source {
        &self.source
    }

    fn writable(&self) -> bool {
        true
    }

    async fn mkdir_p(&self, rel_path: &str) -> Result<()> {
        let dir = self.abs(rel_path)?;
        fs::create_dir_all(&dir)
            .map_err(|e| AurisError::other(format!("mkdir {}: {e}", dir.display())))?;
        Ok(())
    }

    async fn put(
        &self,
        rel_path: &str,
        mut reader: Box<dyn Read + Send>,
        _size: Option<u64>,
    ) -> Result<u64> {
        let path = self.abs(rel_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AurisError::other(format!("mkdir {}: {e}", parent.display())))?;
        }
        let mut out = File::create(&path)
            .map_err(|e| AurisError::other(format!("creation {}: {e}", path.display())))?;
        let n = std::io::copy(&mut reader, &mut out)
            .map_err(|e| AurisError::other(format!("ecriture {}: {e}", path.display())))?;
        Ok(n)
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let dir = self.abs(rel_path)?;
        let rd = fs::read_dir(&dir)
            .map_err(|e| AurisError::other(format!("lecture {}: {e}", dir.display())))?;
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = path.is_dir();
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let size = if is_dir {
                None
            } else {
                fs::metadata(&path).ok().map(|m| m.len())
            };
            out.push(BrowseEntry {
                name,
                uri: make_uri(&self.source.id, &rel),
                is_dir,
                size,
                format: None,
                is_audio: !is_dir && is_audio_file(&rel),
            });
        }
        out.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
        Ok(out)
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let path = self.abs(rel_path)?;
        let f = File::open(&path)
            .map_err(|e| AurisError::other(format!("ouverture {}: {e}", path.display())))?;
        Ok(Box::new(BufReader::with_capacity(256 * 1024, f)))
    }

    async fn size(&self, rel_path: &str) -> Result<Option<u64>> {
        Ok(fs::metadata(self.abs(rel_path)?).ok().map(|m| m.len()))
    }
}

/// Source USB: un disque amovible monte, avec les memes capacites qu'une
/// source locale (listage + lecture + ecriture pour l'import).
pub struct UsbSource {
    source: Source,
    inner: LocalSource,
}

impl UsbSource {
    pub fn new(source: Source) -> Result<Self> {
        let path = match &source.kind {
            auris_core::SourceKind::Usb { path } => PathBuf::from(path),
            _ => return Err(AurisError::other("source non usb")),
        };
        let inner = LocalSource::with_root(
            Source {
                id: source.id.clone(),
                name: source.name.clone(),
                kind: auris_core::SourceKind::Local {
                    path: path.to_string_lossy().to_string(),
                },
                enabled: source.enabled,
            },
            path,
        )?;
        Ok(UsbSource { source, inner })
    }
}

#[async_trait]
impl SourceBackend for UsbSource {
    fn source(&self) -> &Source {
        &self.source
    }
    fn writable(&self) -> bool {
        true
    }
    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        self.inner.list(rel_path).await
    }
    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        self.inner.open(rel_path).await
    }
    async fn size(&self, rel_path: &str) -> Result<Option<u64>> {
        self.inner.size(rel_path).await
    }
    async fn mkdir_p(&self, rel_path: &str) -> Result<()> {
        self.inner.mkdir_p(rel_path).await
    }
    async fn put(
        &self,
        rel_path: &str,
        reader: Box<dyn Read + Send>,
        size: Option<u64>,
    ) -> Result<u64> {
        self.inner.put(rel_path, reader, size).await
    }
}

pub fn walk_audio_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else if is_audio_file(&name) {
                    out.push(p);
                }
            }
        }
    }
    out
}
