//! Tests d'intégration: sélection du DAC (/api/audio/device) avec validation
//! du nom contre la liste réelle, persistance via `player_sink`, et mode de
//! sortie (/api/player/output) persisté lui aussi.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auris_core::{AudioDeviceInfo, OutputMode, Source};
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

fn fake_device(name: &str, is_default: bool) -> AudioDeviceInfo {
    AudioDeviceInfo {
        name: name.to_string(),
        is_default,
        sample_rates: vec![44100, 48000],
        supports_i24: false,
        supports_f32: true,
        max_channels: 2,
    }
}

type Captured = Arc<Mutex<Vec<(Option<String>, OutputMode)>>>;

async fn spawn_server() -> (String, Captured) {
    let root = unique_tmp("device");
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
    let mut state = AppState::new(engine, library, sources, updater, root.clone());

    let devices = vec![
        fake_device("Casque (iBasso DC03 Pro)", false),
        fake_device("Haut-parleurs (Realtek)", true),
    ];
    state.device_lister = Some(Arc::new(move || devices.clone()));

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let captured2 = captured.clone();
    state.player_sink = Some(Arc::new(move |dev, mode| {
        captured2.lock().unwrap().push((dev, mode));
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), captured)
}

#[tokio::test]
async fn get_devices_utilise_le_lister() {
    let (base, _c) = spawn_server().await;
    let http = reqwest::Client::new();
    let devices: Vec<serde_json::Value> = http
        .get(format!("{base}/api/audio/devices"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = devices
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Casque (iBasso DC03 Pro)"));
    assert!(names.contains(&"Haut-parleurs (Realtek)"));
}

#[tokio::test]
async fn post_device_nom_inconnu_400_avec_liste() {
    let (base, captured) = spawn_server().await;
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/audio/device"))
        .json(&serde_json::json!({ "name": "DAC Fantome" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("DAC Fantome"), "{err}");
    assert!(
        err.contains("Casque (iBasso DC03 Pro)"),
        "la liste des noms connus est fournie: {err}"
    );
    assert!(captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn post_device_valide_persiste_en_mode_local() {
    let (base, captured) = spawn_server().await;
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/audio/device"))
        .json(&serde_json::json!({ "name": "Casque (iBasso DC03 Pro)" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let st: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(st["output_mode"], "local");
    let calls = captured.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.as_deref(), Some("Casque (iBasso DC03 Pro)"));
    assert!(matches!(calls[0].1, OutputMode::Local));
}

#[tokio::test]
async fn post_device_null_remet_defaut_et_persiste() {
    let (base, captured) = spawn_server().await;
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/audio/device"))
        .json(&serde_json::json!({ "name": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let calls = captured.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, None);
    assert!(matches!(calls[0].1, OutputMode::Local));
}

#[tokio::test]
async fn post_output_persiste_le_mode() {
    let (base, captured) = spawn_server().await;
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/player/output"))
        .json(&serde_json::json!({ "mode": "browser" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let st: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(st["output_mode"], "browser");
    let calls = captured.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(matches!(calls[0].1, OutputMode::Browser));
}
