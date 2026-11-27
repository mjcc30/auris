//! Source YouTube — #87 : recherche et lecture audio via `yt-dlp` (binaire
//! externe : env `AURIS_YTDLP`, sinon `yt-dlp` sur le PATH). Fonctionne sans
//! compte Google pour le contenu public (search `ytsearch:`, stream `bestaudio`).
//!
//! - Recherche : `search/<q>` -> `yt-dlp --flat-playlist --print … "ytsearch25:<q>"`
//! - Racine : `yt-dlp --flat-playlist … "https://www.youtube.com/feed/trending"`
//! - Lecture : `video/<id>` -> `yt-dlp -f bestaudio -g <watch_url>` -> URL googlevideo
//!   ouverte via le lecteur HTTP seekable.

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::{http::HttpSource, make_uri, SeekRead, SourceBackend};

const WATCH: &str = "https://www.youtube.com/watch?v=";
const TRENDING: &str = "https://www.youtube.com/feed/trending";
/// Format d'impression yt-dlp : champs séparés par `|` (les titres peuvent
/// contenir des `|` rares — on ne garde que le premier champ comme id).
const PRINT_FMT: &str = "%(id)s|%(title)s|%(uploader)s|%(duration)s";

fn ytdlp_path() -> Result<String> {
    if let Ok(p) = std::env::var("AURIS_YTDLP") {
        if !p.trim().is_empty() {
            return Ok(p.trim().to_string());
        }
    }
    Ok("yt-dlp".to_string())
}

fn missing_ytdlp(cmd: &str, err: &std::io::Error) -> AurisError {
    AurisError::other(format!(
        "YouTube: binaire yt-dlp introuvable ({cmd}: {err}) — installez-le \
         (https://github.com/yt-dlp/yt-dlp) ou renseignez AURIS_YTDLP=/chemin/yt-dlp"
    ))
}

