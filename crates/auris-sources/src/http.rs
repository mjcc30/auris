//! Source HTTP(S): URLs directes (webradios, serveurs DLNA) et playlists
//! m3u/m3u8. Lecture par plages (Range) avec read-ahead, via un worker async.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source};

use crate::{SeekRead, SourceBackend};

const CHUNK: u64 = 1024 * 1024; // read-ahead 1 Mo

enum Job {
    Open {
        url: String,
        tx: SyncSender<Result<(u64, u64)>>,
    },
    ReadAt {
        handle: u64,
        offset: u64,
        len: u64,
        tx: SyncSender<Result<Vec<u8>>>,
    },
    Close {
        handle: u64,
    },
}

pub struct Worker {
    tx: Sender<Job>,
    _handle: JoinHandle<()>,
}

impl Worker {
    fn spawn() -> Result<Worker> {
        let (tx, rx) = channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("auris-http".into())
            .spawn(move || run_worker(rx))
            .map_err(|e| AurisError::other(format!("spawn worker http: {e}")))?;
        Ok(Worker {
            tx,
            _handle: handle,
        })
    }

    fn open(&self, url: &str) -> Result<(u64, u64)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Job::Open {
                url: url.to_string(),
                tx,
            })
            .map_err(|_| AurisError::Network("worker http mort".into()))?;
        rx.recv()
            .map_err(|_| AurisError::Network("worker http mort".into()))?
    }

    fn read_at(&self, handle: u64, offset: u64, len: u64) -> Result<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Job::ReadAt {
                handle,
                offset,
                len,
                tx,
            })
            .map_err(|_| AurisError::Network("worker http mort".into()))?;
        rx.recv()
            .map_err(|_| AurisError::Network("worker http mort".into()))?
    }

    fn close(&self, handle: u64) {
        let _ = self.tx.send(Job::Close { handle });
    }
}

fn run_worker(rx: Receiver<Job>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("http: runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let mut urls: HashMap<u64, String> = HashMap::new();
        let mut next_id: u64 = 1;
        while let Ok(job) = rx.recv() {
            match job {
                Job::Open { url, tx } => {
                    let client = client.clone();
                    let res = async {
                        let resp = client
                            .get(&url)
                            .header(reqwest::header::RANGE, "bytes=0-0")
                            .send()
                            .await
                            .map_err(|e| AurisError::Network(format!("GET {url}: {e}")))?;
                        let status = resp.status();
                        if status != reqwest::StatusCode::PARTIAL_CONTENT
                            && status != reqwest::StatusCode::OK
                        {
                            return Err(AurisError::Network(format!("GET {url}: HTTP {status}")));
                        }
                        let headers = resp.headers();
                        let range_total = headers
                            .get(reqwest::header::CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.rsplit('/').next()?.parse::<u64>().ok());
                        let content_length = headers
                            .get(reqwest::header::CONTENT_LENGTH)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok());
                        // Flux live (webradio) : en-têtes ICY, ou sentinelle de
                        // taille inconnue dans Content-Range (2^30-1, convention
                        // icecast), ou aucune longueur connue. Dans ce cas on
                        // rapporte size = 0 pour que `open_uri` bascule sur le
                        // lecteur de flux live (`crate::live`).
                        let has_icy = headers.keys().any(|k| k.as_str().starts_with("icy-"));
                        let live = has_icy
                            || range_total == Some(1_073_741_823)
                            || (range_total.is_none() && content_length.is_none());
                        let size = if live {
                            0
                        } else {
                            range_total.or(content_length).unwrap_or(0)
                        };
                        let id = next_id;
                        next_id += 1;
                        urls.insert(id, url.clone());
                        Ok((id, size))
                    }
                    .await;
                    let _ = tx.send(res);
                }
                Job::ReadAt {
                    handle,
                    offset,
                    len,
                    tx,
                } => {
                    let res = match urls.get(&handle) {
                        Some(url) => {
                            let url = url.clone();
                            let client = client.clone();
                            async move {
                                let resp = client
                                    .get(&url)
                                    .header(
                                        reqwest::header::RANGE,
                                        format!("bytes={}-{}", offset, offset + len - 1),
                                    )
                                    .send()
                                    .await
                                    .map_err(|e| AurisError::Network(format!("GET {url}: {e}")))?;
                                let status = resp.status();
                                if status != reqwest::StatusCode::PARTIAL_CONTENT
                                    && status != reqwest::StatusCode::OK
                                {
                                    return Err(AurisError::Network(format!(
                                        "GET {url}: HTTP {status}"
                                    )));
                                }
                                let bytes = resp
                                    .bytes()
                                    .await
                                    .map_err(|e| AurisError::Network(format!("body {url}: {e}")))?;
                                Ok(bytes.to_vec())
                            }
                            .await
                        }
                        None => Err(AurisError::Network("handle http inconnu".into())),
                    };
                    let _ = tx.send(res);
                }
                Job::Close { handle } => {
                    urls.remove(&handle);
                }
            }
        }
    });
}

