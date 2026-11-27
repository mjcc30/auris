//! Test d'intégration UPnP/DLNA contre un mock de MediaServer (HTTP local) :
//! document de description, Browse SOAP (DirectChildren + Metadata), <res>.

use auris_core::{Source, SourceKind};
use auris_sources::upnp::{self, UpnpDevice};

const DESC_XML: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
<URLBase>http://127.0.0.1:PORT/</URLBase>
<device>
<friendlyName>MockNAS</friendlyName>
<service>
<serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
<controlURL>/control</controlURL>
</service>
</device>
</root>"#;

/// DIDL d'un container racine : 1 dossier + 1 morceau.
fn didl_children_root() -> String {
    r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<container id="0/1" parentID="0" restricted="1"><dc:title>Musique</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>
<item id="0/2" parentID="0" restricted="1"><dc:title>Morceau test</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class><res size="12345" duration="0:03:00">/music/track1.flac</res></item>
</DIDL-Lite>"#
        .to_string()
}

fn didl_children_musique() -> String {
    r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<item id="0/1/7" parentID="0/1" restricted="1"><dc:title>Piste A</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class><res size="999">/music/a.flac</res></item>
</DIDL-Lite>"#
        .to_string()
}

fn didl_metadata(object_id: &str) -> String {
    if object_id == "0/1" {
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<container id="0/1" parentID="0" restricted="1"><dc:title>Musique</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>
</DIDL-Lite>"#
    } else {
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<item id="0/2" parentID="0" restricted="1"><dc:title>Morceau test</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class><res size="12345">/music/track1.flac</res></item>
</DIDL-Lite>"#
    }
    .to_string()
}

fn soap_envelope(didl: &str) -> String {
    let escaped = didl
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><u:BrowseResponse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">
<Result>{escaped}</Result><NumberReturned>1</NumberReturned><TotalMatches>1</TotalMatches><UpdateID>0</UpdateID>
</u:BrowseResponse></s:Body></s:Envelope>"#
    )
}

/// Lance le mock MediaServer sur 127.0.0.1 (port éphémère). Retourne l'URL de
/// la description.
async fn spawn_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Lire la requête complète : headers + body (Content-Length).
                // Un seul read() ne suffit pas — selon l'OS, le corps SOAP peut
                // arriver dans un second segment TCP.
                let mut buf = Vec::with_capacity(65536);
                let req = loop {
                    let mut chunk = [0u8; 65536];
                    let n = match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break String::new(),
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    let req = String::from_utf8_lossy(&buf).to_string();
                    if let Some(pos) = req.find("\r\n\r\n") {
                        let head = &req[..pos];
                        let want = head
                            .lines()
                            .find_map(|l| {
                                let (name, val) = l.split_once(':')?;
                                if name.trim().eq_ignore_ascii_case("content-length") {
                                    val.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        if req.len() - (pos + 4) >= want {
                            break req;
                        }
                    }
                };
                let (body, is_post) = if let Some(pos) = req.find("\r\n\r\n") {
                    (req[pos + 4..].to_string(), req.starts_with("POST"))
                } else {
                    (String::new(), false)
                };
                let (status, content_type, payload, extra): (&str, &str, Vec<u8>, String) = if req
                    .starts_with("GET /desc.xml")
                {
                    let xml = DESC_XML.replace("PORT", &addr.port().to_string());
                    ("200 OK", "text/xml", xml.into_bytes(), String::new())
                } else if req.starts_with("GET /music/") {
                    let total = 12345u64;
                    let data = vec![0xAB_u8; total as usize];
                    // Honorer Range: bytes=start-end -> 206 + Content-Range
                    let range = req.lines().find_map(|l| {
                        let (name, val) = l.split_once(':')?;
                        if !name.trim().eq_ignore_ascii_case("range") {
                            return None;
                        }
                        let l = val.trim().strip_prefix("bytes=")?;
                        let (a, b) = l.split_once('-')?;
                        Some((a.parse::<u64>().ok()?, b.parse::<u64>().unwrap_or(u64::MAX)))
                    });
                    match range {
                        Some((start, end)) => {
                            let end = end.min(total - 1);
                            let start = start.min(total - 1);
                            let slice = data[start as usize..=end as usize].to_vec();
                            (
                                "206 Partial Content",
                                "audio/flac",
                                slice,
                                format!("Content-Range: bytes {start}-{end}/{total}\r\n"),
                            )
                        }
                        None => ("200 OK", "audio/flac", data, String::new()),
                    }
                } else if is_post && req.contains("/control") {
                    let object_id = extract_tag(&body, "ObjectID").unwrap_or_else(|| "0".into());
                    let flag = extract_tag(&body, "BrowseFlag")
                        .unwrap_or_else(|| "BrowseDirectChildren".into());
                    let didl = if flag == "BrowseMetadata" {
                        didl_metadata(&object_id)
                    } else if object_id == "0/1" {
                        didl_children_musique()
                    } else {
                        didl_children_root()
                    };
                    (
                        "200 OK",
                        "text/xml",
                        soap_envelope(&didl).into_bytes(),
                        String::new(),
                    )
                } else {
                    (
                        "404 Not Found",
                        "text/plain",
                        b"nope".to_vec(),
                        String::new(),
                    )
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&payload).await;
            });
        }
    });
    format!("http://{addr}/desc.xml")
}

