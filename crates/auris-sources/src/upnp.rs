//! UPnP/DLNA: decouverte SSDP des serveurs multimedia sur le LAN, puis
//! navigation dans le ContentDirectory (Browse SOAP). Les pistes jouees
//! passent ensuite par la source HTTP (l'URI du <res>).

use std::net::UdpSocket;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use auris_core::{AurisError, BrowseEntry, Result, Source, SourceKind};

use crate::http::HttpSource;
use crate::http_raw::{http_get, http_post, Url};
use crate::{SeekRead, SourceBackend};

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpnpDevice {
    pub friendly_name: String,
    pub location: String,
    pub base_url: String,
    pub content_directory_control: String,
}

/// Decouverte SSDP des MediaServer (type 1 et 2).
pub fn discover(timeout: Duration) -> Vec<UpnpDevice> {
    let mut found: Vec<UpnpDevice> = Vec::new();
    let mut locations: Vec<String> = Vec::new();
    for st in ["urn:schemas-upnp-org:device:MediaServer:1", "ssdp:all"] {
        let sock = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => continue,
        };
        let _ = sock.set_read_timeout(Some(Duration::from_millis(800)));
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\nUSER-AGENT: auris/0.1\r\n\r\n"
        );
        let _ = sock.send_to(msg.as_bytes(), "239.255.255.250:1900");
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 8192];
        while Instant::now() < deadline {
            match sock.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if let Some(loc) = header_value(&text, "LOCATION") {
                        if !locations.contains(&loc) {
                            locations.push(loc);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if st.starts_with("urn:") {
            break; // la recherche MediaServer suffit
        }
    }
    for loc in locations {
        if let Ok(dev) = fetch_device(&loc) {
            if !found.iter().any(|d| d.location == dev.location) {
                found.push(dev);
            }
        }
    }
    found
}

fn header_value(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn fetch_device(location: &str) -> Result<UpnpDevice> {
    let resp = http_get(location, &[], None, Duration::from_secs(5))?;
    if resp.status != 200 {
        return Err(AurisError::Network(format!(
            "UPnP {location}: HTTP {}",
            resp.status
        )));
    }
    let xml = String::from_utf8_lossy(&resp.body).to_string();
    let base = tag_content(&xml, "URLBase").unwrap_or_else(|| {
        let u = Url::parse(location)
            .map(|u| format!("http://{}:{}", u.host, u.port))
            .unwrap_or_default();
        u
    });
    let friendly = tag_content(&xml, "friendlyName").unwrap_or_else(|| "Serveur UPnP".into());
    // Trouver le service ContentDirectory
    let mut control = None;
    let mut rest = xml.as_str();
    while let Some(pos) = rest.find("<service>") {
        let end = rest[pos..]
            .find("</service>")
            .map(|e| pos + e + "</service>".len())
            .unwrap_or(rest.len());
        let block = &rest[pos..end.min(rest.len())];
        if let Some(st) = tag_content(block, "serviceType") {
            if st.contains("ContentDirectory") {
                control = tag_content(block, "controlURL");
                break;
            }
        }
        rest = &rest[pos + 8..];
    }
    let control = control
        .ok_or_else(|| AurisError::Network(format!("UPnP {location}: pas de ContentDirectory")))?;
    let control_abs = absolutize(&base, &control);
    Ok(UpnpDevice {
        friendly_name: friendly,
        location: location.to_string(),
        base_url: base,
        content_directory_control: control_abs,
    })
}

fn absolutize(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if let Ok(b) = Url::parse(base) {
        let path = if url.starts_with('/') {
            url.to_string()
        } else {
            format!("/{}", url)
        };
        // si base contient un chemin, on garde seulement le scheme://host:port
        format!("http://{}:{}{}", b.host, b.port, path)
    } else {
        url.to_string()
    }
}

/// Extrait le contenu du premier `<tag>...</tag>` (namespaces toleres).
pub fn tag_content(xml: &str, tag: &str) -> Option<String> {
    // match <prefix:tag ...> ou <tag ...>
    let needle_open = format!(":{}", tag);
    let mut search_from = 0;
    while let Some(rel) = xml[search_from..].find('<') {
        let pos = search_from + rel;
        let rest = &xml[pos..];
        let close = rest.find('>')?;
        let tagpart = &rest[1..close];
        let name = tagpart.split_whitespace().next()?;
        if name == tag || name.ends_with(&needle_open) {
            let content_start = pos + close + 1;
            let close_tag = format!("</{}>", name);
            let end = xml[content_start..]
                .find(&close_tag)
                .map(|e| content_start + e)?;
            return Some(xml[content_start..end].trim().to_string());
        }
        search_from = pos + 1;
    }
    None
}

/// Extrait tous les blocs `<tag ...>...</tag>` avec leurs attributs.
fn tag_blocks<'a>(xml: &'a str, tag: &str) -> Vec<(String, &'a str)> {
    let mut out = Vec::new();
    let needle_open = format!(":{}", tag);
    let mut search_from = 0;
    while let Some(rel) = xml[search_from..].find('<') {
        let pos = search_from + rel;
        let rest = &xml[pos..];
        let Some(close) = rest.find('>') else { break };
        let tagpart = &rest[1..close];
        let name = match tagpart.split_whitespace().next() {
            Some(n) => n,
            None => {
                search_from = pos + 1;
                continue;
            }
        };
        if name == tag || name.ends_with(&needle_open) {
            // self-closing?
            if tagpart.ends_with('/') {
                out.push((tagpart.trim_end_matches('/').to_string(), ""));
                search_from = pos + close + 1;
                continue;
            }
            let content_start = pos + close + 1;
            let close_tag = format!("</{}>", name);
            match xml[content_start..].find(&close_tag) {
                Some(e) => {
                    let end = content_start + e;
                    out.push((tagpart.to_string(), &xml[content_start..end]));
                    search_from = end + close_tag.len();
                }
                None => break,
            }
        } else {
            search_from = pos + 1;
        }
    }
    out
}

fn attr<'a>(tag_attrs: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{}=\"", name);
    let pos = tag_attrs.find(&needle)? + needle.len();
    let rest = &tag_attrs[pos..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Appel SOAP `Browse` du ContentDirectory. Retourne le DIDL (XML desentry),
/// déjà unescapé. `browse_flag` = "BrowseDirectChildren" ou "BrowseMetadata".
pub fn soap_browse(dev: &UpnpDevice, object_id: &str, browse_flag: &str) -> Result<String> {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">
<ObjectID>{}</ObjectID><BrowseFlag>{}</BrowseFlag><Filter>*</Filter>
<StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria>
</u:Browse></s:Body></s:Envelope>"#,
        escape_xml(object_id),
        browse_flag
    );
    let resp = http_post(
        &dev.content_directory_control,
        "text/xml; charset=\"utf-8\"",
        "\"urn:schemas-upnp-org:service:ContentDirectory:1#Browse\"",
        &body,
        Duration::from_secs(10),
    )?;
    if resp.status != 200 {
        return Err(AurisError::Network(format!(
            "UPnP Browse: HTTP {}",
            resp.status
        )));
    }
    let text = String::from_utf8_lossy(&resp.body).to_string();
    let result = tag_content(&text, "Result").unwrap_or_default();
    Ok(xml_unescape(&result))
}

/// Browse le ContentDirectory. `object_id` = "0" (racine), "0/3", ...
/// Les containers recoivent un URI `upnp://<location>/<object_id>` ; les items
/// l'URL HTTP de leur <res>.
pub fn browse(dev: &UpnpDevice, object_id: &str) -> Result<Vec<BrowseEntry>> {
    let didl = soap_browse(dev, object_id, "BrowseDirectChildren")?;
    parse_didl(&format!("upnp://{}", dev.location), &dev.base_url, &didl)
}

/// `stat` d'un objet UPnP via BrowseMetadata (l'objet lui-meme, pas ses enfants).
/// Les containers recoivent un URI `upnp://<location>/<object_id>` ; les items
/// l'URL HTTP de leur <res>.
pub fn stat(dev: &UpnpDevice, object_id: &str) -> Result<BrowseEntry> {
    stat_with_prefix(&format!("upnp://{}", dev.location), dev, object_id)
}

/// Variante de [`stat`] avec un prefixe d'URI personnalise (ex. `src://<id>`
/// pour une source UPnP configurée manuellement).
pub fn stat_with_prefix(
    uri_prefix: &str,
    dev: &UpnpDevice,
    object_id: &str,
) -> Result<BrowseEntry> {
    let didl = soap_browse(dev, object_id, "BrowseMetadata")?;
    let prefix = uri_prefix.trim_end_matches('/');
    if let Some((attrs, _)) = tag_blocks(&didl, "container").into_iter().next() {
        let id = attr(&attrs, "id").unwrap_or("0").to_string();
        let block = find_block(&didl, "container", &attrs).unwrap_or("");
        let title = tag_content(block, "dc:title").unwrap_or_else(|| "Sans titre".into());
        return Ok(BrowseEntry {
            name: title,
            uri: format!("{prefix}/{id}"),
            is_dir: true,
            size: None,
            format: None,
            is_audio: false,
        });
    }
    if let Some((attrs, _)) = tag_blocks(&didl, "item").into_iter().next() {
        let block = find_block(&didl, "item", &attrs).unwrap_or("");
        let title = tag_content(block, "dc:title").unwrap_or_else(|| "Sans titre".into());
        let class = tag_content(block, "upnp:class").unwrap_or_default();
        let res = tag_blocks(block, "res").into_iter().next();
        let (url, size) = match res {
            Some((rattrs, rcontent)) => (
                absolutize(&dev.base_url, rcontent.trim()),
                attr(&rattrs, "size").and_then(|s| s.parse::<u64>().ok()),
            ),
            None => (String::new(), None),
        };
        if url.is_empty() {
            return Err(AurisError::InvalidUri(format!(
                "objet UPnP sans <res>: {object_id}"
            )));
        }
        let is_audio = class.contains("audioItem") || class.contains("musicTrack");
        return Ok(BrowseEntry {
            name: title,
            uri: url,
            is_dir: false,
            size,
            format: None,
            is_audio,
        });
    }
    Err(AurisError::InvalidUri(format!(
        "objet UPnP introuvable: {object_id}"
    )))
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Parse un DIDL en entrees de parcours. `uri_prefix` sert a construire l'URI
/// des containers (`upnp://<location>` ou `src://<source_id>`) ; les items
/// prennent l'URL absolue de leur <res> (via `base_url`).
fn parse_didl(uri_prefix: &str, base_url: &str, didl: &str) -> Result<Vec<BrowseEntry>> {
    let mut out = Vec::new();
    let prefix = uri_prefix.trim_end_matches('/');
    // containers = dossiers
    for (attrs, _content) in tag_blocks(didl, "container") {
        let id = attr(&attrs, "id").unwrap_or("0").to_string();
        // on parse le titre proprement dans le bloc:
        let block = find_block(didl, "container", &attrs);
        let title = block
            .and_then(|b| tag_content(b, "dc:title"))
            .unwrap_or_else(|| "Sans titre".into());
        out.push(BrowseEntry {
            name: title,
            uri: format!("{prefix}/{id}"),
            is_dir: true,
            size: None,
            format: None,
            is_audio: false,
        });
    }
    for (attrs, _content) in tag_blocks(didl, "item") {
        let id = attr(&attrs, "id").unwrap_or("").to_string();
        let block = find_block(didl, "item", &attrs);
        let block = block.unwrap_or("");
        let title = tag_content(block, "dc:title").unwrap_or_else(|| "Sans titre".into());
        let class = tag_content(block, "upnp:class").unwrap_or_default();
        let res = tag_blocks(block, "res").into_iter().next();
        let (url, size) = match res {
            Some((rattrs, rcontent)) => (
                absolutize(base_url, rcontent.trim()),
                attr(&rattrs, "size").and_then(|s| s.parse::<u64>().ok()),
            ),
            None => (String::new(), None),
        };
        if url.is_empty() {
            continue;
        }
        let is_audio =
            class.contains("audioItem") || class.contains("musicTrack") || class.is_empty();
        out.push(BrowseEntry {
            name: title,
            uri: url,
            is_dir: false,
            size,
            format: None,
            is_audio,
        });
        let _ = id;
    }
    out.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
    Ok(out)
}

/// Retrouve le bloc complet d'un tag dont on connait la partie attributs.
fn find_block<'a>(xml: &'a str, tag: &str, attrs: &str) -> Option<&'a str> {
    // Recherche simple: on localise "<tag" ou "<prefix:tag" suivi des memes attributs
    let mut search_from = 0;
    let needle_open = format!(":{}", tag);
    while let Some(rel) = xml[search_from..].find('<') {
        let pos = search_from + rel;
        let rest = &xml[pos..];
        let Some(close) = rest.find('>') else { break };
        let tagpart = &rest[1..close];
        let name = tagpart.split_whitespace().next().unwrap_or("");
        if (name == tag || name.ends_with(&needle_open))
            && tagpart.contains(attrs.trim_start_matches(name).trim())
        {
            let content_start = pos + close + 1;
            let close_tag = format!("</{}>", name);
            let end = xml[content_start..]
                .find(&close_tag)
                .map(|e| content_start + e + close_tag.len())?;
            return Some(&xml[pos..end]);
        }
        search_from = pos + 1;
    }
    None
}

