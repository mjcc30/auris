//! Test d'integration de l'API web: demarre le routeur axum sur un port
//! ephemere et expose les endpoints REST (state, sources, browse, scan,
//! bibliothèque, player, system, UI embarquee).

use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState};

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

fn unique_tmp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("auris-{tag}-{}-{nanos}", std::process::id()))
}

async fn spawn_server() -> (String, std::path::PathBuf, Source) {
    let root = unique_tmp("webapi");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 2);

    let src = Source::new_local("local1", music.to_str().unwrap());
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
    let state = AppState {
        engine,
        library,
        sources,
        updater,
        started_at: chrono::Utc::now().timestamp(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), root, src)
}

#[tokio::test]
async fn api_rest_complet() {
    let (base, root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // UI embarquee
    let html = http
        .get(format!("{base}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("id=\"root\""), "index UI embarquee: {html}");

    // state initial
    let st: serde_json::Value = http
        .get(format!("{base}/api/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["status"], "stopped");
    assert_eq!(st["volume"], 80);

    // system info
    let info: serde_json::Value = http
        .get(format!("{base}/api/system/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["name"], "Auris");
    assert_eq!(info["source_count"], 1);

    // sources: liste + ajout
    let srcs: serde_json::Value = http
        .get(format!("{base}/api/sources"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(srcs.as_array().unwrap().len(), 1);
    let added: serde_json::Value = http
        .post(format!("{base}/api/sources"))
        .json(&serde_json::json!({"id":"musique2","name":"Musique 2","enabled":true,"kind":"local","path":root.join("music").to_str().unwrap()}))
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(added["id"], "musique2");
    let srcs: serde_json::Value = http
        .get(format!("{base}/api/sources"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(srcs.as_array().unwrap().len(), 2);

    // browse de la racine de la source
    let entries: serde_json::Value = http
        .get(format!("{base}/api/browse"))
        .query(&[("uri", format!("src://{}/", src.id))])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = entries.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "t1.wav");
    assert_eq!(arr[0]["is_audio"], true);

    // scan + bibliothèque
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
    let tracks: serde_json::Value = http
        .get(format!("{base}/api/library/tracks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tracks.as_array().unwrap().len(), 1);
    assert_eq!(tracks[0]["format"]["kind"], "pcm");
    let found: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "t1")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found.as_array().unwrap().len(), 1);
    let albums: serde_json::Value = http
        .get(format!("{base}/api/library/albums"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(albums.as_array().unwrap().len(), 1);

    // player: volume / repeat / shuffle / mute (pas de lecture: pas de DAC en CI)
    let st: serde_json::Value = http
        .post(format!("{base}/api/player/volume"))
        .json(&serde_json::json!({"volume": 30}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["volume"], 30);
    let st: serde_json::Value = http
        .post(format!("{base}/api/player/repeat"))
        .json(&serde_json::json!({"mode": "all"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["repeat"], "all");
    let st: serde_json::Value = http
        .post(format!("{base}/api/player/shuffle"))
        .json(&serde_json::json!({"enabled": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["shuffle"], true);
    let st: serde_json::Value = http
        .post(format!("{base}/api/player/mute"))
        .json(&serde_json::json!({"muted": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["muted"], true);
    // seek sans lecture -> 200 (no-op)
    http.post(format!("{base}/api/player/seek"))
        .json(&serde_json::json!({"secs": 1.0}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // peripheriques audio (liste, peut etre vide en CI)
    let devs = http
        .get(format!("{base}/api/audio/devices"))
        .send()
        .await
        .unwrap();
    assert!(devs.status().is_success());
    let devs: serde_json::Value = devs.json().await.unwrap();
    assert!(devs.is_array());

    // file d'attente + commandes player (asynchrones -> on scrute l'etat)
    let tid = tracks[0]["id"].as_str().unwrap().to_string();
    http.post(format!("{base}/api/player/play"))
        .json(&serde_json::json!({"queue": tracks, "index": 0}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut qlen = 0usize;
    for _ in 0..100 {
        let q: serde_json::Value = http
            .get(format!("{base}/api/queue"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        qlen = q.as_array().map(|a| a.len()).unwrap_or(0);
        if qlen == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(qlen, 1, "la queue doit contenir le titre");
    for ep in ["stop", "pause", "resume", "next", "prev"] {
        http.post(format!("{base}/api/player/{ep}"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    // lecture par track_id (sans DAC en CI la lecture echoue mais le POST repond 200)
    http.post(format!("{base}/api/player/play"))
        .json(&serde_json::json!({"track_id": tid}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // track_id inconnu -> 400 avec erreur JSON
    let r = http
        .post(format!("{base}/api/player/play"))
        .json(&serde_json::json!({"track_id": "inconnu"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    // clear queue
    http.post(format!("{base}/api/queue/clear"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // selection de peripherique (null = defaut)
    http.post(format!("{base}/api/audio/device"))
        .json(&serde_json::json!({"name": null}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // cover inconnu -> 404
    let r = http
        .get(format!("{base}/api/library/cover/inconnu"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);

    // update: pas de release publiee -> erreur JSON propre (400), pas de panic
    let r = http
        .get(format!("{base}/api/system/update"))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error() || r.status().is_success());

    // erreur propre sur URI invalide
    let r = http
        .get(format!("{base}/api/browse"))
        .query(&[("uri", "nimportequoi")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["error"].is_string());

    let _ = std::fs::remove_dir_all(&root);
}
