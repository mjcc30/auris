//! Bibliotheque musicale: index persistant (sled), scan des sources,
//! agregation albums/artistes, recherche plein texte simple, pochettes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use auris_core::{
    Album, Artist, AurisError, Playlist, Result, ShareKind, ShareLink, Source, Track,
};
use auris_player::probe_meta;
use auris_sources::{make_uri, SourcesManager};

/// Etat d'un scan en cours (expose a l'UI).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScanStatus {
    pub running: bool,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub scanned: usize,
    pub added: usize,
    pub errors: usize,
    pub last_error: Option<String>,
    pub finished_at: Option<i64>,
}

const TREE_TRACKS: &str = "tracks";
const TREE_BY_URI: &str = "by_uri";
const TREE_COVERS: &str = "covers";
const TREE_ALBUMS: &str = "albums";
const TREE_ALBUM_BY_KEY: &str = "album_by_key";
const TREE_ARTISTS: &str = "artists";
const TREE_ARTIST_BY_KEY: &str = "artist_by_key";

const TREE_PLAYLISTS: &str = "playlists";
const TREE_SHARES: &str = "shares";
const TREE_BIOS: &str = "bios";

pub struct Library {
    db: sled::Db,
    sources: Arc<SourcesManager>,
    scan_status: Arc<Mutex<ScanStatus>>,
}

fn enc<T: serde::Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| AurisError::Storage(e.to_string()))
}
fn dec<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| AurisError::Storage(e.to_string()))
}
/// Conversion des erreurs sled vers AurisError.
fn se(e: sled::Error) -> AurisError {
    AurisError::Storage(e.to_string())
}

impl Library {
    /// Ouvre (ou cree) la bibliotheque dans `data_dir/library`.
    pub fn open(data_dir: &Path, sources: Arc<SourcesManager>) -> Result<Self> {
        let path = data_dir.join("library");
        let db = sled::open(path).map_err(|e| AurisError::Storage(e.to_string()))?;
        for t in [
            TREE_TRACKS,
            TREE_BY_URI,
            TREE_COVERS,
            TREE_ALBUMS,
            TREE_ALBUM_BY_KEY,
            TREE_ARTISTS,
            TREE_ARTIST_BY_KEY,
            TREE_PLAYLISTS,
            TREE_SHARES,
            TREE_BIOS,
        ] {
            db.open_tree(t)
                .map_err(|e| AurisError::Storage(e.to_string()))?;
        }
        Ok(Library {
            db,
            sources,
            scan_status: Arc::new(Mutex::new(ScanStatus::default())),
        })
    }

    pub fn sources(&self) -> Arc<SourcesManager> {
        self.sources.clone()
    }

    fn tree(&self, name: &str) -> sled::Tree {
        self.db.open_tree(name).expect("tree sled ouverte a l'init")
    }

    pub fn scan_status(&self) -> ScanStatus {
        self.scan_status.lock().unwrap().clone()
    }

