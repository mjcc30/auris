//! Spotify Connect via librespot (processus enfant) — #24.
//!
//! Auris apparaît comme enceinte dans l'app Spotify : le flux est fourni par
//! Spotify au client authentifié (aucun DRM contourné). Le binaire `librespot`
//! est invoqué en processus enfant (introuvable = fonctionnalité indisponible
//! mais l'API répond) ; sur Windows il est lancé via `cmd /C` (permet un mock
//! `.cmd` dans le PATH pour les tests).
//!
//! Les événements de lecture (`--onevent`) sont relayés vers
//! `POST /api/spotify/event` par un petit script écrit dans le data-dir : à
//! « Playing », le moteur Auris est mis en pause pour laisser la place à
//! Spotify (les deux ne peuvent pas sortir sur la même carte son).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use auris_core::SpotifySettings;

use crate::{ApiResult, AppState};

const LOG_TAIL: usize = 200;

/// État exposé à l'UI (polling `GET /api/spotify/status`).
#[derive(Debug, Clone, Serialize)]
pub struct SpotStatus {
    pub available: bool,
    pub version: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub device_name: String,
    pub backend: String,
    pub bitrate: u16,
    pub last_event: Option<String>,
    pub track_id: Option<String>,
    pub position_ms: Option<String>,
    pub log: Vec<String>,
    pub error: Option<String>,
}

impl SpotStatus {
    fn idle(available: bool, version: String) -> Self {
        SpotStatus {
            available,
            version,
            running: false,
            pid: None,
            device_name: String::new(),
            backend: String::new(),
            bitrate: 0,
            last_event: None,
            track_id: None,
            position_ms: None,
            log: Vec::new(),
            error: None,
        }
    }
}

#[derive(Default)]
struct Inner {
    status: Option<SpotStatus>,
    child: Option<Arc<tokio::sync::Mutex<tokio::process::Child>>>,
}

/// Hub Spotify Connect (#24).
pub struct SpotHub {
    inner: Mutex<Inner>,
    /// Nom/chemin du binaire librespot (env `AURIS_LIBRESPOT_BIN` ou « librespot »).
    bin: String,
}

