//! Bibliotheque musicale: index persistant (sled), scan des sources,
//! agregation albums/artistes, recherche plein texte simple, pochettes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use auris_core::{
    Album, Artist, AurisError, DecadeFacet, FavKind, Favorites, GenreFacet, HistoryEntry,
    LibraryStats, PinKind, Pins, Playlist, Result, SearchGroup, SearchResults, ShareKind,
    ShareLink, SmartField, SmartOp, SmartRule, SmartRules, SmartSort, Source, Track,
};
use auris_player::probe_meta_with_path;
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
/// Loudness intégrée (LUFS) mesurée par piste, mise en cache (#37).
const TREE_LUFS: &str = "lufs";

/// Mémoïsation des listes agrégées : évite de relire/trier/décoder tout l'index
/// sled à chaque requête UI (16 k pistes = plusieurs dizaines de ms, ressenti comme
/// un chargement long à chaque navigation). Invalide sur toute mutation.
#[derive(Default)]
struct Memos {
    tracks: Option<(std::time::Instant, Arc<Vec<Track>>)>,
    albums: Option<(std::time::Instant, Arc<Vec<Album>>)>,
    artists: Option<(std::time::Instant, Arc<Vec<Artist>>)>,
    genres: Option<(std::time::Instant, Arc<Vec<GenreFacet>>)>,
    decades: Option<(std::time::Instant, Arc<Vec<DecadeFacet>>)>,
}

const TTL_TRACKS: std::time::Duration = std::time::Duration::from_secs(10);
const TTL_ALBUMS: std::time::Duration = std::time::Duration::from_secs(30);
const TTL_ARTISTS: std::time::Duration = std::time::Duration::from_secs(30);
const TTL_FACETS: std::time::Duration = std::time::Duration::from_secs(60);

