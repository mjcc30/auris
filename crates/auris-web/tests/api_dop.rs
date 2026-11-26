//! Tests d'intégration: endpoints DoP/WASAPI exposés à l'UI
//! (`/api/audio/dop-status`, `/api/audio/dop-setup`, `/api/audio/restart-audio`).
//!
//! Aucun test ne touche au matériel de façon destructive : `dop-status` est en
//! lecture seule, `dop-setup` est appelé avec un nom de device inconnu (erreur
//! propre, aucune écriture), et `restart-audio` n'est appelé que hors Windows
//! (sous Windows il déclenche une invite UAC).

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

async fn spawn_server() -> String {
    let root = unique_tmp("dop");
    std::fs::create_dir_all(root.join("music")).unwrap();
    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
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
    format!("http://{addr}")
}

#[tokio::test]
async fn dop_status_forme() {
    let base = spawn_server().await;
    let http = reqwest::Client::new();
    let st: serde_json::Value = http
        .get(format!("{base}/api/audio/dop-status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["windows"], cfg!(target_os = "windows"));
    assert!(st["devices"].is_array());
    assert!(st["current_device"].is_null() || st["current_device"].is_string());
    for d in st["devices"].as_array().unwrap() {
        assert!(d["name"].is_string());
        assert!(d["mix"].is_null() || d["mix"]["rate"].is_u64());
        assert!(d["default_format"].is_null() || d["default_format"]["rate"].is_u64());
    }
}

#[tokio::test]
async fn dop_setup_device_inconnu_erreur_propre() {
    let base = spawn_server().await;
    let http = reqwest::Client::new();
    let r: serde_json::Value = http
        .post(format!("{base}/api/audio/dop-setup"))
        .json(&serde_json::json!({ "device": "DAC-qui-existe-pas-xyz-123", "rate": 176400 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], false);
    assert_eq!(r["applied"], false);
    assert_eq!(r["rate"], 176400);
    // le nom inconnu doit produire une erreur explicite (pas de repli sur le
    // périphérique par défaut, qui serait modifié à la place du bon DAC)
    assert!(r["message"].as_str().unwrap().contains("introuvable"));
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn restart_audio_hors_windows_message_propre() {
    let base = spawn_server().await;
    let http = reqwest::Client::new();
    let r: serde_json::Value = http
        .post(format!("{base}/api/audio/restart-audio"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("Windows"));
}
