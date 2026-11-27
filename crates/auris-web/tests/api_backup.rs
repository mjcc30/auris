//! Tests d'intégration: sauvegarde / restauration de la configuration (#40) —
//! `GET /api/settings/backup` (ZIP), `POST /api/settings/restore` (roundtrip),
//! variantes WebDAV (PUT/GET) contre un mini-serveur factice.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auris_core::{read_zip, FavKind, PinKind, ShareKind, Source, Track};
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
    t.finalize_keys();
    t
}

async fn spawn_server(tag: &str) -> (String, std::path::PathBuf, Arc<Library>) {
    let root = unique_tmp(tag);
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::write(root.join("config.toml"), "port = 3000\nvolume = 42\n").unwrap();
    let src = Source::new_local("local1", music.to_str().unwrap());
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
    (format!("http://{addr}"), root, library)
}

/// Mini-serveur WebDAV factice : PUT puis GET rendent les octets stockes.
async fn spawn_fake_webdav() -> (String, Arc<Mutex<Vec<u8>>>) {
    use axum::{body::Bytes, extract::Path, Router};
    let store: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let st = store.clone();
    let st_get = store.clone();
    let app = Router::new().route(
        "/dav/{name}",
        axum::routing::put(move |Path(name): Path<String>, body: Bytes| {
            let st = st.clone();
            async move {
                assert!(name.ends_with(".zip"));
                *st.lock().unwrap() = body.to_vec();
                axum::http::StatusCode::CREATED
            }
        })
        .get(move |Path(name): Path<String>| {
            let st = st_get.clone();
            async move {
                assert!(name.ends_with(".zip"));
                axum::response::Response::builder()
                    .status(200)
                    .body(axum::body::Body::from(st.lock().unwrap().clone()))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), store)
}

#[tokio::test]
async fn backup_restore_roundtrip_zip_local() {
    let (base_a, root_a, lib_a) = spawn_server("bak-a").await;
    let t = make_track("a/1.flac", "Radiohead", "OK Computer", "Airbag");
    lib_a.upsert_track(&t).unwrap();
    let tid = t.id.clone();
    lib_a.toggle_favorite(FavKind::Track, &tid).unwrap();
    lib_a.toggle_pin(PinKind::Album, "album-key-1").unwrap();
    let pl = lib_a
        .create_playlist("Ma liste", vec![tid.clone()])
        .unwrap();
    lib_a.record_play(&tid).unwrap();
    let share = lib_a
        .create_share(ShareKind::Track, &tid, "Airbag", None)
        .unwrap();
    let http = reqwest::Client::new();

    // --- backup : ZIP telechargeable ---
    let resp = http
        .get(format!("{base_a}/api/settings/backup"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("application/zip"), "content-type: {ct}");
    let bytes = resp.bytes().await.unwrap().to_vec();

    // le ZIP contient manifest + config + toutes les donnees
    let entries = read_zip(&bytes).unwrap();
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    for attendu in [
        "manifest.json",
        "config.toml",
        "data/favorites.json",
        "data/pins.json",
        "data/playlists.json",
        "data/history.json",
        "data/shares.json",
        "data/settings.json",
    ] {
        assert!(
            names.contains(&attendu),
            "entree manquante: {attendu} dans {names:?}"
        );
    }
    let cfg = entries
        .iter()
        .find(|(n, _)| n == "config.toml")
        .unwrap()
        .1
        .clone();
    assert!(String::from_utf8_lossy(&cfg).contains("volume = 42"));

    // --- restore sur une seconde instance (data-dir vierge, piste re-scannée) ---
    let (base_b, root_b, lib_b) = spawn_server("bak-b").await;
    lib_b.upsert_track(&t).unwrap(); // la bibliothèque est re-scannée, pas dans le ZIP
    let r: serde_json::Value = http
        .post(format!("{base_b}/api/settings/restore"))
        .header("content-type", "application/zip")
        .body(bytes.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["files"].as_u64().unwrap(), 8);
    assert_eq!(r["restored"]["favorites"], true);
    assert_eq!(r["restored"]["playlists"], 1);
    assert_eq!(r["restored"]["history"], 1);
    assert_eq!(r["restored"]["shares"], 1);
    assert_eq!(r["config_restored"], true);

    // config.toml ecrit (et ancienne sauvegarde .bak cree)
    let cfg_b = std::fs::read_to_string(root_b.join("config.toml")).unwrap();
    assert!(cfg_b.contains("volume = 42"));
    assert!(root_b.join("config.toml.bak").exists());

    // donnees verifiables via les API existantes
    let favs: serde_json::Value = http
        .get(format!("{base_b}/api/favorites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(favs["tracks"][0], tid);
    let pins: serde_json::Value = http
        .get(format!("{base_b}/api/pins"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pins["albums"][0], "album-key-1");
    let pls: serde_json::Value = http
        .get(format!("{base_b}/api/playlists"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pls = pls.as_array().unwrap();
    assert_eq!(pls.len(), 1);
    assert_eq!(pls[0]["id"], pl.id);
    assert_eq!(pls[0]["track_ids"][0], tid);
    let hist: serde_json::Value = http
        .get(format!("{base_b}/api/library/history?limit=10"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hist = hist.as_array().unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0]["id"], tid);
    let shares: serde_json::Value = http
        .get(format!("{base_b}/api/shares"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let shares = shares.as_array().unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["token"], share.token);

    // restore avec un corps invalide -> 400
    let resp = http
        .post(format!("{base_b}/api/settings/restore"))
        .header("content-type", "application/zip")
        .body(b"ceci n'est pas un zip".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let _ = std::fs::remove_dir_all(&root_a);
    let _ = std::fs::remove_dir_all(&root_b);
}

#[tokio::test]
async fn backup_restore_via_webdav() {
    let (base_a, _root_a, lib_a) = spawn_server("dav-a").await;
    let t = make_track("a/1.flac", "Björk", "Homogenic", "Hunter");
    lib_a.upsert_track(&t).unwrap();
    lib_a.toggle_favorite(FavKind::Track, &t.id).unwrap();

    let (dav_base, store) = spawn_fake_webdav().await;
    let http = reqwest::Client::new();

    // push WebDAV
    let r: serde_json::Value = http
        .post(format!("{base_a}/api/settings/backup/webdav"))
        .json(&serde_json::json!({
            "url": format!("{dav_base}/dav"),
            "name": "test-backup.zip",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["pushed"]
        .as_str()
        .unwrap()
        .ends_with("/dav/test-backup.zip"));
    assert!(r["bytes"].as_u64().unwrap() > 100);
    assert!(!store.lock().unwrap().is_empty());

    // restore depuis WebDAV sur une autre instance
    let (base_b, _root_b, _lib_b) = spawn_server("dav-b").await;
    let r: serde_json::Value = http
        .post(format!("{base_b}/api/settings/restore/webdav"))
        .json(&serde_json::json!({
            "url": format!("{dav_base}/dav/test-backup.zip"),
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["restored"]["favorites"], true);
    let favs: serde_json::Value = http
        .get(format!("{base_b}/api/favorites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(favs["tracks"][0], t.id);

    // URL vide -> 400
    let resp = http
        .post(format!("{base_b}/api/settings/backup/webdav"))
        .json(&serde_json::json!({ "url": "  " }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
