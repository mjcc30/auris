//! Tests d'intégration des réglages (GET/PUT /api/settings) et des endpoints
//! système avancés : stockage, logs, reset usine, alimentation (reboot/shutdown).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auris_core::{Settings, Source};
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState, PowerAction};

fn unique_tmp(tag: &str) -> std::path::PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("auris-{tag}-{}-{nanos}-{n}", std::process::id()))
}

struct Fixture {
    base: String,
    root: std::path::PathBuf,
    sink_calls: Arc<Mutex<Vec<Settings>>>,
    power_calls: Arc<Mutex<Vec<PowerAction>>>,
    engine: Arc<PlayerEngine>,
}

async fn spawn(power_control: bool) -> Fixture {
    let root = unique_tmp("websettings");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("t1.wav"), b"RIFF....").unwrap();
    // faux fichier de logs
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("auris.log"),
        "2026-01-01 INFO ligne-test\n2026-01-01 INFO autre\n",
    )
    .unwrap();

    let src = Source::new_local("local1", music.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
    let library = Arc::new(Library::open(&root, sources.clone()).unwrap());
    let engine = PlayerEngine::start(sources.clone(), None, 80);
    let updater = Arc::new(Updater::new(UpdaterConfig {
        repo_owner: "mjcc30".into(),
        repo_name: "auris".into(),
        bin_name: "auris".into(),
        current_version: "0.1.0".into(),
        auto_check: false,
    }));

    let sink_calls: Arc<Mutex<Vec<Settings>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_calls2 = sink_calls.clone();
    let settings_sink = Arc::new(move |st: &Settings| {
        sink_calls2.lock().unwrap().push(st.clone());
    });
    let power_calls: Arc<Mutex<Vec<PowerAction>>> = Arc::new(Mutex::new(Vec::new()));
    let power_calls2 = power_calls.clone();
    let power_ops = Arc::new(move |a: PowerAction| {
        power_calls2.lock().unwrap().push(a);
    });

    let mut state = AppState::new(engine.clone(), library, sources, updater, root.clone());
    state.settings_sink = Some(settings_sink);
    state.power_control = power_control;
    state.power_ops = Some(power_ops);
    state.log_path = Some(logs.join("auris.log"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    Fixture {
        base: format!("http://{addr}"),
        root,
        sink_calls,
        power_calls,
        engine,
    }
}

#[tokio::test]
async fn get_settings_defauts() {
    let f = spawn(false).await;
    let http = reqwest::Client::new();
    let s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s.playback.max_volume, 100);
    assert_eq!(s.appearance.accent, "#1ED760");
    assert_eq!(s.appearance.theme, auris_core::Theme::Dark);
    assert_eq!(s.system.device_name, "auris");
}

#[tokio::test]
async fn put_settings_valide_persiste_et_applique_au_moteur() {
    let f = spawn(false).await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s.playback.max_volume = 60;
    s.appearance.theme = auris_core::Theme::Light;
    s.appearance.accent = "#FF8800".into();
    let back: Settings = http
        .put(format!("{}/api/settings", f.base))
        .json(&s)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back.playback.max_volume, 60);
    // persistance via le sink
    {
        let calls = f.sink_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].playback.max_volume, 60);
    }
    // appliqué au moteur (commande async : on attend un peu)
    let mut ok = false;
    for _ in 0..40 {
        if f.engine.state().max_volume == 60 {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ok, "le moteur doit refléter max_volume=60");
}

#[tokio::test]
async fn put_settings_invalides_refuses_400() {
    let f = spawn(false).await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    s.playback.max_volume = 0;
    let r = http
        .put(format!("{}/api/settings", f.base))
        .json(&s)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let mut s2: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s2.appearance.accent = "rouge".into();
    let r = http
        .put(format!("{}/api/settings", f.base))
        .json(&s2)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    // aucun sink appelé pour les requêtes refusées
    assert!(f.sink_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn storage_et_logs() {
    let f = spawn(false).await;
    let http = reqwest::Client::new();

    let st: serde_json::Value = http
        .get(format!("{}/api/system/storage", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(st["used_bytes"].as_u64().unwrap() > 0);
    assert!(st["total_bytes"].as_u64().unwrap() > st["free_bytes"].as_u64().unwrap());
    assert!(st["data_dir"].as_str().unwrap().contains("websettings"));

    let logs = http
        .get(format!("{}/api/system/logs", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(logs.status(), 200);
    let body = logs.text().await.unwrap();
    assert!(body.contains("ligne-test"));
}

#[tokio::test]
async fn reset_exige_confirmation_puis_vide_la_bibliotheque() {
    let f = spawn(false).await;
    let http = reqwest::Client::new();

    let r = http
        .post(format!("{}/api/system/reset", f.base))
        .json(&serde_json::json!({ "confirm": "non" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert!(f.root.join("library").exists());

    let r = http
        .post(format!("{}/api/system/reset", f.base))
        .json(&serde_json::json!({ "confirm": "RESET" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(
        !f.root.join("library").exists(),
        "la bibliothèque doit être supprimée"
    );
}

#[tokio::test]
async fn alimentation_bloquee_sans_power_control_puis_ok_avec_mock() {
    // sans power_control -> 403
    let f = spawn(false).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{}/api/system/reboot", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(f.power_calls.lock().unwrap().is_empty());

    // avec power_control + mock -> 200 et action enregistrée
    let f2 = spawn(true).await;
    let r = http
        .post(format!("{}/api/system/reboot", f2.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("{}/api/system/shutdown", f2.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let calls = f2.power_calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        &[PowerAction::Reboot, PowerAction::Shutdown]
    );
}
