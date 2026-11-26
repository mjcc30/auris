//! Flux live HTTP(S) (webradios) : lecture infinie avec métadonnées ICY
//! (`StreamTitle='...'`) extraites du flux et publiées dans un registre global.
//!
//! Contrairement à [`crate::http::HttpFile`] (fichier seekable de taille connue),
//! un flux live n'a pas de `Content-Length` : on le lit séquentiellement dans un
//! tampon partagé (thread de réception en arrière-plan) et on autorise les seeks
//! uniquement dans la fenêtre retenue à l'arrière du tampon.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use auris_core::{AurisError, Result};

use crate::is_http_uri;

/// Registre global URL -> titre ICY courant (`StreamTitle`).
static RADIO_TITLES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Titre ICY courant d'une radio (None si aucune métadonnée reçue).
pub fn radio_title(url: &str) -> Option<String> {
    RADIO_TITLES.lock().unwrap().get(url).cloned()
}

fn set_radio_title(url: &str, title: Option<String>) {
    let mut m = RADIO_TITLES.lock().unwrap();
    match title {
        Some(t) if !t.is_empty() => {
            m.insert(url.to_string(), t);
        }
        _ => {
            m.remove(url);
        }
    }
}

/// Extrait `StreamTitle` d'un bloc de métadonnées ICY (`StreamTitle='...';`).
pub fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(meta);
    let start = s.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &s[start..];
    let end = rest.find('\'').unwrap_or(rest.len());
    let title = rest[..end].trim_end_matches('\0').trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Machine à états ICY pure (testable sans réseau) : découpe un flux brut en
/// octets audio + blocs de métadonnées selon l'intervalle `icy-metaint`.
pub struct IcyParser {
    metaint: Option<usize>,
    /// octets audio restants avant le prochain bloc de métadonnées
    countdown: usize,
    meta: Option<Vec<u8>>,
    meta_need: usize,
}

impl IcyParser {
    pub fn new(metaint: Option<usize>) -> Self {
        let metaint = metaint.filter(|&m| m > 0);
        IcyParser {
            countdown: metaint.unwrap_or(0),
            metaint,
            meta: None,
            meta_need: 0,
        }
    }

    /// Consomme un bloc brut du flux ; ajoute l'audio à `out` et retourne le
    /// `StreamTitle` s'il a été reçu dans ce bloc.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<u8>) -> Option<String> {
        let mut title = None;
        for &b in data {
            match self.metaint {
                None => out.push(b),
                Some(mi) => {
                    if self.meta.is_some() {
                        // dans un bloc de métadonnées
                        let m = self.meta.as_mut().unwrap();
                        m.push(b);
                        self.meta_need -= 1;
                        if self.meta_need == 0 {
                            if let Some(t) = parse_stream_title(m) {
                                title = Some(t);
                            }
                            self.meta = None;
                            self.countdown = mi;
                        }
                    } else if self.countdown == 0 {
                        // octet de longueur (multiple de 16), 0 = pas de meta
                        let len = (b as usize) * 16;
                        if len == 0 {
                            self.countdown = mi;
                        } else {
                            self.meta = Some(Vec::with_capacity(len));
                            self.meta_need = len;
                        }
                    } else {
                        self.countdown -= 1;
                        out.push(b);
                    }
                }
            }
        }
        title
    }
}

/// Tampon partagé entre le thread de réception et le lecteur.
struct Shared {
    /// octets audio disponibles, commençant à l'offset absolu `base`
    data: Vec<u8>,
    base: u64,
    eof: bool,
    err: Option<String>,
}

struct Inner {
    shared: Mutex<Shared>,
    cond: Condvar,
    closed: AtomicBool,
}

/// Tampon max conservé en mémoire ; au-delà on avance `base` (fenêtre glissante).
const CAP: usize = 48 * 1024 * 1024;
const KEEP: usize = 32 * 1024 * 1024;

/// Flux live seekable dans la fenêtre retenue (Read + Seek + Send + Sync).
pub struct LiveStream {
    inner: std::sync::Arc<Inner>,
    url: String,
    pos: u64,
}

