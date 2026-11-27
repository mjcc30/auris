//! Test d'intégration de l'API Subsonic v1 (lecture seule, #79) :
//! `/rest/{method}` répond `{"subsonic-response": …}` en JSON.

use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState};

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

fn unique_tmp(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("auris-{tag}-{}-{nanos}-{n}", std::process::id()))
}

async fn spawn_server() -> (String, std::path::PathBuf, Source) {
    let root = unique_tmp("subsonic");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 2);
    write_test_wav(&music.join("t2.wav"), 3);

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
    let state = AppState::new(engine, library, sources, updater, root.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), root, src)
}

async fn rest(http: &reqwest::Client, base: &str, method: &str, extra: &str) -> serde_json::Value {
    let url = format!("{base}/rest/{method}.view?f=json&u=test&p=test{extra}");
    let r = http.get(&url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

#[tokio::test]
async fn subsonic_v1_lecture_seule() {
    let (base, root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // scan de la source locale
    http.post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({"source_id": src.id}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/library/stats"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["tracks"].as_u64().unwrap_or(0) >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // ping
    let r = rest(&http, &base, "ping", "").await;
    assert_eq!(r["subsonic-response"]["status"], "ok");
    assert_eq!(r["subsonic-response"]["version"], "1.16.1");

    // getMusicFolders
    let r = rest(&http, &base, "getMusicFolders", "").await;
    assert_eq!(
        r["subsonic-response"]["musicFolders"]["musicFolder"][0]["name"],
        "Bibliothèque"
    );

    // getArtists -> au moins un artiste (Artiste inconnu du scan wav)
    let r = rest(&http, &base, "getArtists", "").await;
    let idx = &r["subsonic-response"]["artists"]["index"];
    assert!(idx.as_array().map(|a| !a.is_empty()).unwrap_or(false));

    // getAlbumList2 alphabeticalByName
    let r = rest(
        &http,
        &base,
        "getAlbumList2",
        "&type=alphabeticalByName&size=10",
    )
    .await;
    let albums = r["subsonic-response"]["albumList2"]["album"]
        .as_array()
        .unwrap();
    assert!(!albums.is_empty());
    let album_id = albums[0]["id"].as_str().unwrap().to_string();

    // getAlbum -> songs
    let r = rest(&http, &base, "getAlbum", &format!("&id={album_id}")).await;
    let songs = r["subsonic-response"]["album"]["song"].as_array().unwrap();
    assert_eq!(songs.len(), 2);
    let t1 = songs
        .iter()
        .find(|c| {
            c["path"]
                .as_str()
                .map(|p| p.ends_with("t1.wav"))
                .unwrap_or(false)
        })
        .expect("t1.wav présent dans l'album");
    let track_id = t1["id"].as_str().unwrap().to_string();
    assert_eq!(t1["type"], "music");
    assert_eq!(t1["suffix"], "wav");
    assert_eq!(t1["contentType"], "audio/wav");

    // getSong
    let r = rest(&http, &base, "getSong", &format!("&id={track_id}")).await;
    assert_eq!(r["subsonic-response"]["song"]["id"], track_id);

    // search3
    let r = rest(
        &http,
        &base,
        "search3",
        "&query=t1&songCount=10&albumCount=10&artistCount=10",
    )
    .await;
    let sr = &r["subsonic-response"]["searchResult3"];
    assert!(sr["song"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false));

    // getPlaylists (vide mais structuré)
    let r = rest(&http, &base, "getPlaylists", "").await;
    assert_eq!(r["subsonic-response"]["status"], "ok");

    // stream entier = fichier wav original
    let expected = std::fs::read(root.join("music/t1.wav")).unwrap();
    let r = http
        .get(format!("{base}/rest/stream.view?f=json&id={track_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers().get("content-type").unwrap(), "audio/wav");
    let body = r.bytes().await.unwrap();
    assert_eq!(&body[..], &expected[..]);

    // stream avec Range -> 206 partiel
    let r = http
        .get(format!("{base}/rest/stream.view?f=json&id={track_id}"))
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(
        r.headers().get("content-range").unwrap(),
        &format!("bytes 0-99/{}", expected.len())
    );
    let body = r.bytes().await.unwrap();
    assert_eq!(&body[..], &expected[..100]);

    // méthode inconnue -> failed code 0, HTTP 200
    let r = rest(&http, &base, "createPlaylist", "&name=x").await;
    assert_eq!(r["subsonic-response"]["status"], "failed");
    assert_eq!(r["subsonic-response"]["error"]["code"], 0);

    // f=xml -> refus propre
    let r: serde_json::Value = http
        .get(format!("{base}/rest/ping.view?f=xml"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["subsonic-response"]["status"], "failed");
}