impl SpotHub {
    pub fn new() -> Arc<Self> {
        let bin = std::env::var("AURIS_LIBRESPOT_BIN")
            .ok()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| "librespot".to_string());
        Arc::new(SpotHub {
            inner: Mutex::new(Inner::default()),
            bin,
        })
    }

    pub fn status(&self) -> SpotStatus {
        let inner = self.inner.lock().unwrap();
        match &inner.status {
            Some(s) => s.clone(),
            None => {
                let (ok, ver) = probe_librespot(&self.bin);
                SpotStatus::idle(ok, ver)
            }
        }
    }

    fn with_status<R>(&self, f: impl FnOnce(&mut SpotStatus) -> R) -> R {
        let mut inner = self.inner.lock().unwrap();
        if inner.status.is_none() {
            let (ok, ver) = probe_librespot(&self.bin);
            inner.status = Some(SpotStatus::idle(ok, ver));
        }
        f(inner.status.as_mut().unwrap())
    }

    /// Démarre librespot avec les réglages `[settings.spotify]`.
    pub async fn start(self: &Arc<Self>, s: &AppState) -> ApiResult<SpotStatus> {
        {
            let inner = self.inner.lock().unwrap();
            if let Some(st) = &inner.status {
                if st.running {
                    return Ok(st.clone());
                }
            }
        }
        let cfg = s.settings.read().unwrap().spotify.clone();
        if !cfg.enabled {
            return Err(err(
                "Spotify Connect est désactivé (Réglages > Spotify Connect)",
            ));
        }
        if cfg.username.trim().is_empty() && cfg.credentials_path.trim().is_empty() {
            return Err(err(
                "renseignez un compte Spotify (identifiants ou credentials.json)",
            ));
        }
        let (avail, version) = probe_librespot(&self.bin);
        if !avail {
            return Err(err(format!(
                "librespot introuvable (`{}`) — installez-le ou définissez AURIS_LIBRESPOT_BIN",
                self.bin
            )));
        }
        let port = s
            .bound_addr
            .read()
            .unwrap()
            .map(|a| a.port())
            .unwrap_or(3000);
        let script = write_event_script(&s.data_dir, port)?;
        let fallback_name = s.settings.read().unwrap().system.device_name.clone();
        let args = build_args(&cfg, &s.data_dir, &script, &fallback_name);

        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&self.bin);
            for a in &args {
                c.arg(a);
            }
            c
        } else {
            let mut c = Command::new(&self.bin);
            for a in &args {
                c.arg(a);
            }
            c
        };
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| err(format!("spawn librespot: {e}")))?;
        let pid = child.id().unwrap_or(0);
        let child = Arc::new(tokio::sync::Mutex::new(child));

        let backend = default_backend(&cfg);
        let device_name = device_name(&cfg, &fallback_name);
        self.with_status(|st| {
            st.available = true;
            st.version = version.clone();
            st.running = true;
            st.pid = Some(pid);
            st.device_name = device_name.clone();
            st.backend = backend.clone();
            st.bitrate = cfg.bitrate;
            st.error = None;
            st.log.clear();
            push_log(&mut st.log, format!("librespot démarré (pid {pid})"));
            push_log(&mut st.log, format!("args: {}", args.join(" ")));
        });
        {
            self.inner.lock().unwrap().child = Some(child.clone());
        }
        let hub = self.clone();
        tokio::spawn(async move {
            hub.read_loop(child).await;
        });
        Ok(self.status())
    }

    /// Arrête librespot (kill du processus).
    pub async fn stop(&self) -> ApiResult<SpotStatus> {
        let child = { self.inner.lock().unwrap().child.take() };
        if let Some(c) = child {
            let mut c = c.lock().await;
            let _ = c.kill().await;
        }
        self.with_status(|st| {
            if st.running {
                st.running = false;
                st.pid = None;
                st.last_event = Some("stopped".to_string());
                push_log(&mut st.log, "arrêté par l'utilisateur".to_string());
            }
        });
        Ok(self.status())
    }

    /// Lit stdout/stderr, met à jour le log, détecte la fin du processus.
    /// Le mutex du child n'est jamais maintenu pendant `wait()` (try_wait +
    /// sleep) pour laisser `stop()` tuer le processus.
    async fn read_loop(self: &Arc<Self>, child: Arc<tokio::sync::Mutex<tokio::process::Child>>) {
        let streams = {
            let mut c = child.lock().await;
            (c.stdout.take(), c.stderr.take())
        };
        let (out, err) = streams;
        let mut tasks = Vec::new();
        if let Some(s) = out {
            tasks.push(spawn_reader(self.clone(), s));
        }
        if let Some(s) = err {
            tasks.push(spawn_reader(self.clone(), s));
        }
        loop {
            let exited = {
                let mut c = child.lock().await;
                match c.try_wait() {
                    Ok(Some(status)) => Some(status),
                    Ok(None) => None,
                    Err(e) => {
                        self.with_status(|st| {
                            st.running = false;
                            st.error = Some(format!("wait librespot: {e}"));
                        });
                        None
                    }
                }
            };
            match exited {
                Some(status) => {
                    let code = status.code();
                    self.with_status(|st| {
                        st.running = false;
                        st.pid = None;
                        push_log(&mut st.log, format!("librespot terminé (code {code:?})"));
                        if !status.success() {
                            st.error = Some(format!(
                                "librespot s'est arrêté anormalement (code {code:?})"
                            ));
                        }
                    });
                    break;
                }
                None => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
        self.inner.lock().unwrap().child = None;
        for t in tasks {
            let _ = t.await;
        }
    }
}

fn err(msg: impl Into<String>) -> crate::ApiError {
    crate::ApiError(auris_core::AurisError::other(msg.into()))
}

/// Tâche de lecture d'un flux (stdout ou stderr) vers le tail du log.
fn spawn_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    hub: Arc<SpotHub>,
    stream: R,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            hub.with_status(|st| push_log(&mut st.log, line));
        }
    })
}

fn push_log(log: &mut Vec<String>, line: String) {
    log.push(line);
    if log.len() > LOG_TAIL {
        let extra = log.len() - LOG_TAIL;
        log.drain(..extra);
    }
}

/// Backend par défaut : rodio sur Windows, alsa ailleurs.
fn default_backend(cfg: &SpotifySettings) -> String {
    let b = cfg.backend.trim();
    if b.is_empty() {
        if cfg!(windows) {
            "rodio".to_string()
        } else {
            "alsa".to_string()
        }
    } else {
        b.to_string()
    }
}

