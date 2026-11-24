//! Test de la source HTTP (reqwest) contre un mini serveur local.

use std::io::Read;

use auris_core::{Source, SourceKind};
use auris_sources::http::HttpSource;

async fn spawn_file_server(
    body: &'static [u8],
    content_type: &'static str,
) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread")] // open_url est bloquant (worker dedie)
async fn http_source_open_et_playlist() {
    let wav: &'static [u8] = b"RIFF....WAVEfmt fake-wav-bytes";
    let addr = spawn_file_server(wav, "audio/wav").await;
    let base = format!("http://{addr}");

    let src = Source {
        id: "http1".into(),
        name: "Radio".into(),
        kind: SourceKind::Http {
            base_url: Some(base.clone()),
        },
        enabled: true,
    };
    let http = HttpSource::new(src).unwrap();

    // open_url: lecture complete du fichier servi
    let mut f = http.open_url(&format!("{base}/t1.wav")).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    assert_eq!(&buf[..], wav);

    // list_url sur un fichier direct -> une entrée audio
    let entries = http.list_url(&format!("{base}/t1.wav")).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "t1.wav");
    assert!(entries[0].is_audio);

    // list_url sur une playlist m3u
    let m3u: &'static [u8] =
        b"#EXTM3U\n#EXTINF:1,Piste 1\nhttp://example.com/a.flac\nhttp://example.com/b.flac\n";
    let addr2 = spawn_file_server(m3u, "audio/mpegurl").await;
    let entries = http
        .list_url(&format!("http://{addr2}/list.m3u"))
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .all(|e| e.is_audio && e.uri.ends_with(".flac")));
}
