//! Tests d'intégration des liens de partage (v0.6) : création, liste, révocation,
//! JSON public, mini-page HTML `/share/<token>` et flux audio `/share/<token>/stream`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use auris_core::{Album, Source, Track};
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

struct Fixture {
    base: String,
    root: std::path::PathBuf,
    wav_bytes: Vec<u8>,
}

async fn spawn() -> Fixture {
    let root = unique_tmp("webshares");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 1);
    write_test_wav(&music.join("t2.wav"), 1);
    let wav_bytes = std::fs::read(music.join("t1.wav")).unwrap();

    let src = Source::new_local("local1", music.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let library = Arc::new(Library::open(&root, sources.clone()).unwrap());
    library.scan_source(&src.id).unwrap();
    for _ in 0..200 {
        if !library.scan_status().running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(library.tracks().len(), 2, "scan requis pour le fixture");

    let engine = PlayerEngine::start(sources.clone(), None, 80);
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
    Fixture {
        base: format!("http://{addr}"),
        root,
        wav_bytes,
    }
}

async fn tracks(f: &Fixture) -> Vec<Track> {
    reqwest::Client::new()
        .get(format!("{}/api/library/tracks", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn partage_piste_cycle_complet() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let t = tracks(&f).await.remove(0);

    // creation -> 201 avec token + url
    let r = http
        .post(format!("{}/api/shares", f.base))
        .json(&serde_json::json!({ "kind": "track", "id": t.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let share: serde_json::Value = r.json().await.unwrap();
    let token = share["token"].as_str().unwrap().to_string();
    assert_eq!(share["url"].as_str().unwrap(), format!("/share/{token}"));
    assert_eq!(share["kind"].as_str().unwrap(), "track");

    // listage
    let list: Vec<serde_json::Value> = http
        .get(format!("{}/api/shares", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["token"].as_str().unwrap(), token);

    // JSON public
    let publ: serde_json::Value = http
        .get(format!("{}/api/public/shares/{token}", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(publ["tracks"].as_array().unwrap().len(), 1);
    assert_eq!(
        publ["tracks"][0]["stream_url"].as_str().unwrap(),
        format!("/share/{token}/stream")
    );

    // mini-page HTML publique
    let page = http
        .get(format!("{}/share/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let ct = page
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"));
    let body = page.text().await.unwrap();
    assert!(body.contains("<audio"), "lecteur audio présent");
    assert!(body.contains(&format!("/share/{token}/stream")));

    // flux audio == octets du fichier
    let stream = http
        .get(format!("{}/share/{token}/stream", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    let ct = stream
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(ct, "audio/wav");
    let bytes = stream.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), f.wav_bytes.as_slice());

    // un lien « piste » ne sert pas /stream/<track_id>
    let r = http
        .get(format!("{}/share/{token}/stream/{}", f.base, t.id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // révocation puis 404
    let r = http
        .delete(format!("{}/api/shares/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .get(format!("{}/api/public/shares/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = http
        .delete(format!("{}/api/shares/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    let _ = std::fs::remove_dir_all(&f.root);
}

#[tokio::test]
async fn partage_album_et_playlist() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let ts = tracks(&f).await;
    assert_eq!(ts.len(), 2);

    // album (les 2 wav sans tags -> un album « Album inconnu »)
    let albums: Vec<Album> = http
        .get(format!("{}/api/library/albums", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(albums.len(), 1);
    let aid = albums[0].id.clone();

    let r = http
        .post(format!("{}/api/shares", f.base))
        .json(&serde_json::json!({ "kind": "album", "id": aid }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let publ: serde_json::Value = http
        .get(format!("{}/api/public/shares/{token}", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pts = publ["tracks"].as_array().unwrap();
    assert_eq!(pts.len(), 2);

    // chaque piste de l'album est servie, une piste inconnue -> 404
    for t in &ts {
        let r = http
            .get(format!("{}/share/{token}/stream/{}", f.base, t.id))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "piste {} servie", t.id);
        assert_eq!(r.headers().get("content-type").unwrap(), "audio/wav");
    }
    let r = http
        .get(format!("{}/share/{token}/stream/piste-inconnue", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // playlist
    let r = http
        .post(format!("{}/api/playlists", f.base))
        .json(&serde_json::json!({ "name": "soirée", "track_ids": [ts[0].id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let pid = r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = http
        .post(format!("{}/api/shares", f.base))
        .json(&serde_json::json!({ "kind": "playlist", "id": pid }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let token2 = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let publ2: serde_json::Value = http
        .get(format!("{}/api/public/shares/{token2}", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(publ2["kind"].as_str().unwrap(), "playlist");
    assert_eq!(publ2["tracks"].as_array().unwrap().len(), 1);
    let r = http
        .get(format!("{}/share/{token2}/stream/{}", f.base, ts[0].id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // piste hors playlist -> 404
    let r = http
        .get(format!("{}/share/{token2}/stream/{}", f.base, ts[1].id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // page HTML de la playlist : liste de pistes avec data-src
    let page = http
        .get(format!("{}/share/{token2}", f.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("class=\"tracks\""));
    assert!(page.contains("data-src="));

    let _ = std::fs::remove_dir_all(&f.root);
}

#[tokio::test]
async fn partage_invalide_ou_expire() {
    let f = spawn().await;
    let http = reqwest::Client::new();
    let t = tracks(&f).await.remove(0);

    // ressource inconnue -> 400
    let r = http
        .post(format!("{}/api/shares", f.base))
        .json(&serde_json::json!({ "kind": "track", "id": "nope" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // lien déjà expiré -> 201 à la création mais 404 ensuite
    let r = http
        .post(format!("{}/api/shares", f.base))
        .json(&serde_json::json!({ "kind": "track", "id": t.id, "expires_in_secs": -1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let r = http
        .get(format!("{}/api/public/shares/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = http
        .get(format!("{}/share/{token}", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // token inconnu
    let r = http
        .get(format!("{}/share/0000", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    let _ = std::fs::remove_dir_all(&f.root);
}
