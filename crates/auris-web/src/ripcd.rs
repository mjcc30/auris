//! Rip CD audio via `whipper` (AccurateRip) — #27.
//!
//! Le binaire `whipper` est invoqué en processus enfant (introuvable = la
//! fonctionnalité est désactivée mais l'API répond). La progression est lue
//! ligne par ligne sur stdout/stderr et exposée via `GET /api/ripcd/status`
//! (polling UI). Après un rip réussi, la bibliothèque est rescannée.
//!
//! Sur Windows le binaire est lancé via `cmd /C` (permet un mock `.cmd` dans
//! le PATH pour les tests) ; sur Unix il est exécuté directement.

use std::process::Stdio;
use std::sync::{Arc, Mutex};

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::{ApiResult, AppState};

const LOG_TAIL: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RipPhase {
    Idle,
    Info,
    Ripping,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RipTrack {
    pub n: u32,
    pub duration_secs: f64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RipStatus {
    pub whipper_available: bool,
    pub whipper_version: String,
    pub phase: RipPhase,
    pub device: Option<String>,
    /// 0..100
    pub progress: f64,
    pub current_track: u32,
    pub total_tracks: u32,
    pub toc: Vec<RipTrack>,
    pub output_dir: Option<String>,
    pub log: Vec<String>,
    pub error: Option<String>,
}

impl RipStatus {
    fn idle(available: bool, version: String) -> Self {
        RipStatus {
            whipper_available: available,
            whipper_version: version,
            phase: RipPhase::Idle,
            device: None,
            progress: 0.0,
            current_track: 0,
            total_tracks: 0,
            toc: Vec::new(),
            output_dir: None,
            log: Vec::new(),
            error: None,
        }
    }
}

#[derive(Default)]
struct Inner {
    status: Option<RipStatus>,
    child: Option<Arc<tokio::sync::Mutex<tokio::process::Child>>>,
}

/// Hub de rip CD (#27).
pub struct RipCdHub {
    inner: Mutex<Inner>,
    /// Nom/chemin du binaire whipper (env `AURIS_WHIPPER_BIN` ou « whipper »).
    bin: String,
    /// Rappel après un rip terminé (résultat : true = succès) — rescan biblio.
    on_finished: OnFinished,
}

type OnFinished = Mutex<Option<Box<dyn Fn(bool) + Send + Sync>>>;

impl RipCdHub {
    pub fn new() -> Arc<Self> {
        let bin = std::env::var("AURIS_WHIPPER_BIN")
            .ok()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| "whipper".to_string());
        Arc::new(RipCdHub {
            inner: Mutex::new(Inner::default()),
            bin,
            on_finished: Mutex::new(None),
        })
    }

    pub fn set_on_finished(&self, f: impl Fn(bool) + Send + Sync + 'static) {
        *self.on_finished.lock().unwrap() = Some(Box::new(f));
    }

    pub fn status(&self) -> RipStatus {
        let inner = self.inner.lock().unwrap();
        match &inner.status {
            Some(s) => s.clone(),
            None => {
                let (ok, ver) = probe_whipper(&self.bin);
                RipStatus::idle(ok, ver)
            }
        }
    }

    fn with_status<R>(&self, f: impl FnOnce(&mut RipStatus) -> R) -> R {
        let mut inner = self.inner.lock().unwrap();
        if inner.status.is_none() {
            let (ok, ver) = probe_whipper(&self.bin);
            inner.status = Some(RipStatus::idle(ok, ver));
        }
        f(inner.status.as_mut().unwrap())
    }

    fn busy(&self) -> bool {
        matches!(
            self.inner.lock().unwrap().status.as_ref().map(|s| s.phase),
            Some(RipPhase::Ripping) | Some(RipPhase::Info)
        )
    }

    /// Construit la commande whipper (cmd /C sur Windows pour résoudre le PATH).
    fn cmd(&self, args: &[&str]) -> Command {
        let c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&self.bin);
            for a in args {
                c.arg(a);
            }
            c
        } else {
            let mut c = Command::new(&self.bin);
            for a in args {
                c.arg(a);
            }
            c
        };
        c
    }

    /// `whipper cd info` : lit la table des matières du CD courant.
    pub async fn info(&self, device: Option<String>) -> ApiResult<RipStatus> {
        if self.busy() {
            return Err(err("un rip ou une analyse est déjà en cours"));
        }
        let mut args: Vec<String> = vec!["cd".into(), "info".into()];
        if let Some(d) = &device {
            args.push("-d".into());
            args.push(d.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = self.cmd(&arg_refs).output().await.map_err(|e| {
            err(format!(
                "impossible d'exécuter `{}` : {e} (installez whipper)",
                self.bin
            ))
        })?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let (toc, total) = parse_toc(&text);
        let ok = output.status.success();
        let status = self.with_status(|s| {
            s.log.clear();
            s.phase = if ok { RipPhase::Idle } else { RipPhase::Error };
            s.device = device;
            s.toc = toc;
            s.total_tracks = total;
            push_log(s, &text);
            if !ok {
                s.error = Some("whipper cd info a échoué (pas de CD dans le lecteur ?)".into());
            } else {
                s.error = None;
            }
            s.clone()
        });
        Ok(status)
    }

    /// `whipper cd rip` : rippe le CD dans la première source locale active.
    pub async fn rip(
        self: &Arc<Self>,
        s: &AppState,
        device: Option<String>,
    ) -> ApiResult<RipStatus> {
        if self.busy() {
            return Err(err("un rip ou une analyse est déjà en cours"));
        }
        let out = local_output_dir(s).ok_or_else(|| {
            err("configurez d'abord une source locale active (le CD y sera ripé)")
        })?;
        std::fs::create_dir_all(&out).map_err(|e| err(format!("création de {out}: {e}")))?;

        let mut args: Vec<String> = vec!["cd".into(), "rip".into(), "-o".into(), out.clone()];
        if let Some(d) = &device {
            args.push("-d".into());
            args.push(d.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let mut child = self
            .cmd(&arg_refs)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                err(format!(
                    "impossible d'exécuter `{}` : {e} (installez whipper)",
                    self.bin
                ))
            })?;

        self.with_status(|st| {
            st.phase = RipPhase::Ripping;
            st.device = device.clone();
            st.progress = 0.0;
            st.current_track = 0;
            st.output_dir = Some(out.clone());
            st.error = None;
            st.log.clear();
        });

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // handle partagé pour pouvoir tuer le process depuis stop()
        let child = Arc::new(tokio::sync::Mutex::new(child));
        {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child.clone());
        }
        let hub = self.clone();
        let on_finished = self.on_finished.lock().unwrap().take();
        tokio::spawn(async move {
            let mut readers = Vec::new();
            if let Some(o) = stdout {
                readers.push(tokio::spawn(read_lines(o, hub.clone())));
            }
            if let Some(e) = stderr {
                readers.push(tokio::spawn(read_lines(e, hub.clone())));
            }
            // scrute try_wait par intervalles : stop() peut verrouiller et tuer
            // le process entre deux polls (wait() bloquerait le mutex sinon).
            let code = loop {
                {
                    let mut c = child.lock().await;
                    if let Some(status) = c.try_wait().ok().flatten() {
                        break Some(status.code().unwrap_or(-1));
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            };
            for r in readers {
                let _ = r.await;
            }
            let success = code == Some(0);
            hub.with_status(|st| {
                st.phase = if success {
                    RipPhase::Done
                } else {
                    RipPhase::Error
                };
                if success {
                    st.progress = 100.0;
                    st.error = None;
                } else {
                    st.error = Some(format!(
                        "whipper a quitté avec le code {code:?} — voir le journal ci-dessous"
                    ));
                }
            });
            {
                let mut inner = hub.inner.lock().unwrap();
                inner.child = None;
            }
            if let Some(f) = on_finished {
                f(success);
            }
        });

        Ok(self.status())
    }

    /// Arrête le rip en cours (kill du processus whipper).
    pub async fn stop(&self) -> ApiResult<RipStatus> {
        let child = {
            let mut inner = self.inner.lock().unwrap();
            inner.child.take()
        };
        if let Some(c) = child {
            let mut c = c.lock().await;
            let _ = c.kill().await;
        }
        self.with_status(|s| {
            if s.phase == RipPhase::Ripping {
                s.phase = RipPhase::Error;
                s.error = Some("rip annulé par l'utilisateur".into());
            }
        });
        Ok(self.status())
    }

    /// Éjecte le tiroir du lecteur de CD.
    pub async fn eject(&self, device: Option<String>) -> ApiResult<serde_json::Value> {
        let dev = device.or_else(|| self.status().device).unwrap_or_default();
        let mut c = if cfg!(windows) {
            // Shell.Application Namespace(17) = lecteurs amovibles
            let mut c = Command::new("powershell");
            c.arg("-NoProfile")
                .arg("-Command")
                .arg(format!(
                    "(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{dev}').InvokeVerb('Eject')"
                ));
            c
        } else {
            let mut c = Command::new("eject");
            if !dev.is_empty() {
                c.arg(&dev);
            }
            c
        };
        let out = c.output().await.map_err(|e| err(format!("eject: {e}")))?;
        let ok = out.status.success();
        Ok(serde_json::json!({
            "ok": ok,
            "stderr": String::from_utf8_lossy(&out.stderr),
        }))
    }
}

async fn read_lines<R: tokio::io::AsyncRead + Unpin>(reader: R, hub: Arc<RipCdHub>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        hub.with_status(|s| {
            parse_progress(&line, s);
            push_log(s, &line);
        });
    }
}

fn push_log(s: &mut RipStatus, text: &str) {
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        s.log.push(line.to_string());
    }
    while s.log.len() > LOG_TAIL {
        let extra = s.log.len() - LOG_TAIL;
        s.log.drain(..extra);
    }
}

