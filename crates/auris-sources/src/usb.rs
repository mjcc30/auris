//! Decouverte des disques amovibles (cles USB, disques externes) montes par l'OS.
//!
//! - Linux: `/media/<user>/*`, `/run/media/<user>/*`, `/mnt/*`
//! - macOS: `/Volumes/*` (hors disque systeme)
//! - Windows: lettres de lecteur `A:`..`Z:` (hors lecteur systeme)

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Un montage amovible detecte.
#[derive(Debug, Clone, Serialize)]
pub struct MountInfo {
    pub label: String,
    pub path: String,
}

/// Racines de montage amovible selon l'OS.
pub fn media_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut roots = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            let user = std::env::var("USER")
                .ok()
                .or_else(|| home.file_name().map(|u| u.to_string_lossy().into_owned()))
                .unwrap_or_default();
            roots.push(home.join("media").join(&user));
            roots.push(PathBuf::from("/run/media").join(&user));
            roots.push(home.join("media"));
        }
        roots.push(PathBuf::from("/media"));
        roots.push(PathBuf::from("/mnt"));
        roots
    }
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/Volumes")]
    }
    #[cfg(target_os = "windows")]
    {
        Vec::new() // les lettres sont sondees directement
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Liste les sous-dossiers d'une racine de montage (un niveau).
pub fn scan_media_dir(root: &Path) -> Vec<MountInfo> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let label = e.file_name().to_string_lossy().to_string();
                // macOS: on ignore le disque systeme
                #[cfg(target_os = "macos")]
                if label == "Macintosh HD" {
                    continue;
                }
                out.push(MountInfo {
                    label,
                    path: p.to_string_lossy().replace('\\', "/"),
                });
            }
        }
    }
    out.sort_by_key(|a| a.label.to_lowercase());
    out
}

/// Sonde les lettres de lecteur Windows `A:`..`Z:` (hors lecteur systeme).
#[cfg(target_os = "windows")]
pub fn scan_windows_drives() -> Vec<MountInfo> {
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let path = format!("{}:\\", letter as char);
        if path.starts_with(&sys) {
            continue;
        }
        if Path::new(&path).is_dir() {
            out.push(MountInfo {
                label: format!("{} ({})", path.trim_end_matches('\\'), letter as char),
                path: path.replace('\\', "/"),
            });
        }
    }
    out
}

/// Decouvre tous les montages amovibles connus de l'OS.
pub fn discover_removable() -> Vec<MountInfo> {
    #[cfg(target_os = "windows")]
    {
        scan_windows_drives()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut out = Vec::new();
        for root in media_roots() {
            out.extend(scan_media_dir(&root));
        }
        // dedup (ex: /media/user et /media peuvent contenir les memes liens)
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("auris-usb-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn scan_media_dir_liste_les_dossiers() {
        let root = tmp("scan");
        std::fs::create_dir_all(root.join("CLE-USB")).unwrap();
        std::fs::create_dir_all(root.join("Disque Externe")).unwrap();
        std::fs::write(root.join("fichier.txt"), b"pas un montage").unwrap();
        let mounts = scan_media_dir(&root);
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].label, "CLE-USB");
        assert_eq!(mounts[1].label, "Disque Externe");
        assert!(mounts[0].path.contains("CLE-USB"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_media_dir_racine_inexistante() {
        let root = tmp("nope");
        assert!(scan_media_dir(&root).is_empty());
    }

    #[test]
    fn discover_ne_panique() {
        // ne leve pas d'erreur, meme sans aucun disque branche
        let _ = discover_removable();
    }
}