/// Fichier HTTP seekable avec read-ahead de 1 Mo.
pub struct HttpFile {
    worker: Arc<Worker>,
    handle: u64,
    size: u64,
    pos: u64,
    buf: Vec<u8>,
    buf_start: u64,
}

impl HttpFile {
    /// Taille connue du fichier (0 = inconnu, ex. flux live).
    pub fn size(&self) -> u64 {
        self.size
    }

    fn fill(&mut self) -> std::io::Result<()> {
        if self.pos >= self.size {
            self.buf.clear();
            self.buf_start = self.pos;
            return Ok(());
        }
        let start = self.pos;
        if self.buf_start == start && !self.buf.is_empty() {
            return Ok(());
        }
        let len = std::cmp::min(CHUNK, self.size.saturating_sub(start));
        let data = self
            .worker
            .read_at(self.handle, start, len)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.buf = data;
        self.buf_start = start;
        Ok(())
    }
}

impl Read for HttpFile {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() || self.pos >= self.size {
            return Ok(0);
        }
        self.fill()?;
        let off = (self.pos - self.buf_start) as usize;
        let avail = self.buf.len().saturating_sub(off);
        if avail == 0 {
            return Ok(0);
        }
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.buf[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for HttpFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(o) => self.size as i128 + o as i128,
            SeekFrom::Current(o) => self.pos as i128 + o as i128,
        };
        self.pos = new.clamp(0, self.size as i128) as u64;
        Ok(self.pos)
    }
}

impl Drop for HttpFile {
    fn drop(&mut self) {
        self.worker.close(self.handle);
    }
}

pub struct HttpSource {
    source: Source,
    worker: Arc<Worker>,
}

impl HttpSource {
    pub fn new(source: Source) -> Result<Self> {
        Ok(HttpSource {
            source,
            worker: Arc::new(Worker::spawn()?),
        })
    }

    pub fn open_url(&self, url: &str) -> Result<HttpFile> {
        let (handle, size) = self.worker.open(url)?;
        Ok(HttpFile {
            worker: self.worker.clone(),
            handle,
            size,
            pos: 0,
            buf: Vec::new(),
            buf_start: u64::MAX,
        })
    }

    /// Liste une URL: playlist m3u/m3u8 -> entrees; sinon une entree unique.
    pub async fn list_url(&self, url: &str) -> Result<Vec<BrowseEntry>> {
        if !is_playlist_url(url) {
            return Ok(vec![BrowseEntry {
                name: url
                    .rsplit('/')
                    .next()
                    .unwrap_or(url)
                    .split('?')
                    .next()
                    .unwrap_or(url)
                    .to_string(),
                uri: url.to_string(),
                is_dir: false,
                size: None,
                format: None,
                is_audio: true,
            }]);
        }
        let body = fetch_text(url).await?;
        let base_dir = url.rsplit_once('/').map(|(d, _)| d).unwrap_or(url);
        let mut out = Vec::new();
        let mut extinf: Option<String> = None;
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("#EXTINF:") {
                extinf = rest.split(',').nth(1).map(|s| s.trim().to_string());
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let abs = if line.starts_with("http://") || line.starts_with("https://") {
                line.to_string()
            } else {
                format!("{}/{}", base_dir, line.trim_start_matches('/'))
            };
            let name = extinf.take().unwrap_or_else(|| {
                abs.rsplit('/')
                    .next()
                    .unwrap_or(&abs)
                    .split('?')
                    .next()
                    .unwrap_or(&abs)
                    .to_string()
            });
            out.push(BrowseEntry {
                name,
                uri: abs,
                is_dir: false,
                size: None,
                format: None,
                is_audio: true,
            });
        }
        Ok(out)
    }
}

async fn fetch_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| AurisError::Network(e.to_string()))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AurisError::Network(format!("GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AurisError::Network(format!(
            "GET {url}: HTTP {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AurisError::Network(e.to_string()))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[async_trait]
impl SourceBackend for HttpSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        self.list_url(rel_path).await
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        Ok(Box::new(self.open_url(rel_path)?))
    }
}

pub fn is_playlist_url(url: &str) -> bool {
    url.to_lowercase().contains(".m3u")
}
