//! Tests d'intégration: historique d'écoute (#69) — `GET /api/library/history`,
//! `Library::record_play` (dédup consécutifs, cap, ordre).

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

fn make_track(uri: &str, title: &str) -> Track {
    let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
    t.artist = "Radiohead".into();
    t.album_artist = "Radiohead".into();
    t.album = "OK Computer".into();
    t.title = title.into();
    t.duration_secs = Some(60.0);
    t.has_cover = true;
    t.finalize_keys();
    t
}

async fn spawn_server() -> (String, Arc<Library>) {
    let root = unique_tmp("history");
    std::fs::create_dir_all(&root).unwrap();
    let src = Source::new_local("local1", root.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
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
    (format!("http://{addr}"), library)
}

#[tokio::test]
async fn history_ordre_dedup_et_limit() {
    let (base, library) = spawn_server().await;
    let t1 = make_track("a/1.flac", "Airbag");
    let t2 = make_track("a/2.flac", "Paranoid Android");
    let t3 = make_track("a/3.flac", "Exit Music");
    for t in [&t1, &t2, &t3] {
        library.upsert_track(t).unwrap();
    }

    let http = reqwest::Client::new();
    // vide au départ
    let h: serde_json::Value = http
        .get(format!("{base}/api/library/history"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h.as_array().unwrap().len(), 0);

    library.record_play(&t1.id).unwrap();
    library.record_play(&t1.id).unwrap(); // doublon consécutif : ignoré
    library.record_play(&t2.id).unwrap();
    library.record_play(&t3.id).unwrap();
    library.record_play(&t3.id).unwrap(); // doublon consécutif : ignoré

    let h: serde_json::Value = http
        .get(format!("{base}/api/library/history"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = h.as_array().unwrap();
    assert_eq!(arr.len(), 3, "history: {h}");
    // plus récent d'abord
    assert_eq!(arr[0]["id"], t3.id);
    assert_eq!(arr[1]["id"], t2.id);
    assert_eq!(arr[2]["id"], t1.id);
    assert!(arr[0]["last_played"].as_i64().unwrap() > 0);
    assert_eq!(arr[0]["title"], "Exit Music");

    // limit=2
    let h: serde_json::Value = http
        .get(format!("{base}/api/library/history?limit=2"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = h.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], t3.id);

    // piste inconnue de l'historique : ignorée silencieusement
    library.record_play("id-inexistant").unwrap();
    let h: serde_json::Value = http
        .get(format!("{base}/api/library/history?limit=10"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h.as_array().unwrap().len(), 3, "history: {h}");
}

#[tokio::test]
async fn history_cap_500() {
    let (_base, library) = spawn_server().await;
    let t = make_track("a/1.flac", "Airbag");
    library.upsert_track(&t).unwrap();
    // 600 lectures entrelacées de 2 pistes -> cap à 500, dédup consécutifs
    for i in 0..600 {
        let id = if i % 2 == 0 {
            t.id.clone()
        } else {
            "autre".into()
        };
        library.record_play(&id).unwrap();
    }
    assert_eq!(library.history(1000).len(), 500);
}