fn run_ytdlp(args: &[&str]) -> Result<String> {
    let cmd = ytdlp_path()?;
    let out = std::process::Command::new(&cmd)
        .args(args)
        .output()
        .map_err(|e| missing_ytdlp(&cmd, &e))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(AurisError::other(format!(
            "yt-dlp {:?} a échoué ({}): {}",
            args.last().copied().unwrap_or(""),
            out.status,
            msg.lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse une ligne `--print '%(id)s|%(title)s|%(uploader)s|%(duration)s'`.
fn parse_line(src_id: &str, line: &str) -> Option<BrowseEntry> {
    let mut parts = line.splitn(4, '|');
    let id = parts.next()?.trim();
    if id.is_empty() || id == "NA" {
        return None;
    }
    let title = parts.next().unwrap_or("").trim();
    let uploader = parts.next().unwrap_or("").trim();
    let dur = parts.next().unwrap_or("").trim();
    let _ = dur; // BrowseEntry ne porte pas la durée ; le lecteur la lira au stream
    let name = if uploader.is_empty() {
        title.to_string()
    } else if title.is_empty() {
        uploader.to_string()
    } else {
        format!("{uploader} — {title}")
    };
    Some(BrowseEntry {
        name,
        uri: make_uri(src_id, &format!("video/{id}")),
        is_dir: false,
        size: None,
        format: None,
        is_audio: true,
    })
}

fn entries_from_ytdlp(src_id: &str, out: &str) -> Vec<BrowseEntry> {
    out.lines().filter_map(|l| parse_line(src_id, l)).collect()
}

fn search_sync(src_id: &str, query: &str) -> Result<Vec<BrowseEntry>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let out = run_ytdlp(&[
        "--no-warnings",
        "--flat-playlist",
        "--print",
        PRINT_FMT,
        &format!("ytsearch25:{q}"),
    ])?;
    Ok(entries_from_ytdlp(src_id, &out))
}

fn list_sync(src_id: &str, rel_path: &str) -> Result<Vec<BrowseEntry>> {
    let rel = rel_path.trim_matches('/');
    match rel {
        // racine = tendances YouTube du moment
        "" => {
            let out = run_ytdlp(&[
                "--no-warnings",
                "--flat-playlist",
                "--print",
                PRINT_FMT,
                TRENDING,
            ])?;
            Ok(entries_from_ytdlp(src_id, &out))
        }
        other => {
            if let Some(q) = other.strip_prefix("search/") {
                return search_sync(src_id, &crate::percent_decode(q));
            }
            Err(AurisError::other(format!(
                "chemin youtube inconnu: {rel_path}"
            )))
        }
    }
}

/// Source YouTube : tendances + recherche ytsearch + lecture bestaudio via yt-dlp.
pub struct YoutubeSource {
    source: Source,
    http: HttpSource,
}

impl YoutubeSource {
    pub fn new(source: Source) -> Result<Self> {
        if !matches!(source.kind, SourceKind::Youtube) {
            return Err(AurisError::other("source non youtube"));
        }
        let http = HttpSource::new(Source {
            id: format!("{}-http", source.id),
            name: format!("{}-http", source.name),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        })?;
        Ok(YoutubeSource { source, http })
    }
}

#[async_trait]
impl SourceBackend for YoutubeSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let rel = rel_path.to_string();
        tokio::task::spawn_blocking(move || list_sync(&src_id, &rel))
            .await
            .map_err(|e| AurisError::other(format!("youtube list (join): {e}")))?
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let rel = rel_path.trim_matches('/').to_string();
        let video_id = rel
            .strip_prefix("video/")
            .ok_or_else(|| AurisError::InvalidUri(rel_path.to_string()))?
            .to_string();
        let url = tokio::task::spawn_blocking(move || {
            let out = run_ytdlp(&[
                "--no-warnings",
                "-f",
                "bestaudio",
                "-g",
                &format!("{WATCH}{video_id}"),
            ])?;
            out.lines()
                .next()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .ok_or_else(|| AurisError::other("yt-dlp: aucune URL bestaudio"))
        })
        .await
        .map_err(|e| AurisError::other(format!("youtube open (join): {e}")))??;
        Ok(Box::new(self.http.open_url(&url)?))
    }

    async fn search(&self, query: &str) -> Result<Vec<BrowseEntry>> {
        let src_id = self.source.id.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || search_sync(&src_id, &q))
            .await
            .map_err(|e| AurisError::other(format!("youtube search (join): {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ligne_complete() {
        let e = parse_line("yt", "rFZHOHl-L8A|lofi hip hop radio|Lofi Girl|NA").unwrap();
        assert!(e.is_audio);
        assert!(!e.is_dir);
        assert_eq!(e.name, "Lofi Girl — lofi hip hop radio");
        assert_eq!(e.uri, "src://yt/video/rFZHOHl-L8A");
    }

    #[test]
    fn parse_ligne_sans_uploader() {
        let e = parse_line("yt", "abc123|Titre seul||300").unwrap();
        assert_eq!(e.name, "Titre seul");
    }

    #[test]
    fn parse_ligne_invalide() {
        assert!(parse_line("yt", "").is_none());
        assert!(parse_line("yt", "NA|x|y|z").is_none());
        assert!(parse_line("yt", "   ").is_none());
    }

    #[test]
    fn entries_filtre_les_vides() {
        let out = "id1|t1|u1|100\n\nNA|x|y|z\nid2|t2||200\n";
        let v = entries_from_ytdlp("yt", out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].uri, "src://yt/video/id2");
    }

    #[test]
    fn source_kind_serde() {
        let src: Source =
            serde_json::from_str(r#"{"id":"y","name":"YouTube","enabled":true,"kind":"youtube"}"#)
                .unwrap();
        assert!(matches!(src.kind, SourceKind::Youtube));
        let s = serde_json::to_string(&src).unwrap();
        assert!(s.contains(r#""kind":"youtube""#), "{s}");
    }

    #[test]
    fn chemin_inconnu() {
        assert!(list_sync("yt", "nimportequoi").is_err());
    }

    #[test]
    fn binaire_absent_message_clair() {
        // force un chemin inexistant
        std::env::set_var("AURIS_YTDLP", "/chemin/inexistant/yt-dlp-xyz");
        let err = run_ytdlp(&["--version"]).unwrap_err().to_string();
        std::env::remove_var("AURIS_YTDLP");
        assert!(err.contains("yt-dlp introuvable"), "{err}");
        assert!(err.contains("AURIS_YTDLP"), "{err}");
    }
}