fn extract_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

fn mock_device(location: &str) -> UpnpDevice {
    let base = location.trim_end_matches("/desc.xml").to_string();
    UpnpDevice {
        friendly_name: "MockNAS".into(),
        location: location.to_string(),
        base_url: format!("{base}/"),
        content_directory_control: format!("{base}/control"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upnp_browse_et_stat_contre_mock() {
    let location = spawn_mock().await;
    let dev = mock_device(&location);
    let base = location.trim_end_matches("/desc.xml");

    // Browse racine : 1 dossier (upnp://...) + 1 morceau (URL HTTP absolue).
    let entries = tokio::task::spawn_blocking({
        let dev = dev.clone();
        move || upnp::browse(&dev, "0")
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(entries.len(), 2);
    let dir = entries.iter().find(|e| e.is_dir).unwrap();
    assert_eq!(dir.name, "Musique");
    assert_eq!(dir.uri, format!("upnp://{location}/0/1"));
    let item = entries.iter().find(|e| !e.is_dir).unwrap();
    assert_eq!(item.name, "Morceau test");
    assert_eq!(item.uri, format!("{base}/music/track1.flac"));
    assert_eq!(item.size, Some(12345));
    assert!(item.is_audio);

    // Browse d'un sous-dossier.
    let sub = tokio::task::spawn_blocking({
        let dev = dev.clone();
        move || upnp::browse(&dev, "0/1")
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].uri, format!("{base}/music/a.flac"));

    // stat (BrowseMetadata) : container puis item.
    let st_dir = tokio::task::spawn_blocking({
        let dev = dev.clone();
        move || upnp::stat(&dev, "0/1")
    })
    .await
    .unwrap()
    .unwrap();
    assert!(st_dir.is_dir);
    assert_eq!(st_dir.uri, format!("upnp://{location}/0/1"));
    let st_item = tokio::task::spawn_blocking({
        let dev = dev.clone();
        move || upnp::stat(&dev, "0/2")
    })
    .await
    .unwrap()
    .unwrap();
    assert!(!st_item.is_dir);
    assert!(st_item.is_audio);
    assert_eq!(st_item.size, Some(12345));
}

#[tokio::test(flavor = "multi_thread")]
async fn upnp_source_configuree_via_manager() {
    let location = spawn_mock().await;
    let src = Source {
        id: "mocknas".into(),
        name: "MockNAS".into(),
        kind: SourceKind::Upnp {
            url: location.clone(),
        },
        enabled: true,
    };
    let mgr = auris_sources::SourcesManager::new(vec![src]).unwrap();

    // list racine via src://mocknas/ -> containers en src://mocknas/<object_id>
    let root = mgr.browse("src://mocknas/").await.unwrap();
    assert_eq!(root.len(), 2);
    let dir = root.iter().find(|e| e.is_dir).unwrap();
    assert_eq!(dir.uri, "src://mocknas/0/1");

    // list du sous-dossier + stat par le parent + walk_audio récursif
    let sub = mgr.browse("src://mocknas/0/1").await.unwrap();
    assert_eq!(sub.len(), 1);
    assert!(sub[0].is_audio);
    let st = mgr.stat("src://mocknas/0/1").await.unwrap();
    assert!(st.is_dir);
    let all = mgr.walk_audio("src://mocknas/").await.unwrap();
    assert_eq!(all.len(), 2); // Morceau test + Piste A

    // size + open d'un item (lecture HTTP du <res>)
    let backend = mgr.backend("mocknas").unwrap();
    let size = backend.size("0/2").await.unwrap();
    assert_eq!(size, Some(12345));
    let mut reader = backend.open("0/2").await.unwrap();
    let mut buf = Vec::new();
    use std::io::Read;
    reader.read_to_end(&mut buf).unwrap();
    assert_eq!(buf.len(), 12345);
    assert!(buf.iter().all(|b| *b == 0xAB));

    // stat d'un item UPnP via manager (match par nom dans le parent)
    let st_item = mgr.stat("src://mocknas/0/2").await.unwrap();
    assert!(!st_item.is_dir);
    assert!(st_item.is_audio);
}