/// Parse un URI `upnp://<location>/<object_id>` en (location, object_id).
/// La location peut contenir un scheme (`http://host:port/desc.xml`) : on coupe
/// au premier '/' *après* le scheme, pas au premier '/' absolu.
pub fn parse_upnp_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("upnp://")?;
    let cut = match rest.find("://") {
        Some(s) => {
            let after_scheme = &rest[s + 3..];
            s + 3 + after_scheme.find('/')?
        }
        None => rest.find('/')?,
    };
    let (loc, id) = rest.split_at(cut);
    Some((loc.to_string(), id[1..].to_string()))
}

/// Backend d'une source UPnP configurée manuellement (`kind = "upnp"`,
/// `url` = URL du document de description du serveur). Utile quand la
/// découverte SSDP est bloquée (multicast filtré) : on branche l'URL à la main.
///
/// Le listage navigue dans le ContentDirectory avec l'object_id comme chemin
/// relatif (`src://<id>/0/3/...`) ; la lecture des items passe par l'URL HTTP
/// de leur <res> (seekable, via HttpSource).
pub struct UpnpSource {
    source: Source,
    url: String,
    http: HttpSource,
    dev: Mutex<Option<UpnpDevice>>,
}

impl UpnpSource {
    pub fn new(source: Source) -> Result<Self> {
        let url = match &source.kind {
            SourceKind::Upnp { url } => url.clone(),
            _ => {
                return Err(AurisError::other(
                    "UpnpSource attend une source kind=upnp".to_string(),
                ))
            }
        };
        if url.trim().is_empty() {
            return Err(AurisError::other(
                "source upnp: url de description vide".to_string(),
            ));
        }
        let http_src = Source {
            id: "__upnp_http__".into(),
            name: "HTTP direct".into(),
            kind: SourceKind::Http { base_url: None },
            enabled: true,
        };
        Ok(UpnpSource {
            source,
            url,
            http: HttpSource::new(http_src)?,
            dev: Mutex::new(None),
        })
    }

