//! Gestionnaire des sources: registre id -> backend, resolution d'URI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::http::HttpSource;
use crate::local::{LocalSource, UsbSource};
use crate::radio::RadioSource;
use crate::smb::SmbSource;
use crate::transfer::{copy_uri, join_rel, CopyReport};
use crate::upnp::{self, UpnpDevice};
use crate::{resolve_uri, SeekRead, SourceBackend, UriRef};

/// Durée de validité du cache de découverte SSDP (évite un M-SEARCH à chaque
/// navigation dans un dossier UPnP).
const UPNP_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

pub struct SourcesManager {
    sources: RwLock<Vec<Source>>,
    backends: RwLock<HashMap<String, Arc<dyn SourceBackend>>>,
    http: Arc<HttpSource>,
    upnp_cache: std::sync::Mutex<Option<(std::time::Instant, Vec<UpnpDevice>)>>,
}

impl SourcesManager {
    pub fn new(sources: Vec<Source>) -> Result<Self> {
        let http_src = Source {
            id: "__http__".into(),
            name: "HTTP direct".into(),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        };
        let http = Arc::new(HttpSource::new(http_src)?);
        let mgr = SourcesManager {
            sources: RwLock::new(Vec::new()),
            backends: RwLock::new(HashMap::new()),
            http,
            upnp_cache: std::sync::Mutex::new(None),
        };
        // Un config historique peut contenir des doublons d'id: la dernière occurrence gagne.
        let mut dedup: Vec<Source> = Vec::new();
        for s in sources {
            if let Some(pos) = dedup.iter().position(|x| x.id == s.id) {
                dedup[pos] = s;
            } else {
                dedup.push(s);
            }
        }
        for s in dedup {
            mgr.add(s)?;
        }
        Ok(mgr)
    }

    fn build_backend(src: &Source) -> Result<Arc<dyn SourceBackend>> {
        match &src.kind {
            SourceKind::Local { .. } => Ok(Arc::new(LocalSource::new(src.clone())?)),
            SourceKind::Usb { .. } => Ok(Arc::new(UsbSource::new(src.clone())?)),
            SourceKind::Smb { .. } => Ok(Arc::new(SmbSource::new(src.clone())?)),
            SourceKind::Http { .. } => Ok(Arc::new(HttpSource::new(src.clone())?)),
            SourceKind::Upnp { .. } => Ok(Arc::new(crate::upnp::UpnpSource::new(src.clone())?)), // ContentDirectory + lecture http des <res>
            SourceKind::Radio { .. } => Ok(Arc::new(RadioSource::new(src.clone())?)),
            SourceKind::LineIn { .. } => {
                Ok(Arc::new(crate::linein::LineInSource::new(src.clone())?))
            } // capture entrée audio live (#75)
            SourceKind::Qobuz { .. } => Ok(Arc::new(crate::qobuz::QobuzSource::new(src.clone())?)),
            SourceKind::Tidal { .. } => Ok(Arc::new(crate::tidal::TidalSource::new(src.clone())?)),
            SourceKind::Archive => Ok(Arc::new(crate::archive::ArchiveSource::new(src.clone())?)), // #84
            SourceKind::Deezer => Ok(Arc::new(crate::deezer::DeezerSource::new(src.clone())?)), // #85
            SourceKind::Jamendo { .. } => {
                Ok(Arc::new(crate::jamendo::JamendoSource::new(src.clone())?))
            } // #86
            SourceKind::Youtube => Ok(Arc::new(crate::youtube::YoutubeSource::new(src.clone())?)), // #87
        }
    }

