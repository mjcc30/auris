//! Test du mini client HTTP synchrone (http_raw) contre un serveur local.

use std::time::Duration;

use auris_sources::http_raw::http_get;

#[tokio::test]
async fn http_get_contre_serveur_local() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let body = b"bonjour";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            });
        }
    });

    let url = format!("http://{addr}/music/fichier.flac");
    let r = tokio::task::spawn_blocking(move || http_get(&url, &[], None, Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/plain")); // insensible a la casse
    assert_eq!(r.header("CONTENT-LENGTH"), Some("7"));
    assert_eq!(&r.body, b"bonjour");
}

#[test]
fn http_get_connexion_refusee() {
    // port 1: rien n'ecoute -> erreur propre, pas de panic
    let r = http_get("http://127.0.0.1:1/x", &[], None, Duration::from_secs(2));
    assert!(r.is_err());
}

#[test]
fn http_get_url_invalide() {
    assert!(http_get("pas://url", &[], None, Duration::from_secs(1)).is_err());
}