    /// Device UPnP (mis en cache ; re-téléchargé si le cache est vide).
    fn device(&self) -> Result<UpnpDevice> {
        if let Some(d) = &*self.dev.lock().unwrap() {
            return Ok(d.clone());
        }
        self.refresh()
    }

    /// Re-télécharge le document de description et remet le cache à jour.
    pub fn refresh(&self) -> Result<UpnpDevice> {
        let d = fetch_device(self.url.trim())?;
        *self.dev.lock().unwrap() = Some(d.clone());
        Ok(d)
    }

    fn object_id(rel_path: &str) -> &str {
        let t = rel_path.trim_matches('/');
        if t.is_empty() {
            "0"
        } else {
            t
        }
    }
}

#[async_trait]
impl SourceBackend for UpnpSource {
    fn source(&self) -> &Source {
        &self.source
    }

    async fn list(&self, rel_path: &str) -> Result<Vec<BrowseEntry>> {
        let dev = self.device()?;
        let didl = soap_browse(&dev, Self::object_id(rel_path), "BrowseDirectChildren")?;
        parse_didl(&format!("src://{}/", self.source.id), &dev.base_url, &didl)
    }

    async fn open(&self, rel_path: &str) -> Result<Box<dyn SeekRead>> {
        let dev = self.device()?;
        let entry = stat_with_prefix(
            &format!("src://{}/", self.source.id),
            &dev,
            Self::object_id(rel_path),
        )?;
        if entry.is_dir {
            return Err(AurisError::other(format!(
                "UPnP: « {} » est un dossier",
                entry.name
            )));
        }
        let f = self.http.open_url(&entry.uri)?;
        if f.size() == 0 {
            return Err(AurisError::other(format!(
                "UPnP: taille inconnue pour {}",
                entry.uri
            )));
        }
        Ok(Box::new(f))
    }

    async fn size(&self, rel_path: &str) -> Result<Option<u64>> {
        let dev = self.device()?;
        let entry = stat_with_prefix(
            &format!("src://{}/", self.source.id),
            &dev,
            Self::object_id(rel_path),
        )?;
        Ok(entry.size)
    }

    async fn stat(&self, rel_path: &str) -> Result<Option<BrowseEntry>> {
        let dev = self.device()?;
        let entry = stat_with_prefix(
            &format!("src://{}/", self.source.id),
            &dev,
            Self::object_id(rel_path),
        )?;
        Ok(Some(entry))
    }
}
