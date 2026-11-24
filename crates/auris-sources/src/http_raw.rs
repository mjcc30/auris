//! Mini client HTTP/1.1 synchrone sur TcpStream (LAN uniquement, http://).
//! Supporte Content-Length, chunked, et l'en-tete Range (lectures par plages).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use auris_core::{AurisError, Result};

#[derive(Debug, Clone)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    pub fn parse(raw: &str) -> Result<Url> {
        let rest = raw
            .strip_prefix("http://")
            .ok_or_else(|| AurisError::InvalidUri(raw.to_string()))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| AurisError::InvalidUri(raw.into()))?,
            ),
            None => (authority.to_string(), 80),
        };
        Ok(Url {
            host,
            port,
            path: path.to_string(),
        })
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_response(stream: &mut TcpStream, method: &str) -> Result<HttpResponse> {
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    // Lire l'en-tete
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(AurisError::Network(
                "connexion fermee pendant l'en-tete HTTP".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 4 * 1024 * 1024 {
            return Err(AurisError::Network("en-tete HTTP trop gros".into()));
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let resp_head = HttpResponse {
        status,
        headers,
        body: Vec::new(),
    };
    let is_head = method == "HEAD";
    let chunked = resp_head
        .header("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let content_length: Option<usize> = resp_head
        .header("content-length")
        .and_then(|v| v.parse().ok());

    let mut body = buf[header_end..].to_vec();
    if is_head {
        return Ok(HttpResponse {
            status: resp_head.status,
            headers: resp_head.headers,
            body: Vec::new(),
        });
    }
    if chunked {
        // Decodage chunked: on accumule tout puis on decode.
        loop {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
            if looks_complete_chunked(&body) {
                break;
            }
        }
        let decoded = decode_chunked(&body)?;
        return Ok(HttpResponse {
            status: resp_head.status,
            headers: resp_head.headers,
            body: decoded,
        });
    }
    if let Some(cl) = content_length {
        while body.len() < cl {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(cl);
    } else {
        // Lire jusqu'a la fermeture.
        loop {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    Ok(HttpResponse {
        status: resp_head.status,
        headers: resp_head.headers,
        body,
    })
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn looks_complete_chunked(body: &[u8]) -> bool {
    // Termine par un chunk de taille 0: "0\r\n\r\n" (eventuellement suivi de trailers).
    let tail = &body[body.len().saturating_sub(16)..];
    find_subsequence(tail, b"0\r\n\r\n").is_some()
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        // Lire la ligne de taille
        let eol = find_subsequence(body, b"\r\n")
            .ok_or_else(|| AurisError::Network("chunk invalide".into()))?;
        let size_str = std::str::from_utf8(&body[..eol])
            .map_err(|_| AurisError::Network("chunk invalide utf8".into()))?;
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| AurisError::Network("taille chunk invalide".into()))?;
        body = &body[eol + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size + 2 {
            return Err(AurisError::Network("chunk tronque".into()));
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
    Ok(out)
}

fn send_request(
    url: &Url,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let addr = (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| AurisError::Network(format!("resolution echouee: {}", url.host)))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        method, url.path, url.host, url.port
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    if let Some(b) = body {
        stream.write_all(b)?;
    }
    stream.flush()?;
    read_response(&mut stream, method)
}

/// GET simple. `range` = Some((start, end_inclusive)) pour une lecture par plage.
pub fn http_get(
    url: &str,
    extra_headers: &[(&str, &str)],
    range: Option<(u64, u64)>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let u = Url::parse(url)?;
    let mut headers: Vec<(String, String)> = vec![
        ("User-Agent".into(), "auris/0.1".into()),
        ("Accept".into(), "*/*".into()),
    ];
    headers.extend(
        extra_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    if let Some((s, e)) = range {
        headers.push(("Range".into(), format!("bytes={}-{}", s, e)));
    }
    send_request(&u, "GET", &headers, None, timeout)
}

/// POST SOAP (UPnP).
pub fn http_post(
    url: &str,
    content_type: &str,
    soap_action: &str,
    body: &str,
    timeout: Duration,
) -> Result<HttpResponse> {
    let u = Url::parse(url)?;
    let headers = vec![
        ("User-Agent".to_string(), "auris/0.1".to_string()),
        ("Content-Type".to_string(), content_type.to_string()),
        ("SOAPAction".to_string(), soap_action.to_string()),
    ];
    send_request(&u, "POST", &headers, Some(body.as_bytes()), timeout)
}
