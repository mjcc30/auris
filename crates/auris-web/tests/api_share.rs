//! Test d'intégration: partage de la piste EN COURS de lecture.
//! Regression: jouer par URI (Parcourir) donnait à `state.current` un id
//! aléatoire (Track::new -> Uuid v4) -> POST /api/shares renvoyait 400
//! « piste introuvable ». Apres correctif, la lecture par URI adopte la piste
//! de la bibliothèque (by_uri) et le partage fonctionne.

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

async fn spawn_server() -> (String, String) {
    let root = unique_tmp("share");
    let dir = root.join("music/1970/Test");
    std::fs::create_dir_all(&dir).unwrap();
    write_test_wav(&dir.join("01 Test.wav"), 1);

    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let library = Arc::new(Library::open(&root.join("db"), sources.clone()).unwrap());
    let engine = PlayerEngine::start(sources.clone(), None, 80);
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
    (format!("http://{addr}"), src.id)
}

async fn wait_scan_done(http: &reqwest::Client, base: &str, source_id: &str) {
    http.post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({"source_id": source_id}))
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
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("scan jamais termine");
}

#[tokio::test]
async fn partager_la_piste_en_cours_lue_par_uri() {
    let (base, src_id) = spawn_server().await;
    let http = reqwest::Client::new();
    wait_scan_done(&http, &base, &src_id).await;

    // la piste scannée a un vrai id de bibliothèque
    let tracks: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tracks.len(), 1);
    let lib_id = tracks[0]["id"].as_str().unwrap().to_string();
    let uri = tracks[0]["uri"].as_str().unwrap().to_string();

    // mode navigateur pour ne pas dependre d'un périphérique audio de test
    http.post(format!("{base}/api/player/output"))
        .json(&serde_json::json!({"mode": "browser"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // lecture par URI, comme depuis la page Parcourir
    http.post(format!("{base}/api/player/play"))
        .json(&serde_json::json!({"uri": uri}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // attendre que current soit renseigne
    let mut current_id = String::new();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(c) = st["current"].as_object() {
            current_id = c["id"].as_str().unwrap().to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!current_id.is_empty(), "current jamais renseigne");
    assert_eq!(
        current_id, lib_id,
        "la piste en cours doit garder l'id de la bibliothèque (by_uri)"
    );

    // partage de la piste en cours -> 201 (avant: 400 « piste introuvable »)
    let resp = http
        .post(format!("{base}/api/shares"))
        .json(&serde_json::json!({"kind": "track", "id": current_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "{resp:?}");
    let share: serde_json::Value = resp.json().await.unwrap();
    let token = share["token"].as_str().unwrap().to_string();
    assert_eq!(share["title"], "01 Test.wav");

    // la page publique et le flux répondent
    let page = http
        .get(format!("{base}/share/{token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let stream = http
        .get(format!("{base}/share/{token}/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    let bytes = stream.bytes().await.unwrap();
    assert!(bytes.starts_with(b"RIFF"));
}