impl LiveStream {
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Read for LiveStream {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut sh = self.inner.shared.lock().unwrap();
        loop {
            let avail_start = sh.base;
            let avail_end = sh.base + sh.data.len() as u64;
            if self.pos < avail_start {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "seek arrière hors de la fenêtre retenue (flux live)",
                ));
            }
            if self.pos < avail_end {
                let off = (self.pos - sh.base) as usize;
                let n = (avail_end - self.pos).min(out.len() as u64) as usize;
                out[..n].copy_from_slice(&sh.data[off..off + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if sh.eof {
                if let Some(e) = &sh.err {
                    return Err(std::io::Error::other(format!(
                        "flux live {}: {e}",
                        self.url
                    )));
                }
                return Ok(0);
            }
            sh = self.inner.cond.wait(sh).unwrap();
        }
    }
}

impl Seek for LiveStream {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let sh = self.inner.shared.lock().unwrap();
        let end = sh.base + sh.data.len() as u64;
        let target = match pos {
            SeekFrom::Start(p) => p as i128,
            // un flux live n'a pas de fin connue : « End » = position courante
            // tamponnée (le décodeur sonde alors un flux de taille indéterminée)
            SeekFrom::End(o) => end as i128 + o as i128,
            SeekFrom::Current(o) => self.pos as i128 + o as i128,
        };
        drop(sh);
        if target < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek négatif",
            ));
        }
        let target = target as u64;
        let sh = self.inner.shared.lock().unwrap();
        if target < sh.base {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek arrière hors de la fenêtre retenue (flux live)",
            ));
        }
        self.pos = target;
        Ok(target)
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.cond.notify_all();
        set_radio_title(&self.url, None);
    }
}

