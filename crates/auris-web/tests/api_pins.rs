//! Tests d'intégration: épingles (#70) — `GET /api/pins`, `POST /api/pins/toggle`,
//! `GET /api/pins/resolved`.

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

fn make_track(uri: &str, artist: &str, album: &str, title: &str) -> Track {
    let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
    t.artist = artist.into();
    t.album_artist = artist.into();
    t.album = album.into();
    t.title = title.into();
    t.duration_secs = Some(60.0);
    t.has_cover = true;
    t.finalize_keys();
    t
}

async fn spawn_server() -> (String, Arc<Library>) {
    let root = unique_tmp("pins");
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
async fn toggle_epingle_album_playlist_radio_et_resolu() {
    let (base, library) = spawn_server().await;
    let t = make_track("a/1.flac", "Radiohead", "OK Computer", "Airbag");
    library.upsert_track(&t).unwrap();
    // index album/playlist reconstruit via les méthodes haut niveau
    let album = library.albums();
    let album_id = album
        .iter()
        .find(|a| a.title == "OK Computer")
        .map(|a| a.id.clone());
    let pl = library
        .create_playlist("Ma liste", vec![t.id.clone()])
        .unwrap();

    let http = reqwest::Client::new();
    let pins: serde_json::Value = http
        .get(format!("{base}/api/pins"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(pins["albums"].as_array().unwrap().is_empty());

    // album
    if let Some(aid) = album_id {
        let r: serde_json::Value = http
            .post(format!("{base}/api/pins/toggle"))
            .json(&serde_json::json!({ "kind": "album", "id": aid }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["pinned"], true);

        let res: serde_json::Value = http
            .get(format!("{base}/api/pins/resolved"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let albums = res["albums"].as_array().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0]["title"], "OK Computer");
        assert!(albums[0].get("cover_url").is_some());

        // re-toggle -> off
        let r: serde_json::Value = http
            .post(format!("{base}/api/pins/toggle"))
            .json(&serde_json::json!({ "kind": "album", "id": aid }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["pinned"], false);
    }

    // playlist + radio (ordre conservé)
    for (kind, id) in [
        ("playlist", pl.id.clone()),
        ("radio", "https://flux.example/radio.mp3".to_string()),
    ] {
        let r: serde_json::Value = http
            .post(format!("{base}/api/pins/toggle"))
            .json(&serde_json::json!({ "kind": kind, "id": id }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["pinned"], true, "kind={kind}");
    }
    let res: serde_json::Value = http
        .get(format!("{base}/api/pins/resolved"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["playlists"][0]["name"], "Ma liste");
    assert_eq!(res["radios"][0], "https://flux.example/radio.mp3");

    // erreurs
    let status = http
        .post(format!("{base}/api/pins/toggle"))
        .json(&serde_json::json!({ "kind": "album", "id": "  " }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status.as_u16(), 400);
    let status = http
        .post(format!("{base}/api/pins/toggle"))
        .json(&serde_json::json!({ "kind": "bogus", "id": "x" }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status.as_u16(), 422);
}