fn err(msg: impl Into<String>) -> crate::ApiError {
    crate::ApiError(auris_core::AurisError::other(msg.into()))
}

/// Sonde `whipper --version` (une fois, mis en cache paresseusement via status).
fn probe_whipper(bin: &str) -> (bool, String) {
    let mut c = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(bin).arg("--version");
        c
    } else {
        let mut c = std::process::Command::new(bin);
        c.arg("--version");
        c
    };
    match c.output() {
        Ok(o) if o.status.success() => (
            true,
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        ),
        Ok(o) => (false, String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(_) => (false, String::new()),
    }
}

/// Répertoire de sortie = première source locale active (le rip y atterrit).
fn local_output_dir(s: &AppState) -> Option<String> {
    s.sources
        .list()
        .into_iter()
        .find(|src| src.enabled && matches!(src.kind, auris_core::SourceKind::Local { .. }))
        .and_then(|src| match src.kind {
            auris_core::SourceKind::Local { path } => Some(path),
            _ => None,
        })
}

/// Parse une ligne de progression whipper : « 42% », « 3/12 », « track 3 »…
fn parse_progress(line: &str, s: &mut RipStatus) {
    let lower = line.to_lowercase();
    if let Some(pct) = extract_percent(line) {
        s.progress = pct;
    }
    if let Some((cur, total)) = extract_frac(line) {
        s.current_track = cur;
        s.total_tracks = s.total_tracks.max(total);
    } else if let Some(n) = extract_track_n(&lower) {
        s.current_track = n;
    }
    if (lower.contains("ripping") || lower.contains("encoding") || lower.contains("flac"))
        && s.phase == RipPhase::Idle
    {
        s.phase = RipPhase::Ripping;
    }
}

