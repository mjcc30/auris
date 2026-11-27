//! Tests d'intégration: crédits étendus (#73) — tags lus au scan, exposés sur
//! `GET /api/library/tracks/{id}` (paroles incluses), allégés sur la liste,
//! et présents dans `GET /api/infos/{track_id}`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use auris_core::{Source, Track};
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

async fn spawn_server() -> (String, Arc<Library>, Source) {
    let root = unique_tmp("credits");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 1);
    let src = Source::new_local("local1", music.to_str().unwrap());
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
    let state = AppState::new(engine, library.clone(), sources, updater, root.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), library, src)
}

#[tokio::test]
async fn credits_etendus_exposes_sur_track_et_infos() {
    let (base, library, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // scan du wav pour indexer la piste (probe réel ensuite possible)
    let r = http
        .post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({ "source_id": src.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut tid = String::new();
    for _ in 0..200 {
        let tracks: serde_json::Value = http
            .get(format!("{base}/api/library/tracks"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(t0) = tracks.as_array().unwrap().first() {
            tid = t0["id"].as_str().unwrap().to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!tid.is_empty(), "piste non indexée");

    // injecte des crédits sur la fiche piste en base (simule un scan de tags)
    {
        let mut t: Track = library.tracks().into_iter().find(|t| t.id == tid).unwrap();
        t.composer = Some("Max Richter".into());
        t.producer = Some("DG".into());
        t.label = Some("Deutsche Grammophon".into());
        t.publisher = Some("DG".into());
        t.isrc = Some("DEUM71800001".into());
        t.copyright = Some("© 2018".into());
        t.lyrics = Some("[00:01.00] ligne 1".into());
        library.upsert_track(&t).unwrap();
    }

    // 1. route individuelle: tous les crédits + paroles embarquées
    let one: serde_json::Value = http
        .get(format!("{base}/api/library/tracks/{tid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["composer"], "Max Richter");
    assert_eq!(one["producer"], "DG");
    assert_eq!(one["label"], "Deutsche Grammophon");
    assert_eq!(one["publisher"], "DG");
    assert_eq!(one["isrc"], "DEUM71800001");
    assert_eq!(one["copyright"], "© 2018");
    assert_eq!(one["lyrics"], "[00:01.00] ligne 1");

    // 2. la liste allège les paroles (champ absent), garde les crédits
    let list: serde_json::Value = http
        .get(format!("{base}/api/library/tracks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let l0 = &list.as_array().unwrap()[0];
    assert!(
        l0.get("lyrics").is_none(),
        "paroles doivent être absentes de la liste"
    );
    assert_eq!(l0["composer"], "Max Richter");

    // 3. /api/infos/{id} expose les crédits (probe réel du wav + fiche en base)
    let infos: serde_json::Value = http
        .get(format!("{base}/api/infos/{tid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(infos["composer"], "Max Richter");
    assert_eq!(infos["isrc"], "DEUM71800001");
    assert_eq!(infos["lyrics_embedded"], true);
    assert!(infos["album_info"].is_object());

    // 4. track inconnu -> erreur JSON
    let r = http
        .get(format!("{base}/api/library/tracks/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert!(r.status() == 400 || r.status() == 404);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body.get("error").is_some());
}