    pub fn list(&self) -> Vec<Source> {
        self.sources.read().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Source> {
        self.sources
            .read()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    pub fn add(&self, source: Source) -> Result<()> {
        if self
            .sources
            .read()
            .unwrap()
            .iter()
            .any(|s| s.id == source.id)
        {
            return Err(AurisError::other(format!(
                "une source avec l'id '{}' existe déjà",
                source.id
            )));
        }
        let backend = Self::build_backend(&source)?;
        self.backends
            .write()
            .unwrap()
            .insert(source.id.clone(), backend);
        self.sources.write().unwrap().push(source);
        Ok(())
    }

    /// Remplace une source existante (meme id) par sa nouvelle definition.
    pub fn update(&self, source: Source) -> Result<()> {
        if self.get(&source.id).is_none() {
            return Err(AurisError::SourceNotFound(source.id.clone()));
        }
        self.remove(&source.id)?;
        self.add(source)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.backends.write().unwrap().remove(id);
        let mut s = self.sources.write().unwrap();
        let before = s.len();
        s.retain(|x| x.id != id);
        if s.len() == before {
            return Err(AurisError::SourceNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Active/desactive une source (le backend reste instancie).
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut s = self.sources.write().unwrap();
        let src = s
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or_else(|| AurisError::SourceNotFound(id.to_string()))?;
        src.enabled = enabled;
        Ok(())
    }

    pub fn backend(&self, id: &str) -> Option<Arc<dyn SourceBackend>> {
        self.backends.read().unwrap().get(id).cloned()
    }

    /// Recherche dans toutes les sources actives qui la supportent (Qobuz #25 ;
    /// les autres backends renvoient une liste vide via le défaut du trait).
    /// Une source en panne est ignorée (pas d'erreur globale).
    pub async fn search_sources(&self, query: &str) -> Vec<BrowseEntry> {
        let mut out = Vec::new();
        for s in self.list().into_iter().filter(|s| s.enabled) {
            if let Some(b) = self.backend(&s.id) {
                if let Ok(items) = b.search(query).await {
                    out.extend(items);
                }
            }
        }
        out
    }

    /// Liste le contenu d'un URI (`src://id/dossier`, `upnp://...`, ou URL playlist).
    pub async fn browse(&self, uri: &str) -> Result<Vec<BrowseEntry>> {
        if uri.starts_with("upnp://") {
            return self.browse_upnp(uri);
        }
        match resolve_uri(uri)? {
            UriRef::Src { source_id, path } => {
                let b = self
                    .backend(&source_id)
                    .ok_or_else(|| AurisError::SourceNotFound(source_id.clone()))?;
                b.list(&path).await
            }
            UriRef::Http(url) => self.http.list_url(&url).await,
        }
    }

    /// Ouvre un URI en lecture seekable.
    pub async fn open_uri(&self, uri: &str) -> Result<Box<dyn SeekRead>> {
        match resolve_uri(uri)? {
            UriRef::Src { source_id, path } => {
                let b = self
                    .backend(&source_id)
                    .ok_or_else(|| AurisError::SourceNotFound(source_id.clone()))?;
                b.open(&path).await
            }
            UriRef::Http(url) => {
                let f = self.http.open_url(&url)?;
                if f.size() == 0 {
                    // pas de Content-Length : flux live (webradio), pas un
                    // fichier — on bascule sur le lecteur de flux infini
                    // (dans un thread dédié : le client bloquant reqwest ne
                    // supporte pas d'être créé/détruit dans ce contexte async)
                    drop(f);
                    Ok(Box::new(crate::live::open_live_offthread(&url)?))
                } else {
                    Ok(Box::new(f))
                }
            }
        }
    }

    /// Chemin local absolu derrière un URI `src://`, si la source est un
    /// montage local/USB. `None` pour SMB/HTTP/UPnP/radio et les URI invalides.
    /// Utilisé par le repli ffmpeg pour un seek natif instantané (#54).
    pub fn local_path_for_uri(&self, uri: &str) -> Option<PathBuf> {
        match resolve_uri(uri).ok()? {
            UriRef::Src { source_id, path } => self.backend(&source_id)?.local_path(&path),
            UriRef::Http(_) => None,
        }
    }

    /// Decouverte UPnP des serveurs multimedia du LAN, mise en cache
    /// ([`UPNP_CACHE_TTL`]) pour ne pas relancer un M-SEARCH à chaque browse.
    pub fn discover_upnp(&self) -> Vec<UpnpDevice> {
        {
            let cache = self.upnp_cache.lock().unwrap();
            if let Some((t, devs)) = &*cache {
                if t.elapsed() < UPNP_CACHE_TTL {
                    return devs.clone();
                }
            }
        }
        self.refresh_upnp()
    }

    /// Force une nouvelle découverte SSDP et met le cache à jour.
    pub fn refresh_upnp(&self) -> Vec<UpnpDevice> {
        let devs = upnp::discover(std::time::Duration::from_secs(2));
        *self.upnp_cache.lock().unwrap() = Some((std::time::Instant::now(), devs.clone()));
        devs
    }

    /// Retrouve le device et l'object_id d'un URI `upnp://<location>[/<object_id>]`.
    /// Les object_id UPnP contiennent des '/': on apparie par prefixe sur la
    /// location des devices découverts (source de vérité), puis repli sur
    /// parse_upnp_uri. Si aucun device n'est en cache, une découverte forcée
    /// est tentée une fois.
    fn resolve_upnp_uri(&self, uri: &str) -> Result<(UpnpDevice, String)> {
        let mut devs = self.discover_upnp();
        for attempt in 0..2 {
            for dev in &devs {
                let root = format!("upnp://{}", dev.location);
                if uri == root {
                    return Ok((dev.clone(), "0".to_string()));
                }
                if let Some(object_id) = uri.strip_prefix(&format!("{}/", root)) {
                    return Ok((dev.clone(), object_id.to_string()));
                }
            }
            if attempt == 0 && devs.is_empty() {
                devs = self.refresh_upnp();
            } else {
                break;
            }
        }
        let (loc, object_id) =
            upnp::parse_upnp_uri(uri).ok_or_else(|| AurisError::InvalidUri(uri.into()))?;
        let dev = devs
            .into_iter()
            .find(|d| d.location == loc)
            .ok_or_else(|| AurisError::Network(format!("serveur UPnP introuvable: {loc}")))?;
        Ok((dev, object_id))
    }

    /// Browse UPnP: uri = upnp://<location>/<object_id> ("0" = racine).
    pub fn browse_upnp(&self, uri: &str) -> Result<Vec<BrowseEntry>> {
        let (dev, object_id) = self.resolve_upnp_uri(uri)?;
        upnp::browse(&dev, &object_id)
    }

    /// `stat` d'un URI UPnP (BrowseMetadata sur l'objet lui-même).
    pub fn stat_upnp(&self, uri: &str) -> Result<BrowseEntry> {
        let (dev, object_id) = self.resolve_upnp_uri(uri)?;
        upnp::stat(&dev, &object_id)
    }

    pub fn http_source(&self) -> Arc<HttpSource> {
        self.http.clone()
    }

    /// `stat` d'un URI (`src://id/chemin` ou `upnp://<location>/<object_id>`).
    /// Pour `src://`, retrouve l'entree en listant le parent ; la racine ("")
    /// est un dossier virtuel nomme d'apres la source. Pour `upnp://`,
    /// BrowseMetadata sur l'objet lui-même.
    pub async fn stat(&self, uri: &str) -> Result<BrowseEntry> {
        if uri.starts_with("upnp://") {
            return self.stat_upnp(uri);
        }
        let (source_id, path) =
            crate::parse_src_uri(uri).ok_or_else(|| AurisError::InvalidUri(uri.into()))?;
        let src = self
            .get(&source_id)
            .ok_or_else(|| AurisError::SourceNotFound(source_id.clone()))?;
        let rel = path.trim_matches('/');
        if rel.is_empty() {
            return Ok(BrowseEntry {
                name: src.name.clone(),
                uri: uri.to_string(),
                is_dir: true,
                size: None,
                format: None,
                is_audio: false,
            });
        }
        // Le backend peut statuer directement (ex. object_id UPnP) ; sinon
        // repli sur le listage du parent + match par nom.
        let backend = self.backend(&source_id);
        if let Some(b) = &backend {
            if let Some(entry) = b.stat(rel).await? {
                return Ok(entry);
            }
        }
        let (dir, name) = match rel.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), rel.to_string()),
        };
        let entries = self.browse(&crate::make_uri(&source_id, &dir)).await?;
        entries
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| AurisError::InvalidUri(format!("entree introuvable: {uri}")))
    }

