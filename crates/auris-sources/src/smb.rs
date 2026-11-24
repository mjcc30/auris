//! Source SMB/CIFS: acces direct a un partage NAS (Synology, QNAP, TrueNAS...).
//!
//! Toute l'I/O passe par un thread worker dedie qui possede le client SMB
//! et un runtime tokio current_thread. Le `SmbFile` expose un Read+Seek
//! synchrone au moteur de lecture via des requetes `read_at` par plages.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;

use async_trait::async_trait;
use auris_core::{is_audio_file, AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::{make_uri, SeekRead, SourceBackend};

const MAX_READ: u32 = 512 * 1024;

enum Job {
    List {
        path: String,
        tx: SyncSender<Result<Vec<(String, u64, bool)>>>,
    },
    Open {
        path: String,
        tx: SyncSender<Result<(u64, u64)>>,
    },
    ReadAt {
        handle: u64,
        offset: u64,
        len: u32,
        tx: SyncSender<Result<Vec<u8>>>,
    },
    Close {
        handle: u64,
    },
}

struct Worker {
    tx: Sender<Job>,
    _handle: JoinHandle<()>,
}

impl Worker {
    fn spawn(
        host: &str,
        share: &str,
        user: &str,
        password: &str,
        base_path: &str,
    ) -> Result<Worker> {
        let addr = if host.contains(':') {
            host.to_string()
        } else {
            format!("{}:445", host)
        };
        let (tx, rx) = channel::<Job>();
        let addr2 = addr.clone();
        let share2 = share.to_string();
        let user2 = user.to_string();
        let pass2 = password.to_string();
        let base2 = base_path.trim_matches('/').to_string();
        let handle = std::thread::Builder::new()
            .name("auris-smb".into())
            .spawn(move || run_worker(&addr2, &share2, &user2, &pass2, &base2, rx))
            .map_err(|e| AurisError::other(format!("spawn worker smb: {e}")))?;
        Ok(Worker {
            tx,
            _handle: handle,
        })
    }

    fn call_list(&self, path: String) -> Result<Vec<(String, u64, bool)>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Job::List { path, tx })
            .map_err(|_| AurisError::Network("worker smb mort".into()))?;
        rx.recv()
            .map_err(|_| AurisError::Network("worker smb mort".into()))?
    }

    fn call_open(&self, path: String) -> Result<(u64, u64)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Job::Open { path, tx })
            .map_err(|_| AurisError::Network("worker smb mort".into()))?;
        rx.recv()
            .map_err(|_| AurisError::Network("worker smb mort".into()))?
    }

    fn call_read(&self, handle: u64, offset: u64, len: u32) -> Result<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Job::ReadAt {
                handle,
                offset,
                len,
                tx,
            })
            .map_err(|_| AurisError::Network("worker smb mort".into()))?;
        rx.recv()
            .map_err(|_| AurisError::Network("worker smb mort".into()))?
    }

    fn call_close(&self, handle: u64) {
        let _ = self.tx.send(Job::Close { handle });
    }
}

fn run_worker(
    addr: &str,
    share: &str,
    user: &str,
    password: &str,
    base_path: &str,
    rx: Receiver<Job>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("smb: runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut client = match smb2::connect(addr, user, password).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("smb: connexion {addr}: {e}");
                // Repond l'erreur a tous les jobs restants.
                while let Ok(job) = rx.recv() {
                    respond_err(job, format!("connexion SMB echouee: {e}"));
                }
                return;
            }
        };
        let mut tree = match client.connect_share(share).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("smb: partage {share}: {e}");
                while let Ok(job) = rx.recv() {
                    respond_err(job, format!("partage SMB {share} inaccessible: {e}"));
                }
                return;
            }
        };
        let mut readers: HashMap<u64, smb2::client::stream::FileReader> = HashMap::new();
        let mut next_id: u64 = 1;
        while let Ok(job) = rx.recv() {
            match job {
                Job::List { path, tx } => {
                    let full = join_path(base_path, &path);
                    let res = client
                        .list_directory(&mut tree, &full)
                        .await
                        .map(|entries| {
                            entries
                                .into_iter()
                                .map(|e| (e.name, e.size, e.is_directory))
                                .collect::<Vec<_>>()
                        });
                    let _ =
                        tx.send(res.map_err(|e| AurisError::Network(format!("list {full}: {e}"))));
                }
                Job::Open { path, tx } => {
                    let full = join_path(base_path, &path);
                    let res = async {
                        let conn = client.connection_for(&tree)?;
                        let fr = smb2::client::stream::open_file_reader(
                            Arc::new(tree.clone()),
                            conn,
                            &full,
                        )
                        .await?;
                        let size = fr.size();
                        let id = next_id;
                        next_id += 1;
                        readers.insert(id, fr);
                        Ok((id, size))
                    }
                    .await;
                    let _ = tx.send(res.map_err(|e: smb2::Error| {
                        AurisError::Network(format!("open {full}: {e}"))
                    }));
                }
                Job::ReadAt {
                    handle,
                    offset,
                    len,
                    tx,
                } => {
                    let res = match readers.get(&handle) {
                        Some(fr) => fr
                            .read_at(offset, len as u64)
                            .await
                            .map_err(|e| AurisError::Network(format!("read_at: {e}"))),
                        None => Err(AurisError::Network("handle smb inconnu".into())),
                    };
                    let _ = tx.send(res);
                }
                Job::Close { handle } => {
                    readers.remove(&handle);
                }
            }
        }
    });
}