/// Ouvre un flux live HTTP(S) (webradio) avec négociation ICY.
///
/// Client **bloquant** dédié : appelable depuis n'importe quel thread, y compris
/// depuis l'intérieur d'un runtime tokio (worker du moteur) sans `block_on`
/// imbriqué. La requête initiale est faite ici (erreurs immédiates : DNS,
/// HTTP >= 400) ; la lecture du corps est transférée à un thread dédié
/// (pas de timeout global de requête, un flux live ne se termine pas).
pub fn open_live(url: &str) -> Result<LiveStream> {
    if !is_http_uri(url) {
        return Err(AurisError::InvalidUri(url.to_string()));
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("auris/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AurisError::Network(format!("client live: {e}")))?;
    let resp = client
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .map_err(|e| AurisError::Network(format!("GET {url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AurisError::Network(format!("GET {url}: HTTP {status}")));
    }
    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let inner = std::sync::Arc::new(Inner {
        shared: Mutex::new(Shared {
            data: Vec::new(),
            base: 0,
            eof: false,
            err: None,
        }),
        cond: Condvar::new(),
        closed: AtomicBool::new(false),
    });
    let inner2 = inner.clone();
    let url2 = url.to_string();
    std::thread::Builder::new()
        .name(format!("auris-live-{}", &url2[..url2.len().min(48)]))
        .spawn(move || {
            let mut parser = IcyParser::new(metaint);
            let mut resp = resp;
            let mut raw = [0u8; 16 * 1024];
            loop {
                if inner2.closed.load(Ordering::SeqCst) {
                    break;
                }
                match resp.read(&mut raw) {
                    Ok(0) => {
                        let mut sh = inner2.shared.lock().unwrap();
                        sh.eof = true;
                        drop(sh);
                        inner2.cond.notify_all();
                        break;
                    }
                    Ok(n) => {
                        let mut audio = Vec::with_capacity(n);
                        let title = parser.push(&raw[..n], &mut audio);
                        {
                            let mut sh = inner2.shared.lock().unwrap();
                            sh.data.extend_from_slice(&audio);
                            if let Some(t) = &title {
                                set_radio_title(&url2, Some(t.clone()));
                            }
                            if sh.data.len() > CAP {
                                let drop_n = sh.data.len() - KEEP;
                                sh.data.drain(..drop_n);
                                sh.base += drop_n as u64;
                            }
                        }
                        inner2.cond.notify_all();
                    }
                    Err(e) => {
                        let mut sh = inner2.shared.lock().unwrap();
                        sh.err = Some(e.to_string());
                        sh.eof = true;
                        drop(sh);
                        inner2.cond.notify_all();
                        break;
                    }
                }
            }
            set_radio_title(&url2, None);
        })
        .map_err(|e| AurisError::other(format!("spawn thread live: {e}")))?;
    Ok(LiveStream {
        inner,
        url: url.to_string(),
        pos: 0,
    })
}

/// Variante de [`open_live`] qui s'exécute dans un thread dédié : le client
/// bloquant reqwest embarque un runtime tokio, qu'il est interdit de créer ou
/// détruire dans un contexte async (panic « Cannot drop a runtime... ») — or
/// `SourcesManager::open_uri` tourne justement dans le runtime du worker.
pub fn open_live_offthread(url: &str) -> Result<LiveStream> {
    let url = url.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("auris-live-open".into())
        .spawn(move || {
            let _ = tx.send(open_live(&url));
        })
        .map_err(|e| AurisError::other(format!("spawn open live: {e}")))?;
    rx.recv()
        .map_err(|_| AurisError::Network("thread open live mort".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(audio: &[u8], meta: &[u8], metaint: usize) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(audio);
        let mut m = meta.to_vec();
        while !m.len().is_multiple_of(16) {
            m.push(0);
        }
        v.push((m.len() / 16) as u8);
        v.extend_from_slice(&m);
        let _ = metaint;
        v
    }

    #[test]
    fn icy_sans_metaint_passe_tout() {
        let mut p = IcyParser::new(None);
        let mut out = Vec::new();
        let t = p.push(b"abcdef", &mut out);
        assert_eq!(out, b"abcdef");
        assert!(t.is_none());
    }

    #[test]
    fn icy_extrait_stream_title() {
        let metaint = 8;
        let mut p = IcyParser::new(Some(metaint));
        let meta = b"StreamTitle='Artiste - Titre';StreamUrl='';";
        let flux = block(b"12345678", meta, metaint);
        let mut out = Vec::new();
        let t = p.push(&flux, &mut out);
        assert_eq!(out, b"12345678");
        assert_eq!(t.as_deref(), Some("Artiste - Titre"));
    }

    #[test]
    fn icy_bloc_meta_nul_passe() {
        let metaint = 4;
        let mut p = IcyParser::new(Some(metaint));
        let mut flux = b"abcd".to_vec();
        flux.push(0); // bloc meta vide
        flux.extend_from_slice(b"efgh");
        let mut out = Vec::new();
        let t = p.push(&flux, &mut out);
        assert_eq!(out, b"abcdefgh");
        assert!(t.is_none());
    }

    #[test]
    fn icy_deux_blocs_consecutifs() {
        let metaint = 4;
        let mut p = IcyParser::new(Some(metaint));
        let mut flux = block(b"aaaa", b"StreamTitle='Un';", metaint);
        flux.extend_from_slice(&block(b"bbbb", b"StreamTitle='Deux';", metaint));
        let mut out = Vec::new();
        let t1 = p.push(&flux[..flux.len()], &mut out);
        assert_eq!(out, b"aaaabbbb");
        // le parser a traité les deux blocs dans le même push : dernier titre
        assert_eq!(t1.as_deref(), Some("Deux"));
    }

    #[test]
    fn parse_stream_title_variantes() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Hello World';").as_deref(),
            Some("Hello World")
        );
        assert_eq!(parse_stream_title(b"StreamTitle='';"), None);
        assert_eq!(parse_stream_title(b"rien ici"), None);
        assert_eq!(
            parse_stream_title(b"StreamTitle='A';StreamUrl='http://x';").as_deref(),
            Some("A")
        );
    }

    #[test]
    fn registre_titre() {
        set_radio_title("http://x", Some("T".into()));
        assert_eq!(radio_title("http://x").as_deref(), Some("T"));
        set_radio_title("http://x", None);
        assert_eq!(radio_title("http://x"), None);
    }

    #[test]
    fn open_live_refuse_non_http() {
        assert!(open_live("src://nas/x.mp3").is_err());
        assert!(open_live("ftp://x").is_err());
    }
}