    /// Lance un scan en arriere-plan pour une source. Retourne immediatement.
    pub fn scan_source(&self, source_id: &str) -> Result<()> {
        {
            let mut st = self.scan_status.lock().unwrap();
            if st.running {
                return Err(AurisError::Other("un scan est deja en cours".into()));
            }
            st.running = true;
            st.source_id = Some(source_id.to_string());
            st.source_name = self.sources.get(source_id).map(|s| s.name);
            st.scanned = 0;
            st.added = 0;
            st.errors = 0;
            st.last_error = None;
            st.finished_at = None;
        }
        let db = self.db.clone();
        let sources = self.sources.clone();
        let status = self.scan_status.clone();
        let sid = source_id.to_string();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    status.lock().unwrap().last_error = Some(e.to_string());
                    status.lock().unwrap().running = false;
                    status.lock().unwrap().finished_at = Some(chrono::Utc::now().timestamp());
                    return;
                }
            };
            rt.block_on(scan_source_blocking(db, sources, status, sid));
        });
        Ok(())
    }

    /// Lance un scan de toutes les sources actives, enchainees une par une.
    pub fn scan_all(&self) -> Result<()> {
        {
            let mut st = self.scan_status.lock().unwrap();
            if st.running {
                return Err(AurisError::Other("un scan est deja en cours".into()));
            }
            st.running = true;
            st.source_id = None;
            st.source_name = Some("Toutes les sources".into());
            st.scanned = 0;
            st.added = 0;
            st.errors = 0;
            st.last_error = None;
            st.finished_at = None;
        }
        let sources: Vec<Source> = self
            .sources
            .list()
            .into_iter()
            .filter(|s| s.enabled)
            .collect();
        let db = self.db.clone();
        let mgr = self.sources.clone();
        let status = self.scan_status.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let mut st = status.lock().unwrap();
                    st.last_error = Some(e.to_string());
                    st.running = false;
                    st.finished_at = Some(chrono::Utc::now().timestamp());
                    return;
                }
            };
            for s in sources {
                {
                    let mut st = status.lock().unwrap();
                    st.running = true;
                    st.source_id = Some(s.id.clone());
                    st.source_name = Some(s.name.clone());
                }
                rt.block_on(scan_source_blocking(
                    db.clone(),
                    mgr.clone(),
                    status.clone(),
                    s.id.clone(),
                ));
            }
            let mut st = status.lock().unwrap();
            st.running = false;
            st.finished_at = Some(chrono::Utc::now().timestamp());
        });
        Ok(())
    }

    pub fn track(&self, id: &str) -> Option<Track> {
        let t = self.tree(TREE_TRACKS);
        t.get(id.as_bytes())
            .ok()?
            .and_then(|v| dec::<Track>(&v).ok())
    }

    // ---------- lectures ----------

    pub fn track_count(&self) -> usize {
        self.tree(TREE_TRACKS).len()
    }

    pub fn tracks(&self) -> Vec<Track> {
        let tree = self.tree(TREE_TRACKS);
        let mut out = Vec::new();
        for (_, v) in tree.iter().flatten() {
            if let Ok(t) = dec::<Track>(&v) {
                out.push(t);
            }
        }
        out.sort_by(|a, b| {
            a.album_key
                .cmp(&b.album_key)
                .then(a.disc_number.unwrap_or(0).cmp(&b.disc_number.unwrap_or(0)))
                .then(
                    a.track_number
                        .unwrap_or(0)
                        .cmp(&b.track_number.unwrap_or(0)),
                )
        });
        out
    }

    pub fn get_by_uri(&self, uri: &str) -> Option<Track> {
        let tree = self.tree(TREE_BY_URI);
        let id = tree.get(uri.as_bytes()).ok()??;
        let id: String = dec(&id).ok()?;
        let t = self.tree(TREE_TRACKS);
        t.get(id.as_bytes())
            .ok()?
            .and_then(|v| dec::<Track>(&v).ok())
    }

    /// Insere ou remplace une piste dans l'index (idempotent par id et par URI).
    /// Utilise pour les pistes jouees hors scan (dossier parcouru directement).
    pub fn upsert_track(&self, t: &Track) -> Result<()> {
        self.tree(TREE_TRACKS)
            .insert(t.id.as_bytes(), enc(t)?)
            .map_err(se)?;
        self.tree(TREE_BY_URI)
            .insert(t.uri.as_bytes(), enc(&t.id)?)
            .map_err(se)?;
        Ok(())
    }

    pub fn search(&self, q: &str) -> Vec<Track> {
        let q = q.to_lowercase();
        self.tracks()
            .into_iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&q)
                    || t.artist.to_lowercase().contains(&q)
                    || t.album.to_lowercase().contains(&q)
                    || t.genre
                        .as_deref()
                        .map(|g| g.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .collect()
    }

    pub fn albums(&self) -> Vec<Album> {
        let tree = self.tree(TREE_ALBUMS);
        let mut out: Vec<Album> = tree
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| dec::<Album>(&v).ok())
            .collect();
        out.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.title.cmp(&b.title))
        });
        out
    }

    pub fn album(&self, album_id: &str) -> Option<(Album, Vec<Track>)> {
        let albums = self.tree(TREE_ALBUMS);
        let album: Album = dec(&albums.get(album_id.as_bytes()).ok()??).ok()?;
        let tracks = self
            .tracks()
            .into_iter()
            .filter(|t| t.album_key == album_key_of(&album))
            .collect();
        Some((album, tracks))
    }

    pub fn artists(&self) -> Vec<Artist> {
        let tree = self.tree(TREE_ARTISTS);
        let mut out: Vec<Artist> = tree
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| dec::<Artist>(&v).ok())
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn artist(&self, artist_id: &str) -> Option<(Artist, Vec<Album>)> {
        let artists = self.tree(TREE_ARTISTS);
        let artist: Artist = dec(&artists.get(artist_id.as_bytes()).ok()??).ok()?;
        let albums = self
            .albums()
            .into_iter()
            .filter(|a| a.artist.to_lowercase() == artist.name.to_lowercase())
            .collect();
        Some((artist, albums))
    }

    /// Tous les titres d'un artiste, tries par album/disque/piste.
    pub fn artist_tracks(&self, artist_id: &str) -> Option<Vec<Track>> {
        let (artist, _) = self.artist(artist_id)?;
        let mut tracks: Vec<Track> = self
            .tracks()
            .into_iter()
            .filter(|t| t.artist.to_lowercase() == artist.name.to_lowercase())
            .collect();
        tracks.sort_by(|a, b| {
            (
                a.album.to_lowercase(),
                a.disc_number.unwrap_or(1),
                a.track_number.unwrap_or(0),
                a.title.to_lowercase(),
            )
                .cmp(&(
                    b.album.to_lowercase(),
                    b.disc_number.unwrap_or(1),
                    b.track_number.unwrap_or(0),
                    b.title.to_lowercase(),
                ))
        });
        Some(tracks)
    }

    /// Biographie mise en cache pour une cle d'artiste (None = non cachee).
    pub fn bio_get(&self, artist_key: &str) -> Option<String> {
        self.tree(TREE_BIOS)
            .get(artist_key.as_bytes())
            .ok()?
            .and_then(|v| String::from_utf8(v.to_vec()).ok())
    }

    /// Met en cache la biographie d'une cle d'artiste.
    pub fn bio_set(&self, artist_key: &str, text: &str) -> Result<()> {
        self.tree(TREE_BIOS)
            .insert(artist_key.as_bytes(), text.as_bytes())
            .map_err(se)?;
        Ok(())
    }

    pub fn cover(&self, track_id: &str) -> Option<Vec<u8>> {
        self.tree(TREE_COVERS)
            .get(track_id.as_bytes())
            .ok()?
            .map(|v| v.to_vec())
    }

    // ---------- listes de lecture ----------

    pub fn playlists(&self) -> Vec<Playlist> {
        let tree = self.tree(TREE_PLAYLISTS);
        let mut out: Vec<Playlist> = tree
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| dec::<Playlist>(&v).ok())
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        out
    }

    /// Cree une liste de lecture. Les IDs inconnus sont filtres a la lecture.
    pub fn create_playlist(&self, name: &str, track_ids: Vec<String>) -> Result<Playlist> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AurisError::other("nom de playlist vide"));
        }
        let p = Playlist {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            track_ids,
            created_at: chrono::Utc::now().timestamp(),
        };
        self.tree(TREE_PLAYLISTS)
            .insert(p.id.as_bytes(), enc(&p)?)
            .map_err(se)?;
        Ok(p)
    }

    /// Playlist + ses pistes resolues (les IDs inconnus sont ignores).
    pub fn playlist(&self, id: &str) -> Option<(Playlist, Vec<Track>)> {
        let p: Playlist = dec(&self.tree(TREE_PLAYLISTS).get(id.as_bytes()).ok()??).ok()?;
        let tracks = p
            .track_ids
            .iter()
            .filter_map(|tid| self.track(tid))
            .collect();
        Some((p, tracks))
    }

    pub fn delete_playlist(&self, id: &str) -> Result<bool> {
        Ok(self
            .tree(TREE_PLAYLISTS)
            .remove(id.as_bytes())
            .map_err(se)?
            .is_some())
    }

    // ---------- liens de partage ----------

    /// Cree un lien de partage. `expires_in_secs` : `None` = permanent.
    pub fn create_share(
        &self,
        kind: ShareKind,
        id: &str,
        title: &str,
        expires_in_secs: Option<i64>,
    ) -> Result<ShareLink> {
        let now = chrono::Utc::now().timestamp();
        let share = ShareLink {
            token: uuid::Uuid::new_v4().simple().to_string(),
            kind,
            id: id.to_string(),
            title: title.to_string(),
            created_at: now,
            expires_at: expires_in_secs.map(|d| now + d),
        };
        self.tree(TREE_SHARES)
            .insert(share.token.as_bytes(), enc(&share)?)
            .map_err(se)?;
        Ok(share)
    }

    /// Tous les liens de partage, plus recents d'abord.
    pub fn shares(&self) -> Vec<ShareLink> {
        let mut out: Vec<ShareLink> = self
            .tree(TREE_SHARES)
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| dec::<ShareLink>(&v).ok())
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        out
    }

    /// Lien de partage valide (non expire) ; les liens expires sont purges.
    pub fn share(&self, token: &str) -> Option<ShareLink> {
        let tree = self.tree(TREE_SHARES);
        let raw = tree.get(token.as_bytes()).ok()??;
        let share: ShareLink = dec(&raw).ok()?;
        if share.is_expired(chrono::Utc::now().timestamp()) {
            let _ = tree.remove(token.as_bytes());
            return None;
        }
        Some(share)
    }

    /// Revoque un lien de partage. Retourne true s'il existait.
    pub fn delete_share(&self, token: &str) -> Result<bool> {
        Ok(self
            .tree(TREE_SHARES)
            .remove(token.as_bytes())
            .map_err(se)?
            .is_some())
    }
}