fn extract_percent(line: &str) -> Option<f64> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // remonte pour lire le nombre juste avant
            let mut j = i;
            while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') {
                j -= 1;
            }
            if let Ok(v) = line[j..i].parse::<f64>() {
                if (0.0..=100.0).contains(&v) {
                    return Some(v);
                }
            }
        }
        i += 1;
    }
    None
}

fn extract_frac(line: &str) -> Option<(u32, u32)> {
    // « 3/12 » ou « 03/12 »
    let b = line.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i].is_ascii_digit() && b[i + 1] == b'/' {
            let mut s = i;
            while s > 0 && b[s - 1].is_ascii_digit() {
                s -= 1;
            }
            if let Ok(cur) = line[s..i + 1].parse::<u32>() {
                let dstart = i + 2;
                let mut k = dstart;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > dstart {
                    if let Ok(total) = line[dstart..k].parse::<u32>() {
                        if cur >= 1 && total >= cur && total <= 999 {
                            return Some((cur, total));
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn extract_track_n(lower: &str) -> Option<u32> {
    for pat in ["track ", "piste "] {
        if let Some(pos) = lower.find(pat) {
            let rest = &lower[pos + pat.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Parse la sortie de `whipper cd info` : lignes «  1. 04:32  … » ou « 1: 04:32 ».
fn parse_toc(text: &str) -> (Vec<RipTrack>, u32) {
    let mut toc = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let b = t.as_bytes();
        if b.is_empty() || !b[0].is_ascii_digit() {
            continue;
        }
        // « 1. 04:32 » / « 1: 04:32 » / « 1) 04:32:12 »
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i >= b.len() || (b[i] != b'.' && b[i] != b':' && b[i] != b')') {
            continue;
        }
        let n: u32 = match t[..i].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let rest = t[i + 1..].trim_start();
        // durée mm:ss ou hh:mm:ss en début de reste
        if let Some((secs, dur_end)) = parse_leading_duration(rest) {
            // titre éventuel après la durée
            let title = rest
                .get(dur_end..)
                .map(|r| {
                    r.trim()
                        .trim_matches(|c| c == '-' || c == '–')
                        .trim()
                        .to_string()
                })
                .filter(|s| !s.is_empty());
            toc.push(RipTrack {
                n,
                duration_secs: secs,
                title,
            });
        }
    }
    let total = toc.iter().map(|t| t.n).max().unwrap_or(0);
    (toc, total)
}

/// Parse une durée « mm:ss » / « hh:mm:ss » en tête de chaîne. Retourne
/// (secondes, index de fin de la durée dans la chaîne).
fn parse_leading_duration(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= b.len() || b[i] != b':' {
        return None;
    }
    let head: u64 = s[..i].parse().ok()?;
    let rest = &s[i + 1..];
    let rb = rest.as_bytes();
    if rb.len() < 2 || !rb[0].is_ascii_digit() || !rb[1].is_ascii_digit() {
        return None;
    }
    let mins: u64 = rest[..2].parse().ok()?;
    let secs;
    let mut end = i + 1 + 2;
    // « hh:mm:ss » : un second « :xx » après les minutes
    if rest.as_bytes().get(2) == Some(&b':')
        && rb.len() >= 5
        && rb[3].is_ascii_digit()
        && rb[4].is_ascii_digit()
    {
        secs = head * 3600 + mins * 60 + rest[3..5].parse::<u64>().ok()?;
        end = i + 1 + 2 + 1 + 2;
    } else {
        secs = head * 60 + mins;
    }
    Some((secs as f64, end))
}

// ---------- handlers axum ----------

#[derive(Deserialize)]
pub struct RipBody {
    device: Option<String>,
}

pub async fn get_status(State(s): State<AppState>) -> Json<RipStatus> {
    Json(s.ripcd.status())
}

pub async fn post_info(
    State(s): State<AppState>,
    body: Option<Json<RipBody>>,
) -> ApiResult<Json<RipStatus>> {
    let dev = body.and_then(|b| b.0.device);
    Ok(Json(s.ripcd.info(dev).await?))
}

pub async fn post_rip(
    State(s): State<AppState>,
    body: Option<Json<RipBody>>,
) -> ApiResult<Json<RipStatus>> {
    let dev = body.and_then(|b| b.0.device);
    Ok(Json(s.ripcd.rip(&s, dev).await?))
}

pub async fn post_stop(State(s): State<AppState>) -> ApiResult<Json<RipStatus>> {
    Ok(Json(s.ripcd.stop().await?))
}

pub async fn post_eject(
    State(s): State<AppState>,
    body: Option<Json<RipBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let dev = body.and_then(|b| b.0.device);
    Ok(Json(s.ripcd.eject(dev).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> RipStatus {
        RipStatus::idle(true, "whipper 0.7.3".into())
    }

    #[test]
    fn parse_percent() {
        assert_eq!(extract_percent("  42%"), Some(42.0));
        assert_eq!(extract_percent("99.5% done"), Some(99.5));
        assert_eq!(extract_percent("no percent"), None);
        assert_eq!(extract_percent("123% out of range"), None);
    }

    #[test]
    fn parse_frac() {
        assert_eq!(extract_frac("Ripping 3/12 tracks"), Some((3, 12)));
        assert_eq!(extract_frac("01/09"), Some((1, 9)));
        assert_eq!(extract_frac("nope"), None);
    }

    #[test]
    fn parse_track_word() {
        let mut s = st();
        parse_progress("Ripping track 7 of 12", &mut s);
        assert_eq!(s.current_track, 7);
        assert_eq!(s.progress, 0.0);
        parse_progress("Track 55%", &mut s);
        assert_eq!(s.progress, 55.0);
    }

    #[test]
    fn parse_toc_lines() {
        let text = "Tracks:\n  1. 04:32  Pink Floyd - Time\n  2: 03:15\n  3) 1:02:45 - Long\n";
        let (toc, total) = parse_toc(text);
        assert_eq!(total, 3);
        assert_eq!(toc.len(), 3);
        assert_eq!(toc[0].n, 1);
        assert!((toc[0].duration_secs - 272.0).abs() < 0.01);
        assert_eq!(toc[0].title.as_deref(), Some("Pink Floyd - Time"));
        assert!((toc[2].duration_secs - 3765.0).abs() < 0.01);
    }

    #[test]
    fn push_log_tail() {
        let mut s = st();
        for i in 0..250 {
            push_log(&mut s, &format!("ligne {i}"));
        }
        assert_eq!(s.log.len(), LOG_TAIL);
        assert_eq!(s.log.first().unwrap(), "ligne 50");
        assert_eq!(s.log.last().unwrap(), "ligne 249");
    }
}
