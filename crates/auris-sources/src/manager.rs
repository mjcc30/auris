//! Gestionnaire des sources: registre id -> backend, resolution d'URI.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::http::HttpSource;
use crate::local::LocalSource;
use crate::smb::SmbSource;
use crate::upnp::{self, UpnpDevice};
use crate::{resolve_uri, SeekRead, SourceBackend, UriRef};

pub struct SourcesManager {
    sources: RwLock<Vec<Source>>,
    backends: RwLock<HashMap<String, Arc<dyn SourceBackend>>>,
    http: Arc<HttpSource>,
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
        };
        for s in sources {
            mgr.add(s)?;
        }
        Ok(mgr)
    }

    fn build_backend(src: &Source) -> Result<Arc<dyn SourceBackend>> {
        match &src.kind {
            SourceKind::Local { .. } => Ok(Arc::new(LocalSource::new(src.clone())?)),
            SourceKind::Smb { .. } => Ok(Arc::new(SmbSource::new(src.clone())?)),
            SourceKind::Http { .. } => Ok(Arc::new(HttpSource::new(src.clone())?)),
            SourceKind::Upnp { .. } => Ok(Arc::new(HttpSource::new(src.clone())?)), // UPnP = browse + lecture http
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
        let backend = Self::build_backend(&source)?;
        self.backends
            .write()
            .unwrap()
            .insert(source.id.clone(), backend);
        self.sources.write().unwrap().push(source);
        Ok(())
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

    pub fn backend(&self, id: &str) -> Option<Arc<dyn SourceBackend>> {
        self.backends.read().unwrap().get(id).cloned()
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
            UriRef::Http(url) => Ok(Box::new(self.http.open_url(&url)?)),
        }
    }

    /// Decouverte UPnP des serveurs multimedia du LAN.
    pub fn discover_upnp(&self) -> Vec<UpnpDevice> {
        upnp::discover(std::time::Duration::from_secs(2))
    }

    /// Browse UPnP: uri = upnp://<location>/<object_id>.
    /// Les object_id UPnP contiennent des '/': on apparie d'abord par prefixe
    /// sur la location des devices découverts (source de vérité), puis repli
    /// sur parse_upnp_uri.
    pub fn browse_upnp(&self, uri: &str) -> Result<Vec<BrowseEntry>> {
        let devs = upnp::discover(std::time::Duration::from_millis(500));
        for dev in &devs {
            let prefix = format!("upnp://{}/", dev.location);
            if let Some(object_id) = uri.strip_prefix(&prefix) {
                return upnp::browse(dev, object_id);
            }
        }
        let (loc, object_id) =
            upnp::parse_upnp_uri(uri).ok_or_else(|| AurisError::InvalidUri(uri.into()))?;
        let dev = devs
            .into_iter()
            .find(|d| d.location == loc)
            .or_else(|| {
                upnp::discover(std::time::Duration::from_secs(2))
                    .into_iter()
                    .find(|d| d.location == loc)
            })
            .ok_or_else(|| AurisError::Network(format!("serveur UPnP introuvable: {loc}")))?;
        upnp::browse(&dev, &object_id)
    }

    pub fn http_source(&self) -> Arc<HttpSource> {
        self.http.clone()
    }
}
