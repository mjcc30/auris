//! Enregistrement des webradios « comme à l'ancienne » — #101.
//!
//! Une connexion dédiée est ouverte sur le flux de la radio (la lecture
//! continue sur sa propre connexion, l'enregistrement ne la perturbe pas) ;
//! les métadonnées ICY sont retirées du flux et l'audio est écrit dans des
//! fichiers. À chaque changement de `StreamTitle`, le fichier courant est
//! fermé — nommé d'après le titre — et le suivant est ouvert : un morceau =
//! un fichier. Les enregistrements tombent dans
//! `<source locale active>/Enregistrements radio/<nom de la radio>/` et la
//! bibliothèque est rescannée à l'arrêt de l'enregistrement.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use auris_core::AurisError;
use auris_sources::live::IcyParser;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{ApiResult, AppState};

/// Sous-dossier (dans la source locale) où tombent les enregistrements.
pub const RECORD_SUBDIR: &str = "Enregistrements radio";

/// État exposé à l'UI (`GET /api/radio/record/status`, réponses start/stop).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordStatus {
    pub recording: bool,
    pub url: Option<String>,
    pub started_at: Option<i64>,
    pub elapsed_secs: u64,
    /// `StreamTitle` ICY du morceau en cours d'enregistrement.
    pub current_title: Option<String>,
    pub bytes_written: u64,
    /// Noms des fichiers finalisés (dans `output_dir`).
    pub files: Vec<String>,
    pub output_dir: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Shared {
    current_title: Option<String>,
    bytes: u64,
    files: Vec<String>,
    dir: Option<String>,
    error: Option<String>,
}

struct Session {
    url: String,
    started_at: i64,
    output_dir: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
}

/// Hub d'enregistrement radio (#101) : une seule session à la fois.
#[derive(Default)]
pub struct RecorderHub {
    inner: Mutex<Option<Session>>,
}

impl RecorderHub {
    pub fn new() -> Arc<Self> {
        Arc::new(RecorderHub {
            inner: Mutex::new(None),
        })
    }

    pub fn status(&self) -> RecordStatus {
        let inner = self.inner.lock().unwrap();
        match &*inner {
            None => RecordStatus {
                recording: false,
                url: None,
                started_at: None,
                elapsed_secs: 0,
                current_title: None,
                bytes_written: 0,
                files: Vec::new(),
                output_dir: None,
                error: None,
            },
            Some(s) => {
                let sh = s.shared.lock().unwrap();
                RecordStatus {
                    recording: true,
                    url: Some(s.url.clone()),
                    started_at: Some(s.started_at),
                    elapsed_secs: (chrono::Utc::now().timestamp() - s.started_at).max(0) as u64,
                    current_title: sh.current_title.clone(),
                    bytes_written: sh.bytes,
                    files: sh.files.clone(),
                    output_dir: sh.dir.clone().or(Some(s.output_dir.clone())),
                    error: sh.error.clone(),
                }
            }
        }
    }

