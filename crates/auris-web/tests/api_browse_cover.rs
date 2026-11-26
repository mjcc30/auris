//! Tests d'intégration: pochette de dossier dans Parcourir (/api/browse/cover)
//! et endpoint artiste détaillé (/api/library/artists/{id} avec albums + titres).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState};

fn unique_tmp(tag: &str) -> std::path::PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("auris-{tag}-{}-{nanos}-{n}", std::process::id()))
}

fn write_test_wav(path: &std::path::Path, secs: u32) {
    let rate = 44100u32;
    let frames = rate * secs;
    let data_len = frames * 2 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v =
            ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 8000.0) as i16;
        w.extend_from_slice(&v.to_le_bytes());
        w.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, w).unwrap();
}

/// Arborescence: music/1970/Black Sabbath/{cover.jpg, 01.wav},
/// music/1970/AC-DC/Folder.PNG, music/1970/Deep Purple/02.wav (pas d'image).
async fn spawn_server() -> (String, std::path::PathBuf, Source) {
    let root = unique_tmp("webcover");
    let sab = root.join("music/1970/Black Sabbath");
    let acdc = root.join("music/1970/AC-DC");
    let dp = root.join("music/1970/Deep Purple");
    for d in [&sab, &acdc, &dp] {
        std::fs::create_dir_all(d).unwrap();
    }
    // fausse image JPEG (octets magiques) et PNG
    let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpg.extend_from_slice(&[0u8; 64]);
    std::fs::write(sab.join("cover.jpg"), &jpg).unwrap();
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&[0u8; 64]);
    std::fs::write(acdc.join("Folder.PNG"), &png).unwrap();
    write_test_wav(&sab.join("01 War Pigs.wav"), 1);
    write_test_wav(&acdc.join("01 Hells Bells.wav"), 1);
    write_test_wav(&dp.join("02 Smoke.wav"), 1);

    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let library = Arc::new(Library::open(&root.join("db"), sources.clone()).unwrap());
    let engine = PlayerEngine::start(sources.clone(), None, 80, false);
    let updater = Arc::new(Updater::new(UpdaterConfig {
        repo_owner: "mjcc30".into(),
        repo_name: "auris".into(),
        bin_name: "auris".into(),
        current_version: "0.1.0".into(),
        auto_check: false,
    }));
    let state = AppState::new(engine, library, sources, updater, root.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), root, src)
}

#[tokio::test]
async fn browse_cover_fichier_image_prioritaire() {
    let (base, _root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // cover.jpg -> image/jpeg avec les octets magiques
    let uri = format!("src://{}/1970/Black Sabbath", src.id);
    let resp = http
        .get(format!("{base}/api/browse/cover"))
        .query(&[("uri", uri.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("content-type").unwrap(), "image/jpeg");
    let bytes = resp.bytes().await.unwrap();
    assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));

    // Folder.PNG (majuscules) -> image/png détecté par les octets magiques
    let uri = format!("src://{}/1970/AC-DC", src.id);
    let resp = http
        .get(format!("{base}/api/browse/cover"))
        .query(&[("uri", uri.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("content-type").unwrap(), "image/png");
}

#[tokio::test]
async fn browse_cover_404_sans_image_ni_audio_embarque() {
    let (base, _root, src) = spawn_server().await;
    let http = reqwest::Client::new();
    // Deep Purple: un wav sans pochette embarquee -> 404
    let uri = format!("src://{}/1970/Deep Purple", src.id);
    let resp = http
        .get(format!("{base}/api/browse/cover"))
        .query(&[("uri", uri.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    // racine: que des sous-dossiers -> 404 aussi
    let uri = format!("src://{}/", src.id);
    let resp = http
        .get(format!("{base}/api/browse/cover"))
        .query(&[("uri", uri.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn artist_detail_avec_albums_et_tracks() {
    let (base, _root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // scan (asynchrone: on attend la fin via le statut)
    http.post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({"source_id": src.id}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/library/scan/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let artists: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/artists"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(artists.len(), 1, "un seul artiste (inconnu): {artists:?}");
    let id = artists[0]["id"].as_str().unwrap().to_string();

    let detail: serde_json::Value = http
        .get(format!("{base}/api/library/artists/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["artist"]["id"], id);
    assert_eq!(
        detail["tracks"].as_array().unwrap().len(),
        3,
        "les 3 titres scannés: {detail}"
    );
    assert_eq!(detail["albums"].as_array().unwrap().len(), 1);
    assert!(detail["cover_url"].is_string() || detail["cover_url"].is_null());

    // artiste inconnu -> 400 avec erreur JSON
    let resp = http
        .get(format!("{base}/api/library/artists/inexistant"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
