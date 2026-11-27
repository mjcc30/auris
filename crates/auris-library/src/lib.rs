//! Bibliotheque musicale: index persistant (sled), scan des sources,
//! agregation albums/artistes, recherche plein texte simple, pochettes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use auris_core::{
    Album, Artist, AurisError, DecadeFacet, FavKind, Favorites, GenreFacet, HistoryEntry,
    LibraryStats, PinKind, Pins, Playlist, Result, SearchGroup, SearchResults, ShareKind,
    ShareLink, Source, Track,
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
const TREE_FAVS: &str = "favs";
const FAVS_KEY: &str = "all";
const TREE_HISTORY: &str = "history";
const HISTORY_KEY: &str = "all";
/// Historique d'écoute : nombre max d'entrées conservées (#69).
const HISTORY_CAP: usize = 500;
const TREE_PINS: &str = "pins";
const PINS_KEY: &str = "all";

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
            TREE_FAVS,
            TREE_HISTORY,
            TREE_PINS,
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

    /// Recherche groupée par type, façon Nova (#71) : pistes, albums, artistes, playlists.
    /// `source_id` restreint la recherche à une source (albums/artistes/playlists filtrés
    /// en conséquence). Chaque groupe est plafonné à `SEARCH_CAP` items, `total` garde le
    /// vrai compte. Les pistes sont triées par pertinence (préfixe de titre d'abord).
    pub fn search_grouped(&self, q: &str, source_id: Option<&str>) -> SearchResults {
        let q = q.trim().to_lowercase();
        let src = source_id.map(str::trim).filter(|s| !s.is_empty());
        let all = self.tracks();
        let hit = |t: &Track| {
            (src.map(|s| t.source_id == s).unwrap_or(true))
                && (t.title.to_lowercase().contains(&q)
                    || t.artist.to_lowercase().contains(&q)
                    || t.album_artist.to_lowercase().contains(&q)
                    || t.album.to_lowercase().contains(&q)
                    || t.genre
                        .as_deref()
                        .map(|g| g.to_lowercase().contains(&q))
                        .unwrap_or(false))
        };
        let mut tracks: Vec<Track> = all.iter().filter(|t| hit(t)).cloned().collect();
        let rank = |t: &Track| {
            if t.title.to_lowercase().starts_with(&q) {
                0
            } else if t.artist.to_lowercase().starts_with(&q)
                || t.album_artist.to_lowercase().starts_with(&q)
            {
                1
            } else if t.album.to_lowercase().starts_with(&q) {
                2
            } else {
                3
            }
        };
        tracks.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });

        // Clés des albums/artistes présents dans la source filtrée (toutes les pistes).
        let src_album_keys: std::collections::HashSet<&str> = all
            .iter()
            .filter(|t| src.map(|s| t.source_id == s).unwrap_or(true))
            .map(|t| t.album_key.as_str())
            .collect();
        let src_artist_keys: std::collections::HashSet<&str> = all
            .iter()
            .filter(|t| src.map(|s| t.source_id == s).unwrap_or(true))
            .map(|t| t.artist_key.as_str())
            .collect();
        let albums: Vec<Album> = self
            .albums()
            .into_iter()
            .filter(|a| {
                (a.title.to_lowercase().contains(&q) || a.artist.to_lowercase().contains(&q))
                    && (src.is_none() || src_album_keys.contains(album_key_of(a).as_str()))
            })
            .collect();
        let artists: Vec<Artist> = self
            .artists()
            .into_iter()
            .filter(|a| {
                a.name.to_lowercase().contains(&q)
                    && (src.is_none() || src_artist_keys.contains(a.name.to_lowercase().as_str()))
            })
            .collect();
        let track_src: HashMap<&str, &str> = all
            .iter()
            .map(|t| (t.id.as_str(), t.source_id.as_str()))
            .collect();
        let playlists: Vec<Playlist> = self
            .playlists()
            .into_iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&q)
                    && match src {
                        None => true,
                        Some(s) => p
                            .track_ids
                            .iter()
                            .any(|id| track_src.get(id.as_str()) == Some(&s)),
                    }
            })
            .collect();

        SearchResults {
            tracks: cap_group(tracks),
            albums: cap_group(albums),
            artists: cap_group(artists),
            playlists: cap_group(playlists),
        }
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

    /// Stats globales calculées depuis les pistes (clés dédupliquées) — #74.
    pub fn stats(&self) -> LibraryStats {
        let tracks = self.tracks();
        let artists = tracks
            .iter()
            .map(|t| t.artist_key.to_lowercase())
            .collect::<std::collections::HashSet<_>>()
            .len();
        let albums = tracks
            .iter()
            .map(|t| t.album_key.to_lowercase())
            .collect::<std::collections::HashSet<_>>()
            .len();
        LibraryStats {
            artists,
            albums,
            tracks: tracks.len(),
            playtime_secs: tracks.iter().map(|t| t.duration_secs.unwrap_or(0.0)).sum(),
        }
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

    /// Facettes « genre » agrégées depuis les tags des pistes (triées par nombre de pistes).
    pub fn genres(&self) -> Vec<GenreFacet> {
        let mut by_key: HashMap<String, GenreFacet> = HashMap::new();
        let mut album_keys: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for t in self.tracks() {
            for g in genre_tags(&t) {
                let key = g.to_lowercase();
                let e = by_key.entry(key.clone()).or_insert_with(|| GenreFacet {
                    name: g,
                    track_count: 0,
                    album_count: 0,
                    cover_track_id: None,
                });
                e.track_count += 1;
                album_keys
                    .entry(key)
                    .or_default()
                    .insert(t.album_key.clone());
                if e.cover_track_id.is_none() && t.has_cover {
                    e.cover_track_id = Some(t.id.clone());
                }
            }
        }
        let mut out: Vec<GenreFacet> = by_key
            .into_iter()
            .map(|(k, mut f)| {
                f.album_count = album_keys.remove(&k).map(|s| s.len()).unwrap_or(0);
                f
            })
            .collect();
        out.sort_by(|a, b| {
            b.track_count
                .cmp(&a.track_count)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    /// Facettes « décennie » agrégées depuis l'année des pistes (triées de la plus récente à la plus ancienne).
    pub fn decades(&self) -> Vec<DecadeFacet> {
        let mut by_decade: HashMap<u32, DecadeFacet> = HashMap::new();
        let mut album_keys: HashMap<u32, std::collections::HashSet<String>> = HashMap::new();
        for t in self.tracks() {
            let Some(year) = t.year else { continue };
            let decade = year / 10 * 10;
            let e = by_decade.entry(decade).or_insert_with(|| DecadeFacet {
                decade,
                track_count: 0,
                album_count: 0,
                cover_track_id: None,
            });
            e.track_count += 1;
            album_keys
                .entry(decade)
                .or_default()
                .insert(t.album_key.clone());
            if e.cover_track_id.is_none() && t.has_cover {
                e.cover_track_id = Some(t.id.clone());
            }
        }
        let mut out: Vec<DecadeFacet> = by_decade
            .into_iter()
            .map(|(d, mut f)| {
                f.album_count = album_keys.remove(&d).map(|s| s.len()).unwrap_or(0);
                f
            })
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.decade));
        out
    }

    /// Pistes filtrées par genre (insensible à la casse) et/ou décennie (ex. 1990).
    pub fn tracks_filtered(&self, genre: Option<&str>, decade: Option<u32>) -> Vec<Track> {
        self.tracks()
            .into_iter()
            .filter(|t| {
                genre
                    .map(|g| genre_tags(t).iter().any(|x| x.eq_ignore_ascii_case(g)))
                    .unwrap_or(true)
                    && decade
                        .map(|d| t.year.map(|y| y / 10 * 10 == d).unwrap_or(false))
                        .unwrap_or(true)
            })
            .collect()
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

    /// Ajoute des pistes a une playlist existante (les IDs en double sont
    /// ignores). Retourne la playlist mise a jour, ou None si introuvable.
    pub fn add_to_playlist(&self, id: &str, track_ids: Vec<String>) -> Result<Option<Playlist>> {
        let raw = self.tree(TREE_PLAYLISTS).get(id.as_bytes()).ok().flatten();
        let mut p: Playlist = match raw.and_then(|v| dec::<Playlist>(&v).ok()) {
            Some(p) => p,
            None => return Ok(None),
        };
        for tid in track_ids {
            if !p.track_ids.contains(&tid) {
                p.track_ids.push(tid);
            }
        }
        self.tree(TREE_PLAYLISTS)
            .insert(p.id.as_bytes(), enc(&p)?)
            .map_err(se)?;
        Ok(Some(p))
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

    // ---------- favoris ----------

    /// Favoris courants (liste d'IDs, ordre d'ajout).
    pub fn favorites(&self) -> Favorites {
        self.tree(TREE_FAVS)
            .get(FAVS_KEY)
            .ok()
            .flatten()
            .and_then(|b| dec::<Favorites>(&b).ok())
            .unwrap_or_default()
    }

    fn save_favorites(&self, f: &Favorites) -> Result<()> {
        self.tree(TREE_FAVS).insert(FAVS_KEY, enc(f)?).map_err(se)?;
        Ok(())
    }

    /// Bascule un favori. Retourne `true` si la ressource est maintenant favorite.
    pub fn toggle_favorite(&self, kind: FavKind, id: &str) -> Result<bool> {
        let id = id.trim();
        if id.is_empty() {
            return Err(AurisError::other("id de favori vide"));
        }
        let mut f = self.favorites();
        let list = match kind {
            FavKind::Track => &mut f.tracks,
            FavKind::Album => &mut f.albums,
            FavKind::Artist => &mut f.artists,
        };
        let now = match list.iter().position(|x| x == id) {
            Some(pos) => {
                list.remove(pos);
                false
            }
            None => {
                list.push(id.to_string());
                true
            }
        };
        self.save_favorites(&f)?;
        Ok(now)
    }

    /// Pistes favorites résolues (les IDs inconnus sont ignorés).
    pub fn favorite_tracks(&self) -> Vec<Track> {
        self.favorites()
            .tracks
            .iter()
            .filter_map(|id| self.track(id))
            .collect()
    }

    /// Albums favoris résolus.
    pub fn favorite_albums(&self) -> Vec<Album> {
        self.favorites()
            .albums
            .iter()
            .filter_map(|id| self.album(id).map(|(a, _)| a))
            .collect()
    }

    /// Artistes favoris résolus.
    pub fn favorite_artists(&self) -> Vec<Artist> {
        self.favorites()
            .artists
            .iter()
            .filter_map(|id| self.artist(id).map(|(a, _)| a))
            .collect()
    }

    // ---------- historique d'écoute (#69) ----------

    fn history_entries(&self) -> Vec<HistoryEntry> {
        self.tree(TREE_HISTORY)
            .get(HISTORY_KEY)
            .ok()
            .flatten()
            .and_then(|b| dec::<Vec<HistoryEntry>>(&b).ok())
            .unwrap_or_default()
    }

    /// Enregistre le début d'une lecture (dédup consécutifs, cap à HISTORY_CAP).
    pub fn record_play(&self, track_id: &str) -> Result<()> {
        let mut h = self.history_entries();
        if h.last().map(|e| e.track_id.as_str()) == Some(track_id) {
            return Ok(()); // même piste enchaînée (tick d'horloge) : on ne duplique pas
        }
        h.push(HistoryEntry {
            track_id: track_id.to_string(),
            played_at: chrono::Utc::now().timestamp(),
        });
        if h.len() > HISTORY_CAP {
            h.drain(..h.len() - HISTORY_CAP);
        }
        self.tree(TREE_HISTORY)
            .insert(HISTORY_KEY, enc(&h)?)
            .map_err(se)?;
        Ok(())
    }

    /// Entrées récentes (plus récente d'abord), limitées à `limit`.
    pub fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        let mut h = self.history_entries();
        h.reverse();
        h.truncate(limit.max(1));
        h
    }

    /// Historique résolu en pistes complètes (les IDs inconnus sont ignorés).
    pub fn history_tracks(&self, limit: usize) -> Vec<(Track, i64)> {
        self.history(limit)
            .into_iter()
            .filter_map(|e| self.track(&e.track_id).map(|t| (t, e.played_at)))
            .collect()
    }

    // ---------- épingles (#70) ----------

    /// Épingles courantes (listes d'IDs, ordre d'ajout).
    pub fn pins(&self) -> Pins {
        self.tree(TREE_PINS)
            .get(PINS_KEY)
            .ok()
            .flatten()
            .and_then(|b| dec::<Pins>(&b).ok())
            .unwrap_or_default()
    }

    fn save_pins(&self, p: &Pins) -> Result<()> {
        self.tree(TREE_PINS).insert(PINS_KEY, enc(p)?).map_err(se)?;
        Ok(())
    }

    /// Bascule une épingle. Retourne `true` si la ressource est maintenant épinglée.
    pub fn toggle_pin(&self, kind: PinKind, id: &str) -> Result<bool> {
        let id = id.trim();
        if id.is_empty() {
            return Err(AurisError::other("id vide".to_string()));
        }
        let mut p = self.pins();
        let list = match kind {
            PinKind::Album => &mut p.albums,
            PinKind::Playlist => &mut p.playlists,
            PinKind::Radio => &mut p.radios,
        };
        let on = match list.iter().position(|x| x == id) {
            Some(i) => {
                list.remove(i);
                false
            }
            None => {
                list.push(id.to_string());
                true
            }
        };
        self.save_pins(&p)?;
        Ok(on)
    }

    /// Albums épinglés résolus (ordre d'épinglage, IDs inconnus ignorés).
    pub fn pinned_albums(&self) -> Vec<Album> {
        self.pins()
            .albums
            .iter()
            .filter_map(|id| self.album(id).map(|(a, _)| a))
            .collect()
    }

    /// Playlists épinglées résolues (ordre d'épinglage).
    pub fn pinned_playlists(&self) -> Vec<Playlist> {
        self.pins()
            .playlists
            .iter()
            .filter_map(|id| self.playlist(id).map(|(p, _)| p))
            .collect()
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

/// Plafonnement des groupes de résultats de recherche (#71).
const SEARCH_CAP: usize = 30;

fn cap_group<T>(v: Vec<T>) -> SearchGroup<T> {
    SearchGroup {
        total: v.len(),
        items: v.into_iter().take(SEARCH_CAP).collect(),
    }
}

/// Découpe un tag de genre en facettes (séparateur `;`), nettoyées et non vides.
fn genre_tags(t: &Track) -> Vec<String> {
    t.genre
        .as_deref()
        .map(|g| {
            g.split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
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
