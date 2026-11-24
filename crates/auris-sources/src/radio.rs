//! Source webradio: un flux HTTP(S) joue directement (pas de navigation).

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::{SeekRead, SourceBackend};

pub struct RadioSource {
    source: Source,
    url: String,
}

impl RadioSource {
    pub fn new(source: Source) -> Result<Self> {
        let url = match &source.kind {
            SourceKind::Radio { url } => url.clone(),
            _ => return Err(AurisError::other("source non radio")),
        };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(AurisError::InvalidUri(url));
        }
        Ok(RadioSource { source, url })
    }
}

#[async_trait]
impl SourceBackend for RadioSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, _rel_path: &str) -> Result<Vec<BrowseEntry>> {
        // une webradio = une seule entree jouable (l'URI est l'URL du flux,
        // geree par le moteur via UriRef::Http)
        Ok(vec![BrowseEntry {
            name: self.source.name.clone(),
            uri: self.url.clone(),
            is_dir: false,
            size: None,
            format: None,
            is_audio: true,
        }])
    }

    async fn open(&self, _rel_path: &str) -> Result<Box<dyn SeekRead>> {
        Err(AurisError::other(
            "webradio: lecture en streaming via l'URL directe, pas d'ouverture seekable",
        ))
    }
}