pub struct Library {
    db: sled::Db,
    sources: Arc<SourcesManager>,
    scan_status: Arc<Mutex<ScanStatus>>,
    memo: Arc<Mutex<Memos>>,
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
            memo: Arc::new(Mutex::new(Memos::default())),
        })
    }

    /// Invalide tous les mémo (mutation de l'index : upsert, scan, purge…).
    pub fn invalidate(&self) {
        *self.memo.lock().unwrap() = Memos::default();
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
        let memo = self.memo.clone();
        *memo.lock().unwrap() = Memos::default();
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
                    *memo.lock().unwrap() = Memos::default();
                    return;
                }
            };
            rt.block_on(scan_source_blocking(db, sources, status, sid));
            *memo.lock().unwrap() = Memos::default();
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
        let memo = self.memo.clone();
        *memo.lock().unwrap() = Memos::default();
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
                    *memo.lock().unwrap() = Memos::default();
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
            *memo.lock().unwrap() = Memos::default();
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
        {
            let m = self.memo.lock().unwrap();
            if let Some((at, v)) = &m.tracks {
                if at.elapsed() < TTL_TRACKS {
                    return v.as_ref().clone();
                }
            }
        }
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
        let arc = Arc::new(out);
        self.memo.lock().unwrap().tracks = Some((std::time::Instant::now(), arc.clone()));
        arc.as_ref().clone()
    }

    /// Page de pistes (filtres genre/décennie + sous-chaîne q sur titre/artiste/album),
    /// avec le total avant pagination — pour un onglet « Titres » rapide même avec
    /// des dizaines de milliers de pistes (#89).
    pub fn tracks_page(
        &self,
        genre: Option<&str>,
        decade: Option<u32>,
        q: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> (usize, Vec<Track>) {
        let all = self.tracks_filtered(genre, decade);
        let ql = q.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
        let filtered: Vec<Track> = match &ql {
            Some(ql) => all
                .into_iter()
                .filter(|t| {
                    t.title.to_lowercase().contains(ql)
                        || t.artist.to_lowercase().contains(ql)
                        || t.album_artist.to_lowercase().contains(ql)
                        || t.album.to_lowercase().contains(ql)
                })
                .collect(),
            None => all,
        };
        let total = filtered.len();
        let items = filtered.into_iter().skip(offset).take(limit).collect();
        (total, items)
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
        self.invalidate();
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
    /// vrai compte. Classement sémantique embarqué (#38) : BM25-lite pondéré par champ,
    /// expansion par synonymes/humeurs, correspondance floue par trigrammes.
    pub fn search_grouped(&self, q: &str, source_id: Option<&str>) -> SearchResults {
        let src = source_id.map(str::trim).filter(|s| !s.is_empty());
        let terms = auris_core::search_semantic::query_terms(q);
        let all = self.tracks();
        let mut scored: Vec<(f64, Track)> = all
            .iter()
            .filter(|t| src.map(|s| t.source_id == s).unwrap_or(true))
            .map(|t| {
                let fields = auris_core::search_semantic::track_fields(
                    &t.title,
                    &t.artist,
                    &t.album_artist,
                    &t.album,
                    t.genre.as_deref(),
                );
                (
                    auris_core::search_semantic::score_fields(&terms, &fields),
                    t.clone(),
                )
            })
            .filter(|(s, _)| *s > 0.0 || terms.is_empty()) // q vide = tout, comme avant #38
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))
        });
        let tracks: Vec<Track> = scored.into_iter().map(|(_, t)| t).collect();

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
        let mut albums: Vec<(f64, Album)> = self
            .albums()
            .into_iter()
            .filter(|a| src.is_none() || src_album_keys.contains(album_key_of(a).as_str()))
            .map(|a| {
                let s = auris_core::search_semantic::score_fields(
                    &terms,
                    &[(&a.title, 3.0), (&a.artist, 2.0)],
                );
                (s, a)
            })
            .filter(|(s, _)| *s > 0.0 || terms.is_empty()) // q vide = tout, comme avant #38
            .collect();
        albums.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))
        });
        let albums: Vec<Album> = albums.into_iter().map(|(_, a)| a).collect();
        let mut artists: Vec<(f64, Artist)> = self
            .artists()
            .into_iter()
            .filter(|a| src.is_none() || src_artist_keys.contains(a.name.to_lowercase().as_str()))
            .map(|a| {
                let s = auris_core::search_semantic::score_fields(&terms, &[(&a.name, 3.0)]);
                (s, a)
            })
            .filter(|(s, _)| *s > 0.0 || terms.is_empty()) // q vide = tout, comme avant #38
            .collect();
        artists.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        });
        let artists: Vec<Artist> = artists.into_iter().map(|(_, a)| a).collect();
        let track_src: HashMap<&str, &str> = all
            .iter()
            .map(|t| (t.id.as_str(), t.source_id.as_str()))
            .collect();
        let mut playlists: Vec<(f64, Playlist)> = self
            .playlists()
            .into_iter()
            .filter(|p| match src {
                None => true,
                Some(s) => p
                    .track_ids
                    .iter()
                    .any(|id| track_src.get(id.as_str()) == Some(&s)),
            })
            .map(|p| {
                let s = auris_core::search_semantic::score_fields(&terms, &[(&p.name, 3.0)]);
                (s, p)
            })
            .filter(|(s, _)| *s > 0.0 || terms.is_empty()) // q vide = tout, comme avant #38
            .collect();
        playlists.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        });
        let playlists: Vec<Playlist> = playlists.into_iter().map(|(_, p)| p).collect();

        SearchResults {
            tracks: cap_group(tracks),
            albums: cap_group(albums),
            artists: cap_group(artists),
            playlists: cap_group(playlists),
            radios: SearchGroup {
                total: 0,
                items: Vec::new(),
            },
            sources: SearchGroup {
                total: 0,
                items: Vec::new(),
            },
        }
    }

    pub fn albums(&self) -> Vec<Album> {
        {
            let m = self.memo.lock().unwrap();
            if let Some((at, v)) = &m.albums {
                if at.elapsed() < TTL_ALBUMS {
                    return v.as_ref().clone();
                }
            }
        }
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
        let arc = Arc::new(out);
        self.memo.lock().unwrap().albums = Some((std::time::Instant::now(), arc.clone()));
        arc.as_ref().clone()
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
        {
            let m = self.memo.lock().unwrap();
            if let Some((at, v)) = &m.artists {
                if at.elapsed() < TTL_ARTISTS {
                    return v.as_ref().clone();
                }
            }
        }
        let tree = self.tree(TREE_ARTISTS);
        let mut out: Vec<Artist> = tree
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| dec::<Artist>(&v).ok())
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        let arc = Arc::new(out);
        self.memo.lock().unwrap().artists = Some((std::time::Instant::now(), arc.clone()));
        arc.as_ref().clone()
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
        {
            let m = self.memo.lock().unwrap();
            if let Some((at, v)) = &m.genres {
                if at.elapsed() < TTL_FACETS {
                    return v.as_ref().clone();
                }
            }
        }
        let out = self.genres_uncached();
        let arc = Arc::new(out);
        self.memo.lock().unwrap().genres = Some((std::time::Instant::now(), arc.clone()));
        arc.as_ref().clone()
    }

    fn genres_uncached(&self) -> Vec<GenreFacet> {
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
        {
            let m = self.memo.lock().unwrap();
            if let Some((at, v)) = &m.decades {
                if at.elapsed() < TTL_FACETS {
                    return v.as_ref().clone();
                }
            }
        }
        let out = self.decades_uncached();
        let arc = Arc::new(out);
        self.memo.lock().unwrap().decades = Some((std::time::Instant::now(), arc.clone()));
        arc.as_ref().clone()
    }

    fn decades_uncached(&self) -> Vec<DecadeFacet> {
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

    /// Loudness intégrée (LUFS) en cache pour une piste (None = non mesurée).
    pub fn loudness_get(&self, track_id: &str) -> Option<f64> {
        self.tree(TREE_LUFS)
            .get(track_id.as_bytes())
            .ok()?
            .and_then(|v| dec::<f64>(&v).ok())
    }

    /// Met en cache la loudness d'une piste.
    pub fn loudness_set(&self, track_id: &str, lufs: f64) -> Result<()> {
        self.tree(TREE_LUFS)
            .insert(
                track_id.as_bytes(),
                enc(&lufs).map_err(|e| AurisError::Storage(e.to_string()))?,
            )
            .map_err(se)?;
        Ok(())
    }

    /// Nombre de pistes dont la loudness est en cache.
    pub fn loudness_count(&self) -> usize {
        self.tree(TREE_LUFS).len()
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
            rules: None,
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

    /// Playlist + ses pistes résolues. Pour une smart playlist (#98), les pistes
    /// sont évaluées dynamiquement depuis les règles ; sinon IDs statiques.
    pub fn playlist(&self, id: &str) -> Option<(Playlist, Vec<Track>)> {
        let p: Playlist = dec(&self.tree(TREE_PLAYLISTS).get(id.as_bytes()).ok()??).ok()?;
        let tracks = match &p.rules {
            Some(r) => self.eval_smart(r),
            None => p
                .track_ids
                .iter()
                .filter_map(|tid| self.track(tid))
                .collect(),
        };
        Some((p, tracks))
    }

    pub fn delete_playlist(&self, id: &str) -> Result<bool> {
        Ok(self
            .tree(TREE_PLAYLISTS)
            .remove(id.as_bytes())
            .map_err(se)?
            .is_some())
    }

    /// Crée une smart playlist (#98) : liste dynamique évaluée à chaque lecture.
    pub fn create_smart_playlist(&self, name: &str, rules: SmartRules) -> Result<Playlist> {
        let mut p = self.create_playlist(name, Vec::new())?;
        p.rules = Some(rules);
        self.tree(TREE_PLAYLISTS)
            .insert(p.id.as_bytes(), enc(&p)?)
            .map_err(se)?;
        Ok(p)
    }

    /// Met à jour le nom et/ou les règles d'une playlist. `rules`: `Some(Some(r))`
    /// = smart playlist, `Some(None)` = repasse en statique, `None` = inchangé.
    pub fn update_playlist(
        &self,
        id: &str,
        name: Option<&str>,
        rules: Option<Option<SmartRules>>,
    ) -> Result<Option<Playlist>> {
        let raw = self.tree(TREE_PLAYLISTS).get(id.as_bytes()).ok().flatten();
        let mut p: Playlist = match raw.and_then(|v| dec(&v).ok()) {
            Some(p) => p,
            None => return Ok(None),
        };
        if let Some(n) = name {
            let n = n.trim();
            if n.is_empty() {
                return Err(AurisError::other("nom de playlist vide"));
            }
            p.name = n.to_string();
        }
        if let Some(r) = rules {
            p.rules = r;
        }
        self.tree(TREE_PLAYLISTS)
            .insert(p.id.as_bytes(), enc(&p)?)
            .map_err(se)?;
        Ok(Some(p))
    }

    /// Évalue des règles de smart playlist (#98) contre la bibliothèque :
    /// filtre (ET/OU), trie, limite.
    pub fn eval_smart(&self, rules: &SmartRules) -> Vec<Track> {
        eval_smart_tracks(self.tracks(), rules)
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

/// Remplit les crédits d'album (label/éditeur/copyright) depuis une piste (#73) :
/// la première valeur trouvée gagne.
fn merge_album_credits(a: &mut Album, t: &Track) {
    if a.label.is_none() {
        a.label = t.label.clone();
    }
    if a.publisher.is_none() {
        a.publisher = t.publisher.clone();
    }
    if a.copyright.is_none() {
        a.copyright = t.copyright.clone();
    }
}

/// Plafonnement des groupes de résultats de recherche (#71).
/// Plafond d'items par groupe de résultats de recherche (exposé pour le web).
pub const SEARCH_CAP: usize = 30;

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

// ---------- Smart playlists (#98) ----------

/// Champ d'une piste vu comme chaîne (comparaisons insensibles à la casse).
fn smart_field_str(t: &Track, f: SmartField) -> Option<String> {
    match f {
        SmartField::Title => Some(t.title.clone()),
        SmartField::Artist => Some(t.artist.clone()),
        SmartField::Album => Some(t.album.clone()),
        SmartField::AlbumArtist => Some(t.album_artist.clone()),
        SmartField::Genre => t.genre.clone(),
        SmartField::Format => t.format.map(|fmt| match fmt {
            auris_core::AudioFormat::Pcm { .. } => "pcm".to_string(),
            auris_core::AudioFormat::Dsd { .. } => "dsd".to_string(),
        }),
        SmartField::Source => Some(t.source_id.clone()),
        _ => None,
    }
}

/// Champ d'une piste vu comme nombre (comparaisons numériques).
fn smart_field_num(t: &Track, f: SmartField) -> Option<f64> {
    match f {
        SmartField::Year => t.year.map(f64::from),
        SmartField::DurationSecs => t.duration_secs,
        SmartField::TrackNumber => t.track_number.map(f64::from),
        SmartField::BitDepth => match t.format {
            Some(auris_core::AudioFormat::Pcm { bits, .. }) => Some(f64::from(bits)),
            _ => None,
        },
        SmartField::SampleRate => match t.format {
            Some(auris_core::AudioFormat::Pcm { sample_rate, .. })
            | Some(auris_core::AudioFormat::Dsd { sample_rate, .. }) => {
                Some(f64::from(sample_rate))
            }
            None => None,
        },
        _ => None,
    }
}

/// Une règle matche-t-elle la piste ?
fn smart_rule_match(r: &SmartRule, t: &Track) -> bool {
    if r.field == SmartField::HasCover {
        let v = r.value.as_bool().unwrap_or(false);
        return match r.op {
            SmartOp::Eq => t.has_cover == v,
            SmartOp::Ne => t.has_cover != v,
            _ => false,
        };
    }
    if let Some(n) = smart_field_num(t, r.field) {
        let v = r
            .value
            .as_f64()
            .or_else(|| r.value.as_str().and_then(|s| s.parse::<f64>().ok()));
        return match (v, r.op) {
            (Some(v), SmartOp::Eq) => n == v,
            (Some(v), SmartOp::Ne) => n != v,
            (Some(v), SmartOp::Gt) => n > v,
            (Some(v), SmartOp::Lt) => n < v,
            (Some(v), SmartOp::Gte) => n >= v,
            (Some(v), SmartOp::Lte) => n <= v,
            // champ numérique présent mais valeur non numérique : eq échoue, ne réussit
            (None, SmartOp::Ne) => true,
            _ => false,
        };
    }
    let sl = smart_field_str(t, r.field)
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let v = r
        .value
        .as_str()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    match r.op {
        SmartOp::Eq => sl == v,
        SmartOp::Ne => sl != v,
        SmartOp::Contains => !v.is_empty() && sl.contains(&v),
        SmartOp::Gt => sl > v,
        SmartOp::Lt => sl < v,
        SmartOp::Gte => sl >= v,
        SmartOp::Lte => sl <= v,
    }
}

/// Évalue des règles (#98) sur un ensemble de pistes : filtre (ET/OU), trie, limite.
pub fn eval_smart_tracks(tracks: Vec<Track>, rules: &SmartRules) -> Vec<Track> {
    let mut out: Vec<Track> = tracks
        .into_iter()
        .filter(|t| smart_match(rules, t))
        .collect();
    if let Some(s) = &rules.sort {
        smart_sort(&mut out, s);
    }
    if let Some(l) = rules.limit {
        out.truncate(l);
    }
    out
}

fn smart_match(rules: &SmartRules, t: &Track) -> bool {
    if rules.rules.is_empty() {
        return true;
    }
    if rules.match_all {
        rules.rules.iter().all(|r| smart_rule_match(r, t))
    } else {
        rules.rules.iter().any(|r| smart_rule_match(r, t))
    }
}

/// Clé de tri hétérogène : bool < nombre < chaîne.
#[derive(Debug, PartialEq, PartialOrd)]
enum SmartKey {
    B(bool),
    N(f64),
    S(String),
}

fn smart_key(t: &Track, f: SmartField) -> SmartKey {
    if f == SmartField::HasCover {
        return SmartKey::B(t.has_cover);
    }
    match smart_field_num(t, f) {
        Some(n) => SmartKey::N(n),
        None => SmartKey::S(
            smart_field_str(t, f)
                .map(|s| s.to_lowercase())
                .unwrap_or_default(),
        ),
    }
}

fn smart_sort(out: &mut [Track], s: &SmartSort) {
    out.sort_by(|a, b| {
        let ord = smart_key(a, s.field)
            .partial_cmp(&smart_key(b, s.field))
            .unwrap_or(std::cmp::Ordering::Equal);
        if s.desc {
            ord.reverse()
        } else {
            ord
        }
    });
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
    // Chemin local si dispo : ffprobe sonde le fichier directement (#54).
    let local_path = sources.local_path_for_uri(uri);
    let (meta, _spec, dur) = probe_meta_with_path(reader, &ext, local_path)?;
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
    // credits étendus (#73)
    t.composer = meta.composer.clone();
    t.producer = meta.producer.clone();
    t.label = meta.label.clone();
    t.publisher = meta.publisher.clone();
    t.isrc = meta.isrc.clone();
    t.copyright = meta.copyright.clone();
    t.lyrics = meta.lyrics.clone();
    t.mb_track_id = meta.mb_track_id.clone();
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
                            cover_track_id: if t.has_cover {
                                Some(t.id.clone())
                            } else {
                                None
                            },
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
                            label: t.label.clone(),
                            publisher: t.publisher.clone(),
                            copyright: t.copyright.clone(),
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
            merge_album_credits(&mut a, &t);
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
            if a.cover_track_id.is_none() && t.has_cover {
                a.cover_track_id = Some(t.id.clone());
            }
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

// ---------- sauvegarde / restauration (#40) ----------

impl Library {
    /// Remplace les favoris (restauration de sauvegarde).
    pub fn restore_favorites(&self, f: &Favorites) -> Result<()> {
        self.save_favorites(f)
    }

    /// Remplace les épingles (restauration de sauvegarde).
    pub fn restore_pins(&self, p: &Pins) -> Result<()> {
        self.save_pins(p)
    }

    /// Remplace l'historique d'écoute (restauration de sauvegarde).
    pub fn restore_history(&self, h: &[HistoryEntry]) -> Result<()> {
        self.tree(TREE_HISTORY)
            .insert(HISTORY_KEY, enc(&h.to_vec())?)
            .map_err(se)?;
        Ok(())
    }

    /// Ajoute/remplace des playlists (restauration de sauvegarde).
    pub fn restore_playlists(&self, ps: &[Playlist]) -> Result<()> {
        for p in ps {
            self.tree(TREE_PLAYLISTS)
                .insert(p.id.as_bytes(), enc(p)?)
                .map_err(se)?;
        }
        Ok(())
    }

    /// Ajoute/remplace des liens de partage (restauration de sauvegarde).
    pub fn restore_shares(&self, ss: &[ShareLink]) -> Result<()> {
        for s in ss {
            self.tree(TREE_SHARES)
                .insert(s.token.as_bytes(), enc(s)?)
                .map_err(se)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_with_credits() -> Track {
        let mut t = Track::new("src://s/a.flac".into(), "s".into());
        t.label = Some("Deutsche Grammophon".into());
        t.publisher = Some("DG".into());
        t.copyright = Some("© 2018".into());
        t
    }

    #[test]
    fn credits_album_remplis_depuis_la_piste() {
        let mut a = Album {
            id: "a1".into(),
            title: "Album".into(),
            artist: "Artiste".into(),
            year: None,
            cover_track_id: None,
            track_count: 0,
            duration_secs: 0.0,
            label: None,
            publisher: None,
            copyright: None,
        };
        merge_album_credits(&mut a, &track_with_credits());
        assert_eq!(a.label.as_deref(), Some("Deutsche Grammophon"));
        assert_eq!(a.publisher.as_deref(), Some("DG"));
        assert_eq!(a.copyright.as_deref(), Some("© 2018"));
        // une seconde piste ne réécrase pas
        let mut t2 = Track::new("src://s/b.flac".into(), "s".into());
        t2.label = Some("Autre label".into());
        merge_album_credits(&mut a, &t2);
        assert_eq!(a.label.as_deref(), Some("Deutsche Grammophon"));
    }

    fn mk_track(
        title: &str,
        artist: &str,
        genre: Option<&str>,
        year: Option<u32>,
        secs: f64,
    ) -> Track {
        let mut t = Track::new(format!("src://s/{title}.flac"), "s".into());
        t.title = title.into();
        t.artist = artist.into();
        t.genre = genre.map(str::to_string);
        t.year = year;
        t.duration_secs = Some(secs);
        t
    }

    fn rule(field: SmartField, op: SmartOp, value: serde_json::Value) -> SmartRule {
        SmartRule { field, op, value }
    }

    #[test]
    fn smart_playlist_filtre_et_tri() {
        let tracks = vec![
            mk_track("A", "X", Some("Jazz"), Some(1975), 200.0),
            mk_track("B", "Y", Some("Rock"), Some(1990), 300.0),
            mk_track("C", "X", Some("Jazz"), Some(1985), 100.0),
        ];
        // genre=Jazz ET année>1980, tri année desc, limite 10
        let rules = SmartRules {
            match_all: true,
            rules: vec![
                rule(SmartField::Genre, SmartOp::Eq, "jazz".into()),
                rule(SmartField::Year, SmartOp::Gt, 1980.into()),
            ],
            sort: Some(SmartSort {
                field: SmartField::Year,
                desc: true,
            }),
            limit: Some(10),
        };
        let out = eval_smart_tracks(tracks.clone(), &rules);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "C");
        // OU : Jazz ou Rock -> 3, tri durée asc
        let rules = SmartRules {
            match_all: false,
            rules: vec![
                rule(SmartField::Genre, SmartOp::Eq, "jazz".into()),
                rule(SmartField::Genre, SmartOp::Eq, "rock".into()),
            ],
            sort: Some(SmartSort {
                field: SmartField::DurationSecs,
                desc: false,
            }),
            limit: None,
        };
        let out = eval_smart_tracks(tracks.clone(), &rules);
        assert_eq!(
            out.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["C", "A", "B"]
        );
        // limite
        let mut rules = rules.clone();
        rules.limit = Some(2);
        let out = eval_smart_tracks(tracks, &rules);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn smart_playlist_contains_et_champ_absent() {
        let tracks = vec![
            mk_track("The Best Of", "X", None, None, 10.0),
            mk_track("Other", "Y", None, None, 20.0),
        ];
        // title contient "best" (casse-insensitive)
        let rules = SmartRules {
            match_all: true,
            rules: vec![rule(SmartField::Title, SmartOp::Contains, "BEST".into())],
            sort: None,
            limit: None,
        };
        let out = eval_smart_tracks(tracks.clone(), &rules);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "The Best Of");
        // genre absent : eq "" matche les pistes sans genre, ne "" ne matche pas
        let rules = SmartRules {
            match_all: true,
            rules: vec![rule(SmartField::Genre, SmartOp::Ne, "jazz".into())],
            sort: None,
            limit: None,
        };
        assert_eq!(eval_smart_tracks(tracks.clone(), &rules).len(), 2);
    }
}