/// Nom de l'enceinte affiché dans l'app Spotify.
fn device_name(cfg: &SpotifySettings, fallback: &str) -> String {
    let n = cfg.device_name.trim();
    if n.is_empty() {
        fallback.to_string()
    } else {
        n.to_string()
    }
}

/// Construit la ligne de commande librespot (testable unitairement).
fn build_args(
    cfg: &SpotifySettings,
    data_dir: &Path,
    script: &Path,
    fallback_name: &str,
) -> Vec<String> {
    let backend = default_backend(cfg);
    let mut a = vec![
        "--name".to_string(),
        device_name(cfg, fallback_name),
        "--device-type".to_string(),
        "speaker".to_string(),
        "--backend".to_string(),
        backend.clone(),
    ];
    if backend == "alsa" {
        let dev = cfg.alsa_device.trim();
        a.push("--device".to_string());
        a.push(if dev.is_empty() {
            "default".to_string()
        } else {
            dev.to_string()
        });
    }
    a.push("--bitrate".to_string());
    a.push(cfg.bitrate.to_string());
    let cache = if cfg.cache_dir.trim().is_empty() {
        data_dir.join("spotify-cache")
    } else {
        PathBuf::from(cfg.cache_dir.trim())
    };
    a.push("--cache".to_string());
    a.push(cache.to_string_lossy().into_owned());
    a.push("--onevent".to_string());
    a.push(script.to_string_lossy().into_owned());
    let cred = cfg.credentials_path.trim();
    if !cred.is_empty() {
        a.push("--credentials".to_string());
        a.push(cred.to_string());
    } else {
        a.push("--username".to_string());
        a.push(cfg.username.trim().to_string());
        if !cfg.password.is_empty() {
            a.push("--password".to_string());
            a.push(cfg.password.clone());
        }
    }
    a
}

/// Écrit le script `--onevent` qui relaie les événements librespot vers l'API.
fn write_event_script(data_dir: &Path, port: u16) -> ApiResult<PathBuf> {
    std::fs::create_dir_all(data_dir).map_err(|e| err(format!("data-dir: {e}")))?;
    let (name, content) = if cfg!(windows) {
        (
            "auris-spotify-event.cmd",
            format!(
                "@echo off\r\ncurl -s -X POST \"http://127.0.0.1:{port}/api/spotify/event\" -H \"Content-Type: application/json\" -d \"{{\\\"event\\\":\\\"%PLAYER_EVENT%\\\",\\\"track_id\\\":\\\"%TRACK_ID%\\\",\\\"position_ms\\\":\\\"%POSITION_MS%\\\"}}\" >nul 2>&1\r\n"
            ),
        )
    } else {
        (
            "auris-spotify-event.sh",
            format!(
                "#!/bin/sh\ncurl -s -X POST \"http://127.0.0.1:{port}/api/spotify/event\" -H \"Content-Type: application/json\" -d \"{{\\\"event\\\":\\\"$PLAYER_EVENT\\\",\\\"track_id\\\":\\\"$TRACK_ID\\\",\\\"position_ms\\\":\\\"$POSITION_MS\\\"}}\" >/dev/null 2>&1\n"
            ),
        )
    };
    let path = data_dir.join(name);
    std::fs::write(&path, content).map_err(|e| err(format!("écriture {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(path)
}

/// Sonde `librespot --version`. Retourne (disponible, version).
fn probe_librespot(bin: &str) -> (bool, String) {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(bin).arg("--version");
        c
    } else {
        let mut c = std::process::Command::new(bin);
        c.arg("--version");
        c
    };
    let out = match cmd.output() {
        Ok(o) => o,
        Err(_) => return (false, String::new()),
    };
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), parse_version(&text))
}

/// Extrait la version d'une sortie « librespot 0.6.0 (...) ».
fn parse_version(text: &str) -> String {
    for tok in text.split_whitespace() {
        if tok
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return tok
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.')
                .to_string();
        }
    }
    String::new()
}

// ---------- handlers axum ----------

#[derive(Deserialize)]
pub struct SpotEvent {
    event: Option<String>,
    track_id: Option<String>,
    position_ms: Option<String>,
}

pub async fn get_status(State(s): State<AppState>) -> Json<SpotStatus> {
    Json(s.spotify.status())
}