fn album_key_of(a: &Album) -> String {
    format!("{}|{}", a.artist.to_lowercase(), a.title.to_lowercase())
}

/// Scan bloquant d'une source: parcours recursif + probe de chaque fichier audio.
async fn scan_source_blocking(
    db: sled::Db,
    sources: Arc<SourcesManager>,
    status: Arc<Mutex<ScanStatus>>,
    source_id: String,
) {
    let mut uris: Vec<String> = Vec::new();
    collect_audio(&sources, &source_id, "", 0, &mut uris).await;
    let total = uris.len();
    for uri in uris {
        {
            let st = status.lock().unwrap();
            if !st.running {
                break; // annule
            }
        }
        let scanned = {
            let mut st = status.lock().unwrap();
            st.scanned += 1;
            st.scanned
        };
        if scanned % 25 == 0 {
            tracing::info!("scan {source_id}: {scanned}/{total}");
        }
        if let Err(e) = probe_and_store(&db, &sources, &uri).await {
            tracing::warn!("probe {uri}: {e}");
            let mut st = status.lock().unwrap();
            st.errors += 1;
            st.last_error = Some(format!("{uri}: {e}"));
        } else {
            status.lock().unwrap().added += 1;
        }
    }
    let mut st = status.lock().unwrap();
    st.running = false;
    st.finished_at = Some(chrono::Utc::now().timestamp());
    tracing::info!(
        "scan {source_id} termine: {} ajoutes, {} erreurs",
        st.added,
        st.errors
    );
}

