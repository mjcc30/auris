//! Tests d'intégration: favoris bibliothèque (#68) — `GET /api/favorites`,
//! `POST /api/favorites/toggle`, `GET /api/favorites/resolved`, persistance sled.

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

async fn spawn_server() -> (String, std::path::PathBuf, Arc<Library>, Source) {
    let root = unique_tmp("favs");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 2);
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
    (format!("http://{addr}"), root, library, src)
}

#[tokio::test]
async fn toggle_favori_piste_et_etat() {
    let (base, _root, library, _src) = spawn_server().await;
    let t = make_track("a/1.flac", "Radiohead", "OK Computer", "Airbag");
    library.upsert_track(&t).unwrap();
    let tid = t.id.clone();

    let http = reqwest::Client::new();
    // état initial vide
    let favs: serde_json::Value = http
        .get(format!("{base}/api/favorites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(favs["tracks"].as_array().unwrap().is_empty());

    // toggle ON
    let r: serde_json::Value = http
        .post(format!("{base}/api/favorites/toggle"))
        .json(&serde_json::json!({ "kind": "track", "id": tid }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["favorited"], true);
    assert_eq!(r["kind"], "track");

    let favs: serde_json::Value = http
        .get(format!("{base}/api/favorites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(favs["tracks"].as_array().unwrap().len(), 1);
    assert_eq!(favs["tracks"][0], tid);

    // resolved contient la piste complète
    let res: serde_json::Value = http
        .get(format!("{base}/api/favorites/resolved"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tracks = res["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["title"], "Airbag");

    // toggle OFF
    let r: serde_json::Value = http
        .post(format!("{base}/api/favorites/toggle"))
        .json(&serde_json::json!({ "kind": "track", "id": tid }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["favorited"], false);
    let favs: serde_json::Value = http
        .get(format!("{base}/api/favorites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(favs["tracks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn favoris_album_et_artiste_resolus() {
    let (base, _root, _library, src) = spawn_server().await;
    let http = reqwest::Client::new();
    // scan du wav de test pour indexer piste + album + artiste
    let scan_res = http
        .post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({ "source_id": src.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_res.status(), 200);
    let mut albums: Vec<serde_json::Value> = Vec::new();
    let mut artists: Vec<serde_json::Value> = Vec::new();
    for _ in 0..200 {
        albums = http
            .get(format!("{base}/api/library/albums"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        artists = http
            .get(format!("{base}/api/library/artists"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !albums.is_empty() && !artists.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(albums.len(), 1);
    assert_eq!(artists.len(), 1);
    let album_id = albums[0]["id"].as_str().unwrap().to_string();
    let artist_id = artists[0]["id"].as_str().unwrap().to_string();

    for (kind, id) in [("album", &album_id), ("artist", &artist_id)] {
        let r = http
            .post(format!("{base}/api/favorites/toggle"))
            .json(&serde_json::json!({ "kind": kind, "id": id }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let res: serde_json::Value = http
        .get(format!("{base}/api/favorites/resolved"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fav_albums = res["albums"].as_array().unwrap();
    assert_eq!(fav_albums.len(), 1);
    assert_eq!(fav_albums[0]["id"], album_id);
    assert!(fav_albums[0]["cover_url"]
        .as_str()
        .map(|c| c.contains("/api/library/cover/"))
        .unwrap_or(true));
    let fav_artists = res["artists"].as_array().unwrap();
    assert_eq!(fav_artists.len(), 1);
    assert_eq!(fav_artists[0]["id"], artist_id);
}

#[tokio::test]
async fn favoris_invalides_rejetes() {
    let (base, _root, _library, _src) = spawn_server().await;
    let http = reqwest::Client::new();
    // kind inconnu -> rejeté par la désérialisation (422)
    let r = http
        .post(format!("{base}/api/favorites/toggle"))
        .json(&serde_json::json!({ "kind": "podcast", "id": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    // id vide -> 400
    let r = http
        .post(format!("{base}/api/favorites/toggle"))
        .json(&serde_json::json!({ "kind": "track", "id": "  " }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn favoris_persistes_au_reouverture() {
    // test au niveau Library (le serveur HTTP garde le verrou sled sinon)
    let root = unique_tmp("favs-persist");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    let src = Source::new_local("local1", music.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
    let tid;
    {
        let library = Library::open(&root.join("db"), sources.clone()).unwrap();
        let t = make_track("a/1.flac", "Radiohead", "OK Computer", "Airbag");
        library.upsert_track(&t).unwrap();
        tid = t.id.clone();
        let on = library
            .toggle_favorite(auris_core::FavKind::Track, &tid)
            .unwrap();
        assert!(on);
        assert_eq!(library.favorites().tracks, vec![tid.clone()]);
    }
    // réouverture sur le même data-dir : le favori est toujours là
    let lib2 = Library::open(&root.join("db"), sources).unwrap();
    let favs = lib2.favorites();
    assert_eq!(favs.tracks, vec![tid.clone()]);
    let resolved = lib2.favorite_tracks();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, tid);
    // re-toggle -> OFF et persisté
    let on = lib2
        .toggle_favorite(auris_core::FavKind::Track, &favs.tracks[0])
        .unwrap();
    assert!(!on);
    let sources2 = lib2.sources();
    drop(lib2);
    let lib3 = Library::open(&root.join("db"), sources2).unwrap();
    assert!(lib3.favorites().tracks.is_empty());
}