    /// Parcours recursif d'un dossier de source: tous les fichiers audio
    /// (utile pour "tout lire" un dossier depuis l'explorateur).
    pub async fn walk_audio(&self, uri: &str) -> Result<Vec<BrowseEntry>> {
        let all = self.walk_all(uri).await?;
        Ok(all.into_iter().filter(|e| e.is_audio).collect())
    }

    /// Parcours recursif d'un dossier de source: tous les fichiers (audio ou non,
    /// utile pour l'export ZIP d'un dossier complet avec pochettes).
    pub async fn walk_all(&self, uri: &str) -> Result<Vec<BrowseEntry>> {
        let mut out = Vec::new();
        let mut stack = vec![(uri.to_string(), 0usize)];
        while let Some((u, depth)) = stack.pop() {
            if depth > 12 {
                continue;
            }
            match self.browse(&u).await {
                Ok(entries) => {
                    for e in entries {
                        if e.is_dir {
                            stack.push((e.uri.clone(), depth + 1));
                        } else {
                            out.push(e);
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("walk_all {u}: {err}");
                }
            }
        }
        out.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(out)
    }

    /// Copie un fichier ou un dossier d'une source vers un dossier d'une
    /// autre source (import). `to` = dossier destination `src://dst/...`.
    pub async fn copy(&self, from: &str, to: &str) -> Result<CopyReport> {
        copy_uri(self, from, to).await
    }

    /// Liste les sources vers lesquelles on peut copier (ecritables).
    pub fn writable_sources(&self) -> Vec<Source> {
        self.list()
            .into_iter()
            .filter(|s| s.kind.is_writable() && s.enabled)
            .collect()
    }

    /// Chemin relatif d'un URI src:// par rapport a une racine de dossier.
    pub fn rel_join(dir: &str, name: &str) -> String {
        join_rel(dir, name)
    }
}
