//! Source locale: repertoire monte (NAS en NFS/SMB monte par l'OS, disque USB...).

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

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
        if !path.is_dir() {
            return Err(AurisError::other(format!(
                "dossier introuvable: {}",
                path.display()
            )));
        }
        Ok(LocalSource { source, root: path })
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }
}

#[async_trait]
impl SourceBackend for LocalSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let dir = self.abs(rel_path);
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
        let path = self.abs(rel_path);
        let f = File::open(&path)
            .map_err(|e| AurisError::other(format!("ouverture {}: {e}", path.display())))?;
        Ok(Box::new(BufReader::with_capacity(256 * 1024, f)))
    }

    async fn size(&self, rel_path: &str) -> Result<Option<u64>> {
        Ok(fs::metadata(self.abs(rel_path)).ok().map(|m| m.len()))
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