fn respond_err(job: Job, msg: String) {
    match job {
        Job::List { tx, .. } => {
            let _ = tx.send(Err(AurisError::Network(msg)));
        }
        Job::Open { tx, .. } => {
            let _ = tx.send(Err(AurisError::Network(msg)));
        }
        Job::ReadAt { tx, .. } => {
            let _ = tx.send(Err(AurisError::Network(msg)));
        }
        Job::Close { .. } => {}
    }
}

fn join_path(base: &str, rel: &str) -> String {
    let rel = rel.trim_matches('/');
    if base.is_empty() {
        rel.to_string()
    } else if rel.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, rel)
    }
}

/// Fichier SMB seekable (Read + Seek synchrone, bridge vers le worker async).
pub struct SmbFile {
    worker: Arc<Worker>,
    handle: u64,
    size: u64,
    pos: u64,
}

impl Read for SmbFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let len = std::cmp::min(buf.len() as u64, self.size - self.pos).min(MAX_READ as u64) as u32;
        let data = self
            .worker
            .call_read(self.handle, self.pos, len)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let n = data.len();
        buf[..n].copy_from_slice(&data);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SmbFile {
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

impl Drop for SmbFile {
    fn drop(&mut self) {
        self.worker.call_close(self.handle);
    }
}

pub struct SmbSource {
    source: Source,
    base_path: String,
    worker: Arc<Worker>,
}

impl SmbSource {
    pub fn new(source: Source) -> Result<Self> {
        let (host, share, user, password, base_path) = match &source.kind {
            SourceKind::Smb {
                host,
                share,
                user,
                password,
                base_path,
            } => (
                host.clone(),
                share.clone(),
                user.clone(),
                password.clone(),
                base_path.clone(),
            ),
            _ => return Err(AurisError::other("source non smb")),
        };
        let worker = Arc::new(Worker::spawn(&host, &share, &user, &password, &base_path)?);
        Ok(SmbSource {
            source,
            base_path,
            worker,
        })
    }
}

#[async_trait]
impl SourceBackend for SmbSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let entries = self.worker.call_list(rel_path.to_string())?;
        let base = join_path(&self.base_path, rel_path);
        let mut out = Vec::new();
        for (name, size, is_dir) in entries {
            if name.starts_with('.') {
                continue;
            }
            let rel = if base.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", base, name)
            };
            out.push(BrowseEntry {
                name,
                uri: make_uri(&self.source.id, &rel),
                is_dir,
                size: if is_dir { None } else { Some(size) },
                format: None,
                is_audio: !is_dir && is_audio_file(&rel),
            });
        }
        out.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
        Ok(out)
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let (handle, size) = self.worker.call_open(rel_path.to_string())?;
        Ok(Box::new(SmbFile {
            worker: self.worker.clone(),
            handle,
            size,
            pos: 0,
        }))
    }

    async fn size(&self, rel_path: &str) -> Result<Option<u64>> {
        // via une ouverture legere: on liste le parent et on cherche le nom
        let (dir, name) = match rel_path.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), rel_path.to_string()),
        };
        let entries = self.worker.call_list(dir)?;
        Ok(entries
            .into_iter()
            .find(|(n, _, is_dir)| *n == name && !is_dir)
            .map(|(_, s, _)| s))
    }
}
