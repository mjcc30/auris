//! Tests d'intégration: workflow « morceau d'abord » (#106) — ajout en masse à
//! la file (`POST /api/queue/append`), réordonnancement des playlists
//! (`PATCH /api/playlists/{id}` avec `track_ids`) et ajout par URI
//! (`POST /api/playlists/{id}/add`).

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

fn make_track(uri: &str, artist: &str, title: &str) -> Track {
    let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
    t.artist = artist.into();
    t.album_artist = artist.into();
    t.album = format!("Album de {artist}");
    t.title = title.into();
    t.genre = Some("rock".into());
    t.duration_secs = Some(60.0);
    t.finalize_keys();
    t
}

async fn spawn_server() -> (String, Arc<Library>) {
    let root = unique_tmp("trackfirst");
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
async fn queue_append_playlist_reorder_et_add_par_uri() {
    let (base, library) = spawn_server().await;
    let http = reqwest::Client::new();

    let t1 = make_track("a/1.wav", "Artiste A", "Morceau 1");
    let t2 = make_track("a/2.wav", "Artiste A", "Morceau 2");
    library.upsert_track(&t1).unwrap();
    library.upsert_track(&t2).unwrap();

    // 1. ajout en masse à la file (un URI vide est ignoré)
    let r: serde_json::Value = http
        .post(format!("{base}/api/queue/append"))
        .json(&serde_json::json!({ "uris": ["src://local1/a/1.wav", "", "src://local1/a/2.wav"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["added"], 2);
    assert_eq!(r["state"]["queue"].as_array().unwrap().len(), 2);

    let q: Vec<serde_json::Value> = http
        .get(format!("{base}/api/queue"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(q.len(), 2);
    assert_eq!(q[0]["id"], t1.id);

    // 2. playlist créée par IDs, puis réordonnée par PATCH track_ids
    let pl: serde_json::Value = http
        .post(format!("{base}/api/playlists"))
        .json(&serde_json::json!({ "name": "Ma liste", "track_ids": [t1.id, t2.id] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = pl["id"].as_str().unwrap().to_string();

    let pl2: serde_json::Value = http
        .patch(format!("{base}/api/playlists/{pid}"))
        .json(&serde_json::json!({ "track_ids": [t2.id, t1.id] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        pl2["track_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        vec![t2.id.clone(), t1.id.clone()]
    );

    // la lecture reflète le nouvel ordre
    let detail: serde_json::Value = http
        .get(format!("{base}/api/playlists/{pid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let order: Vec<String> = detail["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(order, vec![t2.id.clone(), t1.id.clone()]);

    // 3. ajout par URI (piste hors scan -> créée + upsertée)
    let add: serde_json::Value = http
        .post(format!("{base}/api/playlists/{pid}/add"))
        .json(&serde_json::json!({ "uri": "src://local1/b/3.wav" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(add["added"], 1);
    assert!(library.get_by_uri("src://local1/b/3.wav").is_some());

    // 4. mix depuis un morceau : l'autre piste du même artiste est proposée
    let mix: serde_json::Value = http
        .post(format!("{base}/api/mix"))
        .json(&serde_json::json!({ "kind": "track", "id": t1.id, "limit": 10 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<String> = mix["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&t2.id), "mix = {ids:?}");
}
