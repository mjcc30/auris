//! Tests d'intégration « Jouer ici » : bascule de sortie (POST /api/player/output)
//! et flux binaire PCM sur /ws/audio.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use auris_core::{BrowserFrame, OutputMode, Source};
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState};
use futures_util::StreamExt;

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
    ws_base: String,
    engine: Arc<PlayerEngine>,
}

async fn spawn() -> Fixture {
    let root = unique_tmp("webbrowser");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(music.join("t1.wav"), b"RIFF....").unwrap();
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
    let state = AppState::new(engine.clone(), library, sources, updater, root);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    Fixture {
        base: format!("http://{addr}"),
        ws_base: format!("ws://{addr}"),
        engine,
    }
}

#[tokio::test]
async fn bascule_sortie_browser_puis_local() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    assert_eq!(f.engine.state().output_mode, OutputMode::Local);

    let st: serde_json::Value = http
        .post(format!("{}/api/player/output", f.base))
        .json(&serde_json::json!({ "mode": "browser" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["output_mode"], "browser");

    // le moteur reflète le changement (commande async)
    let mut ok = false;
    for _ in 0..40 {
        if f.engine.state().output_mode == OutputMode::Browser {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ok);

    let st: serde_json::Value = http
        .post(format!("{}/api/player/output", f.base))
        .json(&serde_json::json!({ "mode": "local" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["output_mode"], "local");

    // mode invalide -> 400
    let r = http
        .post(format!("{}/api/player/output", f.base))
        .json(&serde_json::json!({ "mode": "mars" }))
        .send()
        .await
        .unwrap();
    // 400/422 selon la couche de rejet (validation JSON axum = 422)
    assert!(r.status() == 400 || r.status() == 422);
}

#[tokio::test]
async fn ws_audio_recoit_spec_puis_pcm_binaire() {
    let f = spawn().await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("{}/ws/audio", f.ws_base))
        .await
        .unwrap();

    // 1ere trame: hello audio_mode
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let txt = msg.to_text().unwrap();
    let hello: serde_json::Value = serde_json::from_str(txt).unwrap();
    assert_eq!(hello["type"], "audio_mode");
    assert_eq!(hello["mode"], "local");

    // injection de trames côté moteur
    f.engine
        .browser_tx
        .send(BrowserFrame::Spec {
            rate: 48000,
            channels: 2,
        })
        .unwrap();
    let samples: Arc<[f32]> = vec![0.5f32, -0.25, 1.0, -1.0].into();
    f.engine
        .browser_tx
        .send(BrowserFrame::Pcm(samples.clone()))
        .unwrap();

    // trame spec (texte JSON)
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(spec["type"], "audio_spec");
    assert_eq!(spec["sample_rate"], 48000);
    assert_eq!(spec["channels"], 2);

    // trame PCM (binaire, f32 LE)
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bytes = msg.into_data();
    assert_eq!(bytes.len(), 16);
    let (chunks, _) = bytes.as_chunks::<4>();
    let got: Vec<f32> = chunks.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert_eq!(got, samples.to_vec());

    ws.close(None).await.ok();
}