    /// Démarre l'enregistrement du flux `url` dans `output_dir` (thread dédié,
    /// retour immédiat — la requête HTTP initiale se fait dans le thread).
    pub fn start(&self, url: &str, output_dir: &Path) -> Result<RecordStatus, AurisError> {
        if !auris_sources::is_http_uri(url) {
            return Err(AurisError::InvalidUri(url.to_string()));
        }
        fs::create_dir_all(output_dir)
            .map_err(|e| AurisError::other(format!("création {}: {e}", output_dir.display())))?;
        let mut inner = self.inner.lock().unwrap();
        if inner.is_some() {
            return Err(AurisError::Other(
                "un enregistrement est déjà en cours".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let url2 = url.to_string();
        let dir2 = output_dir.to_path_buf();
        let stop2 = stop.clone();
        let shared2 = shared.clone();
        let handle = std::thread::Builder::new()
            .name("auris-recorder".into())
            .spawn(move || record_stream(&url2, &dir2, &stop2, &shared2))
            .map_err(|e| AurisError::other(format!("spawn recorder: {e}")))?;
        let started_at = chrono::Utc::now().timestamp();
        let output_dir_str = output_dir.to_string_lossy().to_string();
        inner.replace(Session {
            url: url.to_string(),
            started_at,
            output_dir: output_dir_str.clone(),
            stop,
            handle: Some(handle),
            shared,
        });
        drop(inner);
        Ok(self.status())
    }

    /// Arrête l'enregistrement en cours et finalise le dernier fichier.
    /// Bloquant (joint le thread, ≤ read_timeout) : à appeler via
    /// `tokio::task::spawn_blocking`.
    pub fn stop(&self) -> Result<RecordStatus, AurisError> {
        let mut session = self
            .inner
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| AurisError::Other("aucun enregistrement en cours".into()))?;
        session.stop.store(true, Ordering::SeqCst);
        if let Some(h) = session.handle.take() {
            // joint avec timeout : un flux mort sans RST peut bloquer read()
            // indéfiniment ; le thread est alors détaché, il s'arrêtera au
            // prochain bloc (flag stop) ou au keepalive TCP
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !h.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        let sh = session.shared.lock().unwrap();
        Ok(RecordStatus {
            recording: false,
            url: Some(session.url.clone()),
            started_at: Some(session.started_at),
            elapsed_secs: (chrono::Utc::now().timestamp() - session.started_at).max(0) as u64,
            current_title: sh.current_title.clone(),
            bytes_written: sh.bytes,
            files: sh.files.clone(),
            output_dir: sh.dir.clone().or(Some(session.output_dir.clone())),
            error: sh.error.clone(),
        })
    }
}

/// Fichier segment en cours d'écriture (nom temporaire, renommé à la fin).
struct SegmentFile {
    path: PathBuf,
    file: Option<File>,
}

impl SegmentFile {
    fn create_temp(dir: &Path, ext: &str) -> Result<Self, AurisError> {
        let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S");
        let path = unique_path(dir, &format!("REC {stamp}"), ext);
        let f = File::create(&path)
            .map_err(|e| AurisError::other(format!("création {}: {e}", path.display())))?;
        Ok(SegmentFile {
            path,
            file: Some(f),
        })
    }
}

/// Boucle d'enregistrement : lit le flux, strippe l'ICY, découpe par titre.
fn record_stream(url: &str, out_dir: &Path, stop: &AtomicBool, shared: &Arc<Mutex<Shared>>) {
    if let Err(e) = try_record_stream(url, out_dir, stop, shared) {
        shared.lock().unwrap().error = Some(e.to_string());
        tracing::warn!("enregistrement radio {url}: {e}");
    }
}

fn try_record_stream(
    url: &str,
    out_dir: &Path,
    stop: &AtomicBool,
    shared: &Arc<Mutex<Shared>>,
) -> Result<(), AurisError> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // pas de timeout de lecture global (un flux live ne se termine pas) ;
        // le keepalive TCP fait tomber un flux mort sans RST, et le flag
        // `stop` estrelu entre deux blocs -> arrêt effectif au prochain bloc
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .user_agent(concat!("auris/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AurisError::Network(format!("client recorder: {e}")))?;
    let resp = client
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .map_err(|e| AurisError::Network(format!("GET {url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AurisError::Network(format!("GET {url}: HTTP {status}")));
    }
    let headers = resp.headers().clone();
    let ext = ext_from_content_type(headers.get("content-type").and_then(|v| v.to_str().ok()));
    let metaint = headers
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    // sous-dossier par radio (en-tête icy-name) si présent
    let dir = match headers
        .get("icy-name")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_filename)
    {
        Some(name) if !name.is_empty() => {
            let d = out_dir.join(name);
            fs::create_dir_all(&d)
                .map_err(|e| AurisError::other(format!("création {}: {e}", d.display())))?;
            d
        }
        _ => out_dir.to_path_buf(),
    };
    shared.lock().unwrap().dir = Some(dir.to_string_lossy().to_string());

    let mut parser = IcyParser::new(metaint);
    let mut resp = resp;
    let mut segment_title: Option<String> = None;
    let mut segment = SegmentFile::create_temp(&dir, ext)?;
    let mut raw = [0u8; 16 * 1024];
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match resp.read(&mut raw) {
            Ok(0) => break,
            Ok(n) => {
                // événements ordonnés (audio / titre) : chaque octet audio est
                // attribué au bon segment, même si un bloc contient les deux
                enum Ev {
                    Audio(Vec<u8>),
                    Title(String),
                }
                // RefCell : les deux callbacks pushent dans le même journal
                let evs = std::cell::RefCell::new(Vec::new());
                parser.push_segments(
                    &raw[..n],
                    |a| evs.borrow_mut().push(Ev::Audio(a.to_vec())),
                    |t| evs.borrow_mut().push(Ev::Title(t)),
                );
                for ev in evs.into_inner() {
                    match ev {
                        Ev::Audio(a) => {
                            if !a.is_empty() {
                                if let Some(f) = segment.file.as_mut() {
                                    f.write_all(&a).map_err(|e| {
                                        AurisError::other(format!("écriture segment: {e}"))
                                    })?;
                                }
                                shared.lock().unwrap().bytes += a.len() as u64;
                            }
                        }
                        Ev::Title(t) => {
                            if segment_title.as_deref() != Some(t.as_str()) {
                                // nouveau morceau : on ferme le segment courant
                                finalize_segment(
                                    &mut segment,
                                    &dir,
                                    segment_title.take(),
                                    ext,
                                    shared,
                                );
                                segment_title = Some(t.clone());
                                shared.lock().unwrap().current_title = Some(t);
                                segment = SegmentFile::create_temp(&dir, ext)?;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                return Err(AurisError::Network(format!("lecture flux {url}: {e}")));
            }
        }
    }
    finalize_segment(&mut segment, &dir, segment_title, ext, shared);
    Ok(())
}

/// Ferme le segment courant : fichier vide -> supprimé ; sinon renommé d'après
/// le `StreamTitle` du morceau (ou gardé tel quel) et ajouté à la liste.
fn finalize_segment(
    segment: &mut SegmentFile,
    dir: &Path,
    title: Option<String>,
    ext: &str,
    shared: &Arc<Mutex<Shared>>,
) {
    let tmp = segment.path.clone();
    let size = segment
        .file
        .as_ref()
        .and_then(|f| f.metadata().ok())
        .map(|m| m.len())
        .unwrap_or(0);
    segment.file.take();
    if size == 0 {
        let _ = fs::remove_file(&tmp);
        return;
    }
    let stem = match &title {
        Some(t) => sanitize_filename(t),
        None => tmp
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "REC".into()),
    };
    // unique, mais sans compter le fichier temporaire lui-même
    let mut final_path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while final_path != tmp && final_path.exists() {
        final_path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    if fs::rename(&tmp, &final_path).is_ok() {
        if let Some(name) = final_path.file_name() {
            shared
                .lock()
                .unwrap()
                .files
                .push(name.to_string_lossy().to_string());
        }
    }
}

/// Nettoie un `StreamTitle` pour en faire un nom de fichier.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if (c as u32) < 32 => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches([' ', '.']).to_string();
    let mut s = if trimmed.is_empty() {
        "Sans titre".to_string()
    } else {
        trimmed
    };
    if s.chars().count() > 100 {
        s = s
            .chars()
            .take(100)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    s
}

/// Extension déduite du `Content-Type` du flux (défaut mp3, le plus courant).
fn ext_from_content_type(ct: Option<&str>) -> &'static str {
    let ct = ct.unwrap_or("").to_ascii_lowercase();
    if ct.contains("mpeg") || ct.contains("mp3") {
        "mp3"
    } else if ct.contains("aac") {
        "aac"
    } else if ct.contains("ogg") {
        "ogg"
    } else if ct.contains("flac") {
        "flac"
    } else if ct.contains("wav") {
        "wav"
    } else if ct.contains("mp4") || ct.contains("m4a") {
        "m4a"
    } else if ct.contains("webm") {
        "webm"
    } else if ct.contains("aiff") {
        "aiff"
    } else {
        "mp3"
    }
}

/// Chemin `dir/<stem>.<ext>` unique (ajoute « (2) », « (3) »… si besoin).
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    candidate
}

/// Racine d'enregistrement = première source locale active (+ sous-dossier).
fn local_record_root(s: &AppState) -> Option<PathBuf> {
    s.sources
        .list()
        .into_iter()
        .find(|src| src.enabled && matches!(src.kind, auris_core::SourceKind::Local { .. }))
        .and_then(|src| match src.kind {
            auris_core::SourceKind::Local { path } => Some(PathBuf::from(path)),
            _ => None,
        })
}

#[derive(Debug, Deserialize)]
pub struct StartBody {
    pub url: String,
}

/// `POST /api/radio/record/start` — démarre l'enregistrement d'un flux live.
pub async fn post_start(
    State(s): State<AppState>,
    Json(b): Json<StartBody>,
) -> ApiResult<Json<RecordStatus>> {
    let root = local_record_root(&s).ok_or_else(|| {
        AurisError::Other(
            "aucune source locale active : ajoutez un dossier musique dans Sources pour y enregistrer"
                .into(),
        )
    })?;
    let st = s.recorder.start(&b.url, &root.join(RECORD_SUBDIR))?;
    Ok(Json(st))
}

/// `POST /api/radio/record/stop` — arrête et finalise, puis rescan bibliothèque.
pub async fn post_stop(State(s): State<AppState>) -> ApiResult<Json<RecordStatus>> {
    let hub = s.recorder.clone();
    let st = tokio::task::spawn_blocking(move || hub.stop())
        .await
        .map_err(|e| AurisError::other(format!("join stop: {e}")))??;
    // les morceaux enregistrés doivent apparaître dans la bibliothèque
    let _ = s.library.scan_all();
    Ok(Json(st))
}

/// `GET /api/radio/record/status` — état courant (polling UI).
pub async fn get_status(State(s): State<AppState>) -> Json<RecordStatus> {
    Json(s.recorder.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::net::TcpListener;

    #[test]
    fn sanitize_ok() {
        assert_eq!(sanitize_filename("Artiste - Titre"), "Artiste - Titre");
        assert_eq!(sanitize_filename("AC/DC: Live?"), "AC DC Live");
        assert_eq!(sanitize_filename("  ...  "), "Sans titre");
        assert_eq!(sanitize_filename(""), "Sans titre");
        let long = "x".repeat(200);
        assert!(sanitize_filename(&long).chars().count() <= 100);
    }

    #[test]
    fn ext_ok() {
        assert_eq!(ext_from_content_type(Some("audio/mpeg")), "mp3");
        assert_eq!(ext_from_content_type(Some("audio/aac")), "aac");
        assert_eq!(ext_from_content_type(Some("audio/ogg; codecs=opus")), "ogg");
        assert_eq!(ext_from_content_type(None), "mp3");
    }

    #[test]
    fn unique_ok() {
        let dir = std::env::temp_dir().join(format!("auris-rec-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let p1 = unique_path(&dir, "Morceau", "mp3");
        File::create(&p1).unwrap();
        let p2 = unique_path(&dir, "Morceau", "mp3");
        assert_ne!(p1, p2);
        assert!(p2.file_name().unwrap().to_string_lossy().contains("(2)"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_refuse_non_http_et_double() {
        let hub = RecorderHub::new();
        let dir = std::env::temp_dir().join(format!("auris-rec-test-{}", uuid::Uuid::new_v4()));
        assert!(hub.start("src://nas/x.mp3", &dir).is_err());
        // double start : on simule une session en cours via un flux bidon
        // (start valide mais connexion impossible -> thread mort rapidement)
        let hub2 = RecorderHub::new();
        let r = hub2.start("http://127.0.0.1:1/x", &dir);
        assert!(r.is_ok()); // le thread échoue en arrière-plan, la session existe
        assert!(hub2.start("http://127.0.0.1:1/y", &dir).is_err()); // déjà en cours
        let st = hub2.stop().unwrap();
        assert!(!st.recording);
        assert!(st.error.is_some()); // connexion refusée
        let _ = fs::remove_dir_all(&dir);
    }

    /// Serveur HTTP minimal qui envoie un flux ICY truqué (metaint = 16) :
    /// 16 octets audio, bloc meta « StreamTitle='A - B'; », 16 octets audio,
    /// bloc meta « StreamTitle='C - D'; », 16 octets audio, puis fin.
    fn spawn_fake_icy() -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut stream = stream;
            let mut req = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            // lit les en-têtes jusqu'à la ligne vide
            loop {
                line.clear();
                use std::io::BufRead;
                if req.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line.trim().is_empty() {
                    break;
                }
            }
            let meta = |t: &str| {
                let mut m = format!("StreamTitle='{t}';").into_bytes();
                while m.len() % 16 != 0 {
                    m.push(0);
                }
                m
            };
            let mut body = Vec::new();
            body.extend_from_slice(&[b'A'; 16]);
            let m1 = meta("A - B");
            body.push((m1.len() / 16) as u8);
            body.extend_from_slice(&m1);
            body.extend_from_slice(&[b'B'; 16]);
            let m2 = meta("C - D");
            body.push((m2.len() / 16) as u8);
            body.extend_from_slice(&m2);
            body.extend_from_slice(&[b'C'; 16]);
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nicy-metaint: 16\r\nicy-name: TestRadio\r\nConnection: close\r\n\r\n";
            stream.write_all(resp.as_bytes()).unwrap();
            // envoi par petits morceaux avec pause, pour que chaque read() du
            // recorder voie un seul bloc (audio OU meta) à la fois
            let mut i = 0;
            while i < body.len() {
                let end = (i + 16).min(body.len());
                stream.write_all(&body[i..end]).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(Duration::from_millis(30));
                i = end;
            }
            // fermeture propre : FIN explicite puis on draine ce que le client
            // enverrait encore, sinon le noyau peut envoyer un RST (données
            // non lues) et le client perd la fin du flux
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut sink = [0u8; 256];
            while let Ok(n) = stream.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
        });
        (format!("http://{addr}/flux"), h)
    }

    #[test]
    fn enregistre_et_decoupe_par_titre() {
        let (url, server) = spawn_fake_icy();
        let dir = std::env::temp_dir().join(format!("auris-rec-e2e-{}", uuid::Uuid::new_v4()));
        let hub = RecorderHub::new();
        let st = hub.start(&url, &dir).unwrap();
        assert!(st.recording);
        // attend la fin du flux (le thread termine seul)
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let done = {
                let inner = hub.inner.lock().unwrap();
                inner
                    .as_ref()
                    .map(|s| s.handle.as_ref().unwrap().is_finished())
                    .unwrap_or(true)
            };
            if done || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let st = hub.stop().unwrap();
        server.join().unwrap();
        // 3 segments : le partiel initial sans titre (REC …), puis un fichier
        // par StreamTitle (« A - B », « C - D »)
        assert_eq!(st.files.len(), 3, "fichiers: {:?} err: {:?}", st.files, st.error);
        assert!(
            st.files.iter().any(|f| f.starts_with("REC ")),
            "{:?}",
            st.files
        );
        assert!(
            st.files.iter().any(|f| f.starts_with("A - B.")),
            "{:?}",
            st.files
        );
        assert!(
            st.files.iter().any(|f| f.starts_with("C - D.")),
            "{:?}",
            st.files
        );
        // sous-dossier icy-name + contenu audio non vide
        let sub = dir.join("TestRadio");
        assert!(sub.is_dir());
        for f in &st.files {
            let p = sub.join(f);
            assert!(p.metadata().unwrap().len() >= 16, "{f} trop petit");
        }
        assert!(st.bytes_written >= 48);
        let _ = fs::remove_dir_all(&dir);
    }
}
