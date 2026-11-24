//! Transfert (import) entre sources: copie de fichiers/dossiers d'une source
//! vers une autre (ex: NAS -> cle USB, dossier local -> NAS SMB...).

use serde::Serialize;

use auris_core::{AurisError, Result};

use crate::manager::SourcesManager;

/// Bilan d'une copie.
#[derive(Debug, Clone, Serialize)]
pub struct CopyReport {
    /// Nombre de fichiers copies.
    pub files: usize,
    /// Octets copies.
    pub bytes: u64,
    /// Dossiers crees.
    pub dirs: usize,
    /// Erreurs rencontrees (la copie continue malgre tout).
    pub errors: Vec<String>,
}

/// Rejoint un chemin de dossier et un nom, en separateurs `/`.
pub fn join_rel(dir: &str, name: &str) -> String {
    let d = dir.trim_matches('/');
    if d.is_empty() {
        name.to_string()
    } else {
        format!("{d}/{name}")
    }
}

/// Copie `from` (fichier ou dossier, URI `src://...`) dans le dossier
/// `to_dir` (URI `src://dst/...` d'un dossier existant ou a creer).
pub async fn copy_uri(sources: &SourcesManager, from: &str, to_dir: &str) -> Result<CopyReport> {
    let (src_id, src_rel) =
        crate::parse_src_uri(from).ok_or_else(|| AurisError::InvalidUri(from.into()))?;
    let (dst_id, dst_rel) =
        crate::parse_src_uri(to_dir).ok_or_else(|| AurisError::InvalidUri(to_dir.into()))?;
    if src_id == dst_id && src_rel == dst_rel {
        return Err(AurisError::InvalidUri(
            "source et destination identiques".into(),
        ));
    }
    let dst_backend = sources
        .backend(&dst_id)
        .ok_or_else(|| AurisError::SourceNotFound(dst_id.clone()))?;
    if !dst_backend.writable() {
        return Err(AurisError::other(format!(
            "la source destination '{dst_id}' est en lecture seule"
        )));
    }
    let entry = sources.stat(from).await?;
    let mut report = CopyReport {
        files: 0,
        bytes: 0,
        dirs: 0,
        errors: Vec::new(),
    };
    if entry.is_dir {
        copy_dir(
            sources,
            &src_id,
            &src_rel,
            &dst_id,
            &join_rel(&dst_rel, &entry.name),
            &mut report,
        )
        .await;
    } else {
        let dst_rel = join_rel(&dst_rel, &entry.name);
        match copy_file(sources, &src_id, &src_rel, &dst_id, &dst_rel).await {
            Ok(n) => {
                report.files += 1;
                report.bytes += n;
            }
            Err(e) => report.errors.push(format!("{from}: {e}")),
        }
    }
    Ok(report)
}

async fn copy_file(
    sources: &SourcesManager,
    src_id: &str,
    src_rel: &str,
    dst_id: &str,
    dst_rel: &str,
) -> Result<u64> {
    let src = sources
        .backend(src_id)
        .ok_or_else(|| AurisError::SourceNotFound(src_id.to_string()))?;
    let size = src.size(src_rel).await.ok().flatten();
    let reader = sources.open_uri(&crate::make_uri(src_id, src_rel)).await?;
    let dst = sources
        .backend(dst_id)
        .ok_or_else(|| AurisError::SourceNotFound(dst_id.to_string()))?;
    dst.put(dst_rel, reader, size).await
}

async fn copy_dir(
    sources: &SourcesManager,
    src_id: &str,
    src_rel: &str,
    dst_id: &str,
    dst_rel: &str,
    report: &mut CopyReport,
) {
    let dst = match sources.backend(dst_id) {
        Some(b) => b,
        None => {
            report.errors.push(format!("source {dst_id} introuvable"));
            return;
        }
    };
    if let Err(e) = dst.mkdir_p(dst_rel).await {
        report.errors.push(format!("mkdir {dst_rel}: {e}"));
        return;
    }
    report.dirs += 1;
    // pile explicite pour eviter la recursion async
    let mut stack: Vec<(String, String)> = vec![(src_rel.to_string(), dst_rel.to_string())];
    while let Some((s_rel, d_rel)) = stack.pop() {
        let uri = crate::make_uri(src_id, &s_rel);
        let entries = match sources.browse(&uri).await {
            Ok(e) => e,
            Err(e) => {
                report.errors.push(format!("browse {uri}: {e}"));
                continue;
            }
        };
        for e in entries {
            let child_src = join_rel(&s_rel, &e.name);
            let child_dst = join_rel(&d_rel, &e.name);
            if e.is_dir {
                if let Err(err) = dst.mkdir_p(&child_dst).await {
                    report.errors.push(format!("mkdir {child_dst}: {err}"));
                    continue;
                }
                report.dirs += 1;
                stack.push((child_src, child_dst));
            } else {
                match copy_file(sources, src_id, &child_src, dst_id, &child_dst).await {
                    Ok(n) => {
                        report.files += 1;
                        report.bytes += n;
                    }
                    Err(err) => report.errors.push(format!(
                        "{} -> {}: {err}",
                        crate::make_uri(src_id, &child_src),
                        child_dst
                    )),
                }
            }
        }
    }
}