/// Parcours recursif d'une source, collecte les URI des fichiers audio.
fn collect_audio<'a>(
    sources: &'a Arc<SourcesManager>,
    source_id: &'a str,
    path: &'a str,
    depth: usize,
    out: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth > 12 {
            return;
        }
        let uri = make_uri(source_id, path);
        let entries = match sources.browse(&uri).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("browse {uri}: {e}");
                return;
            }
        };
        for e in entries {
            if e.is_dir {
                let p = if path.is_empty() {
                    e.name.clone()
                } else {
                    format!("{path}/{}", e.name)
                };
                collect_audio(sources, source_id, &p, depth + 1, out).await;
            } else if e.is_audio {
                out.push(make_uri(
                    source_id,
                    format!("{path}/{}", e.name).trim_start_matches('/'),
                ));
            }
        }
    })
}

/// Sonde un fichier et l'enregistre dans l'index.
async fn probe_and_store(db: &sled::Db, sources: &Arc<SourcesManager>, uri: &str) -> Result<()> {
    let reader = sources.open_uri(uri).await?;
    let ext = uri
        .rsplit('.')
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_string();
    let (meta, _spec, dur) = probe_meta(reader, &ext)?;
    let (source_id, rel) =
        auris_sources::parse_src_uri(uri).ok_or_else(|| AurisError::InvalidUri(uri.into()))?;
    let mut t = Track::new(uri.to_string(), source_id);
    if let Some(v) = &meta.title {
        t.title = v.clone();
    }
    if let Some(v) = &meta.artist {
        t.artist = v.clone();
    }
    if let Some(v) = &meta.album {
        t.album = v.clone();
    }
    t.album_artist = meta
        .album_artist
        .clone()
        .or(meta.artist.clone())
        .unwrap_or_else(|| "Artiste inconnu".into());
    t.track_number = meta.track_number;
    t.disc_number = meta.disc_number;
    t.year = meta.year;
    t.genre = meta.genre.clone();
    // Pour le DSD (DSF/DFF), la durée provient du champ samples de l'en-tête,
    // pas des métadonnées taguées.
    t.duration_secs = meta.duration_secs.or(dur);
    t.format = meta.format;
    t.has_cover = meta.has_cover;
    t.finalize_keys();

    let tracks = db
        .open_tree(TREE_TRACKS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let by_uri = db
        .open_tree(TREE_BY_URI)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let covers = db
        .open_tree(TREE_COVERS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let albums = db
        .open_tree(TREE_ALBUMS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let album_by_key = db
        .open_tree(TREE_ALBUM_BY_KEY)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let artists = db
        .open_tree(TREE_ARTISTS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let artist_by_key = db
        .open_tree(TREE_ARTIST_BY_KEY)
        .map_err(|e| AurisError::Storage(e.to_string()))?;

    // idempotent: un URI deja indexe reutilise son id (pas de doublon au re-scan)
    let existing: Option<String> = by_uri
        .get(uri.as_bytes())
        .map_err(se)?
        .map(|v| dec(&v))
        .transpose()?;
    let is_new = existing.is_none();
    if let Some(id) = existing {
        t.id = id;
    }

    if is_new {
        // artiste
        let artist_id = match artist_by_key.get(t.artist_key.as_bytes()).map_err(se)? {
            Some(v) => dec::<String>(&v)?,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                artist_by_key
                    .insert(t.artist_key.as_bytes(), enc(&id)?)
                    .map_err(se)?;
                artists
                    .insert(
                        id.as_bytes(),
                        enc(&Artist {
                            id: id.clone(),
                            name: t.album_artist.clone(),
                            album_count: 0,
                        })?,
                    )
                    .map_err(se)?;
                id
            }
        };

        // album
        let album_id = match album_by_key.get(t.album_key.as_bytes()).map_err(se)? {
            Some(v) => dec::<String>(&v)?,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                album_by_key
                    .insert(t.album_key.as_bytes(), enc(&id)?)
                    .map_err(se)?;
                albums
                    .insert(
                        id.as_bytes(),
                        enc(&Album {
                            id: id.clone(),
                            title: t.album.clone(),
                            artist: t.album_artist.clone(),
                            year: t.year,
                            cover_track_id: None,
                            track_count: 0,
                            duration_secs: 0.0,
                        })?,
                    )
                    .map_err(se)?;
                id
            }
        };

        // met a jour les agregats album/artiste
        if let Some(v) = albums.get(album_id.as_bytes()).map_err(se)? {
            let mut a: Album = dec(&v)?;
            a.track_count += 1;
            a.duration_secs += t.duration_secs.unwrap_or(0.0);
            if a.cover_track_id.is_none() && t.has_cover {
                a.cover_track_id = Some(t.id.clone());
            }
            albums.insert(album_id.as_bytes(), enc(&a)?).map_err(se)?;
        }
        if let Some(v) = artists.get(artist_id.as_bytes()).map_err(se)? {
            let mut a: Artist = dec(&v)?;
            // compte d'albums distincts de l'artiste
            let n = album_by_key
                .iter()
                .filter_map(|r| r.ok())
                .filter(|(k, _)| {
                    // cle d'album commencant par "<artist_key>|"
                    k.starts_with(format!("{}|", t.artist_key).as_bytes())
                })
                .count();
            a.album_count = n;
            artists.insert(artist_id.as_bytes(), enc(&a)?).map_err(se)?;
        }
    } // fin if is_new

    if let Some(c) = &meta.cover {
        covers.insert(t.id.as_bytes(), c.clone()).map_err(se)?;
    }
    by_uri.insert(uri.as_bytes(), enc(&t.id)?).map_err(se)?;
    tracks.insert(t.id.as_bytes(), enc(&t)?).map_err(se)?;
    let _ = rel;
    Ok(())
}

/// Supprime toutes les pistes d'une source (rescan propre).
pub fn purge_source(db: &sled::Db, source_id: &str) -> Result<usize> {
    let tracks = db
        .open_tree(TREE_TRACKS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let by_uri = db
        .open_tree(TREE_BY_URI)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let covers = db
        .open_tree(TREE_COVERS)
        .map_err(|e| AurisError::Storage(e.to_string()))?;
    let mut ids: Vec<String> = Vec::new();
    let mut uris: Vec<Vec<u8>> = Vec::new();
    for (_, v) in tracks.iter().flatten() {
        if let Ok(t) = dec::<Track>(&v) {
            if t.source_id == source_id {
                ids.push(t.id.clone());
                uris.push(t.uri.clone().into_bytes());
            }
        }
    }
    let n = ids.len();
    for id in &ids {
        tracks.remove(id.as_bytes()).map_err(se)?;
        covers.remove(id.as_bytes()).map_err(se)?;
    }
    for u in &uris {
        by_uri.remove(u).map_err(se)?;
    }
    Ok(n)
}

/// HashMap utilitaire pour les reponses API.
pub type TrackMap = HashMap<String, Track>;
