//! Tests d'intégration : minuterie de sommeil / réveil / égaliseur via
//! `PUT /api/settings` et `GET /api/alarms`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auris_core::{Settings, Source};
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

struct Fixture {
    base: String,
    sink_calls: Arc<Mutex<Vec<Settings>>>,
    alarms_handle: Arc<Mutex<auris_core::AlarmState>>,
}

async fn spawn() -> Fixture {
    let root = unique_tmp("webalarms");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();

    let src = Source::new_local("local1", music.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
    let library = Arc::new(Library::open(&root, sources.clone()).unwrap());
    let engine = PlayerEngine::start(sources.clone(), None, 80, false);
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

    let alarms_handle: Arc<Mutex<auris_core::AlarmState>> =
        Arc::new(Mutex::new(auris_core::AlarmState::default()));
    let mut state = AppState::new(engine.clone(), library, sources, updater, root.clone());
    state.settings_sink = Some(settings_sink);
    state.alarms = alarms_handle.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    Fixture {
        base: format!("http://{addr}"),
        sink_calls,
        alarms_handle,
    }
}

#[tokio::test]
async fn alarms_off_par_defaut() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let a: serde_json::Value = http
        .get(format!("{}/api/alarms", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a["sleep_timer"], serde_json::Value::Null);
    assert_eq!(a["sleep_remaining_secs"], serde_json::Value::Null);
    assert_eq!(a["wake_time"], serde_json::Value::Null);
}

#[tokio::test]
async fn sleep_timer_arma_et_compte_a_rebours() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s.playback.sleep_timer = Some(30);
    s.playback.wake_time = Some("06:30".into());
    let back: Settings = http
        .put(format!("{}/api/settings", f.base))
        .json(&s)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back.playback.sleep_timer, Some(30));
    assert_eq!(back.playback.wake_time.as_deref(), Some("06:30"));

    // le délai est armé côté serveur (tick de serve non lancé ici, on arme via l'état)
    // -> on vérifie surtout que GET /api/alarms reflète les réglages
    let a: serde_json::Value = http
        .get(format!("{}/api/alarms", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a["sleep_timer"], 30);
    assert_eq!(a["wake_time"], "06:30");
    // sans tick, pas de deadline armée : remaining null
    assert_eq!(a["sleep_remaining_secs"], serde_json::Value::Null);

    // persistance via le sink
    assert_eq!(f.sink_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn sleep_timer_invalide_refuse_400() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s.playback.sleep_timer = Some(0);
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
    s2.playback.wake_time = Some("25:00".into());
    let r = http
        .put(format!("{}/api/settings", f.base))
        .json(&s2)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert!(f.sink_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn eq_valide_persiste_et_invalide_refuse() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!s.playback.eq.enabled);
    assert_eq!(s.playback.eq.bands.len(), 10);

    s.playback.eq.enabled = true;
    s.playback.eq.bands[5].gain_db = 6.0; // 1 kHz
    s.playback.eq.bands[0].gain_db = -3.5; // 31 Hz
    let back: Settings = http
        .put(format!("{}/api/settings", f.base))
        .json(&s)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(back.playback.eq.enabled);
    assert_eq!(back.playback.eq.bands[5].gain_db, 6.0);
    assert_eq!(back.playback.eq.bands[0].freq_hz, 31);

    // gain hors plage -> 400
    let mut s2: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s2.playback.eq.enabled = true;
    s2.playback.eq.bands[5].gain_db = 25.0;
    let r = http
        .put(format!("{}/api/settings", f.base))
        .json(&s2)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn etat_alarms_avec_deadline_arme() {
    // on arme directement l'état (sans attendre le tick de serve) et on lit /api/alarms
    let f = spawn().await;
    let http = reqwest::Client::new();
    let mut s: Settings = http
        .get(format!("{}/api/settings", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    s.playback.sleep_timer = Some(10);
    http.put(format!("{}/api/settings", f.base))
        .json(&s)
        .send()
        .await
        .unwrap();

    // simule un tick : armement de la deadline
    {
        let now = chrono::Utc::now().timestamp();
        let handle = f_state_alarms(&f);
        let mut a = handle.lock().unwrap();
        let ev = auris_core::tick(now, 0, Some(10), None, &mut a);
        assert!(ev.is_empty());
    }
    let a: serde_json::Value = http
        .get(format!("{}/api/alarms", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rem = a["sleep_remaining_secs"].as_i64().unwrap();
    assert!((590..=600).contains(&rem), "remaining {rem}");
}

// helper : accès à l'état des alarmes du fixture (AppState non exposé, on passe par un clone)
fn f_state_alarms(f: &Fixture) -> Arc<Mutex<auris_core::AlarmState>> {
    f.alarms_handle.clone()
}