pub async fn post_start(State(s): State<AppState>) -> ApiResult<Json<SpotStatus>> {
    let st = s.spotify.start(&s).await?;
    Ok(Json(st))
}

pub async fn post_stop(State(s): State<AppState>) -> ApiResult<Json<SpotStatus>> {
    let st = s.spotify.stop().await?;
    Ok(Json(st))
}

/// Webhook appelé par le script `--onevent` de librespot à chaque événement.
pub async fn post_event(
    State(s): State<AppState>,
    body: Option<Json<SpotEvent>>,
) -> ApiResult<Json<SpotStatus>> {
    let ev = body
        .as_ref()
        .and_then(|b| b.event.clone())
        .unwrap_or_default();
    let track_id = body.as_ref().and_then(|b| b.track_id.clone());
    let position_ms = body.as_ref().and_then(|b| b.position_ms.clone());
    s.spotify.with_status(|st| {
        st.last_event = Some(ev.clone());
        if let Some(t) = &track_id {
            st.track_id = Some(t.clone());
        }
        if let Some(p) = &position_ms {
            st.position_ms = Some(p.clone());
        }
        push_log(&mut st.log, format!("event: {ev}"));
    });
    // À « Playing », on met le moteur Auris en pause : librespot sort sur la
    // même carte son, les deux ne peuvent pas jouer en même temps.
    if ev.eq_ignore_ascii_case("playing") {
        s.engine.pause();
    }
    Ok(Json(s.spotify.status()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SpotifySettings {
        SpotifySettings {
            enabled: true,
            username: "user".into(),
            password: "secret".into(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_version_ok() {
        assert_eq!(parse_version("librespot 0.6.0 (abc)"), "0.6.0");
        assert_eq!(parse_version("Version: librespot 0.4.2"), "0.4.2");
        assert_eq!(parse_version("pas de version"), "");
    }

    #[test]
    fn build_args_username_password() {
        let c = cfg();
        let args = build_args(&c, Path::new("/data"), Path::new("/data/ev.sh"), "auris");
        let joined = args.join(" ");
        assert!(joined.contains("--name auris"));
        assert!(joined.contains("--device-type speaker"));
        assert!(joined.contains("--bitrate 320"));
        assert!(joined.contains("--username user"));
        assert!(joined.contains("--password secret"));
        assert!(joined.contains("--onevent"));
        assert!(joined.contains("ev.sh"));
        assert!(joined.contains("--cache"));
        assert!(joined.contains("spotify-cache"));
        if cfg!(windows) {
            assert!(joined.contains("--backend rodio"));
        } else {
            assert!(joined.contains("--backend alsa"));
            assert!(joined.contains("--device default"));
        }
        assert!(!joined.contains("--credentials"));
    }

    #[test]
    fn build_args_credentials_et_overrides() {
        let mut c = cfg();
        c.username.clear();
        c.password.clear();
        c.credentials_path = "/home/auris/credentials.json".into();
        c.device_name = "Salon".into();
        c.bitrate = 160;
        c.backend = "alsa".into();
        c.alsa_device = "hw:0,0".into();
        let args = build_args(&c, Path::new("/data"), Path::new("/data/ev.sh"), "auris");
        let joined = args.join(" ");
        assert!(joined.contains("--name Salon"));
        assert!(joined.contains("--credentials /home/auris/credentials.json"));
        assert!(joined.contains("--bitrate 160"));
        assert!(joined.contains("--device hw:0,0"));
        assert!(!joined.contains("--username"));
    }

    #[test]
    fn event_script_contient_le_port() {
        let dir = std::env::temp_dir().join(format!("auris-spotify-test-{}", std::process::id()));
        let p = match write_event_script(&dir, 3999) {
            Ok(p) => p,
            Err(_) => panic!("write_event_script a échoué"),
        };
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(content.contains("http://127.0.0.1:3999/api/spotify/event"));
        assert!(content.contains("PLAYER_EVENT"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_log_tail() {
        let mut log = Vec::new();
        for i in 0..250 {
            push_log(&mut log, format!("ligne {i}"));
        }
        assert_eq!(log.len(), LOG_TAIL);
        assert_eq!(log[0], "ligne 50");
        assert_eq!(log[LOG_TAIL - 1], "ligne 249");
    }
}
