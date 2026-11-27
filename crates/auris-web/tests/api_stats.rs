//! Tests d'intégration: stats bibliothèque (#74) — `GET /api/library/stats`.

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

fn make_track(uri: &str, artist: &str, album: &str, title: &str, dur: f64) -> Track {
    let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
    t.artist = artist.into();
    t.album_artist = artist.into();
    t.album = album.into();
    t.title = title.into();
    t.duration_secs = Some(dur);
    t.finalize_keys();
    t
}

async fn spawn_server() -> (String, Arc<Library>) {
    let root = unique_tmp("stats");
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
async fn stats_bibliotheque_vide_puis_remplie() {
    let (base, library) = spawn_server().await;
    let http = reqwest::Client::new();

    // vide au départ
    let s: serde_json::Value = http
        .get(format!("{base}/api/library/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["artists"], 0);
    assert_eq!(s["albums"], 0);
    assert_eq!(s["tracks"], 0);
    assert_eq!(s["playtime_secs"].as_f64().unwrap(), 0.0);

    // 2 artistes, 2 albums, 3 titres, durées 60+120+30 = 210 s
    library
        .upsert_track(&make_track(
            "a/1.flac",
            "Radiohead",
            "OK Computer",
            "Airbag",
            60.0,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "a/2.flac",
            "Radiohead",
            "OK Computer",
            "Paranoid Android",
            120.0,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "b/1.flac",
            "Portishead",
            "Dummy",
            "Sour Times",
            30.0,
        ))
        .unwrap();

    let s: serde_json::Value = http
        .get(format!("{base}/api/library/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["artists"], 2, "stats: {s}");
    assert_eq!(s["albums"], 2, "stats: {s}");
    assert_eq!(s["tracks"], 3, "stats: {s}");
    assert_eq!(s["playtime_secs"].as_f64().unwrap(), 210.0, "stats: {s}");
}
