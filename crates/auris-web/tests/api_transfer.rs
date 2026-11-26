//! Tests d'integration des nouvelles routes: usb devices, browse tree,
//! copie entre sources, export ZIP, playlists, toggle de source.

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

struct Fixture {
    base: String,
    root: std::path::PathBuf,
}

async fn spawn_server() -> Fixture {
    let root = unique_tmp("webtransfer");
    let music = root.join("nas").join("1970").join("Album A");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 1);
    write_test_wav(&music.join("t2.wav"), 1);
    std::fs::write(music.join("cover.jpg"), b"jpeg-bytes").unwrap();
    let dst = root.join("usb");
    std::fs::create_dir_all(&dst).unwrap();

    let mut src_a = Source::new_local("nas", root.join("nas").to_str().unwrap());
    src_a.id = "nas".into();
    let mut src_b = Source::new_local("backup", dst.to_str().unwrap());
    src_b.id = "backup".into();
    let mut usb = Source::new_usb("CLE", dst.to_str().unwrap());
    usb.id = "cle".into();

    let sources = Arc::new(SourcesManager::new(vec![src_a, src_b, usb]).unwrap());
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
    Fixture {
        base: format!("http://{addr}"),
        root,
    }
}

#[tokio::test]
async fn browse_tree_usb_devices_toggle() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // browse tree recursif: 2 fichiers audio (cover.jpg exclu)
    let tree: serde_json::Value = http
        .get(format!("{}/api/browse/tree", f.base))
        .query(&[("uri", "src://nas/")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = tree.as_array().unwrap();
    assert_eq!(arr.len(), 2, "{tree}");
    assert!(arr.iter().all(|e| e["is_audio"] == true));

    // un fichier audio directement -> lui-meme
    let tree: serde_json::Value = http
        .get(format!("{}/api/browse/tree", f.base))
        .query(&[("uri", "src://nas/1970/Album A/t1.wav")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tree.as_array().unwrap().len(), 1);

    // usb devices: repond un tableau (vide ou non selon la machine)
    let devs: serde_json::Value = http
        .get(format!("{}/api/usb/devices", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(devs.is_array());

    // toggle enabled sur une source
    let r: serde_json::Value = http
        .patch(format!("{}/api/sources/nas", f.base))
        .json(&serde_json::json!({"enabled": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["enabled"], false);
    let srcs: serde_json::Value = http
        .get(format!("{}/api/sources", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nas = srcs
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "nas")
        .unwrap()
        .clone();
    assert_eq!(nas["enabled"], false);
    // toggle sur source inconnue -> 400
    let r = http
        .patch(format!("{}/api/sources/inconnu", f.base))
        .json(&serde_json::json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());

    let _ = std::fs::remove_dir_all(&f.root);
}

#[tokio::test]
async fn copie_fichier_et_dossier_entre_sources() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // copie d'un fichier vers la source "backup"
    let report: serde_json::Value = http
        .post(format!("{}/api/transfer/copy", f.base))
        .json(&serde_json::json!({
            "from": "src://nas/1970/Album A/t1.wav",
            "to": "src://backup/"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["files"], 1);
    assert!(report["bytes"].as_u64().unwrap() > 1000);
    assert!(f.root.join("usb/t1.wav").exists());

    // copie recursive d'un dossier vers la cle USB (kind usb, id "cle")
    let report: serde_json::Value = http
        .post(format!("{}/api/transfer/copy", f.base))
        .json(&serde_json::json!({
            "from": "src://nas/1970",
            "to": "src://cle/import"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["files"], 3, "{report}"); // t1.wav + t2.wav + cover.jpg
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert!(f.root.join("usb/import/1970/Album A/t2.wav").exists());
    assert!(f.root.join("usb/import/1970/Album A/cover.jpg").exists());

    // destination en lecture seule (radio) -> 400
    let radio = serde_json::json!({
        "id": "radio1", "name": "FIP", "enabled": true,
        "kind": "radio", "url": "https://example.com/fip.aac"
    });
    http.post(format!("{}/api/sources", f.base))
        .json(&radio)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let r = http
        .post(format!("{}/api/transfer/copy", f.base))
        .json(&serde_json::json!({
            "from": "src://nas/1970/Album A/t1.wav",
            "to": "src://radio1/"
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("lecture seule"));

    let _ = std::fs::remove_dir_all(&f.root);
}

#[tokio::test]
async fn export_zip_fichier_dossier_et_album() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // export d'un fichier -> ZIP avec 1 entree
    let r = http
        .get(format!("{}/api/export", f.base))
        .query(&[("uri", "src://nas/1970/Album A/t1.wav")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers().get("content-type").unwrap(), "application/zip");
    let cd = r
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("attachment"), "{cd}");
    assert!(cd.contains("t1.wav.zip"), "{cd}");
    let zip = r.bytes().await.unwrap().to_vec();
    assert!(zip.starts_with(b"PK\x03\x04"), "header local ZIP");
    assert!(
        zip.windows(4).any(|w| w == b"PK\x05\x06"),
        "EOCD present (fin d'archive ZIP)"
    );
    assert!(contains_str(&zip, "t1.wav"));

    // export d'un dossier -> ZIP avec tout le contenu (audio + cover.jpg)
    let r = http
        .get(format!("{}/api/export", f.base))
        .query(&[("uri", "src://nas/1970")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let zip = r.bytes().await.unwrap().to_vec();
    assert!(contains_str(&zip, "1970/Album A/t1.wav"), "chemin interne");
    assert!(contains_str(&zip, "1970/Album A/cover.jpg"));
    assert!(contains_str(&zip, "t2.wav"));

    // scan puis export d'album via la bibliotheque
    http.post(format!("{}/api/library/scan", f.base))
        .json(&serde_json::json!({"source_id": "nas"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{}/api/library/scan/status", f.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let albums: serde_json::Value = http
        .get(format!("{}/api/library/albums", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let album_id = albums[0]["id"].as_str().unwrap().to_string();
    let r = http
        .get(format!("{}/api/library/albums/{album_id}/export", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let zip = r.bytes().await.unwrap().to_vec();
    assert!(contains_str(&zip, "t1.wav"));
    assert!(contains_str(&zip, "t2.wav"));

    // export d'un URI invalide -> 400 JSON
    let r = http
        .get(format!("{}/api/export", f.base))
        .query(&[("uri", "nimportequoi")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());

    let _ = std::fs::remove_dir_all(&f.root);
}

#[tokio::test]
async fn playlists_crud_et_play() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // scan pour avoir des pistes
    http.post(format!("{}/api/library/scan", f.base))
        .json(&serde_json::json!({"source_id": "nas"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{}/api/library/scan/status", f.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let tracks: serde_json::Value = http
        .get(format!("{}/api/library/tracks", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<String> = tracks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);

    // creation explicite
    let p: serde_json::Value = http
        .post(format!("{}/api/playlists", f.base))
        .json(&serde_json::json!({"name": "Mix soirée", "track_ids": ids}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["name"], "Mix soirée");

    // nom vide -> 400
    let r = http
        .post(format!("{}/api/playlists", f.base))
        .json(&serde_json::json!({"name": "  "}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());

    // listage + detail (pistes resolues)
    let pls: serde_json::Value = http
        .get(format!("{}/api/playlists", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pls.as_array().unwrap().len(), 1);
    let detail: serde_json::Value = http
        .get(format!("{}/api/playlists/{pid}", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["tracks"].as_array().unwrap().len(), 2);

    // play de la playlist -> file d'attente remplie (poll)
    http.post(format!("{}/api/playlists/{pid}/play", f.base))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut qlen = 0;
    for _ in 0..100 {
        let q: serde_json::Value = http
            .get(format!("{}/api/queue", f.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        qlen = q.as_array().map(|a| a.len()).unwrap_or(0);
        if qlen == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(qlen, 2);

    // creation depuis la file courante (track_ids absent)
    let p2: serde_json::Value = http
        .post(format!("{}/api/playlists", f.base))
        .json(&serde_json::json!({"name": "Copie de la file"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(p2["track_ids"].as_array().unwrap().len(), 2);

    // suppression
    http.delete(format!("{}/api/playlists/{pid}", f.base))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let r = http
        .delete(format!("{}/api/playlists/{pid}", f.base))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    let pls: serde_json::Value = http
        .get(format!("{}/api/playlists", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pls.as_array().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(&f.root);
}

/// Cherche une sequence UTF-8 dans un buffer binaire (noms dans le ZIP).
fn contains_str(haystack: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    haystack.windows(n.len()).any(|w| w == n)
}

#[tokio::test]
async fn playlist_sauvegardee_depuis_file_hors_scan() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // joue un dossier SANS scan prealable: les pistes de la file sont ad-hoc
    http.post(format!("{}/api/player/play", f.base))
        .json(&serde_json::json!({"uri": "src://nas/1970/Album A"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut qlen = 0;
    for _ in 0..100 {
        let q: serde_json::Value = http
            .get(format!("{}/api/queue", f.base))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        qlen = q.as_array().map(|a| a.len()).unwrap_or(0);
        if qlen == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(qlen, 2);

    // sauvegarde la file -> les pistes ad-hoc doivent etre resolues au detail
    let p: serde_json::Value = http
        .post(format!("{}/api/playlists", f.base))
        .json(&serde_json::json!({"name": "Dossier non scanne"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["track_ids"].as_array().unwrap().len(), 2);
    let detail: serde_json::Value = http
        .get(format!("{}/api/playlists/{pid}", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        detail["tracks"].as_array().unwrap().len(),
        2,
        "les pistes hors scan doivent etre indexees a la sauvegarde: {detail}"
    );
}

#[tokio::test]
async fn put_source_modifie_une_source_existante() {
    let f = spawn_server().await;
    let http = reqwest::Client::new();

    // la source "nas" existe (fixture) -> on la remplace par une version renommee
    let mut body = serde_json::json!({"id": "nas", "name": "NAS renomme", "enabled": true,
        "kind": "local", "path": f.root.join("nas").to_str().unwrap()});
    let r = http
        .put(format!("{}/api/sources/nas", f.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "PUT nas: {:?}", r.status());

    let srcs: serde_json::Value = http
        .get(format!("{}/api/sources", f.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = srcs.as_array().unwrap();
    assert_eq!(arr.len(), 3, "pas de doublon apres update");
    let nas = arr.iter().find(|s| s["id"] == "nas").unwrap();
    assert_eq!(nas["name"], "NAS renomme");

    // id incoherent corps/URL -> 400
    body["id"] = serde_json::json!("autre");
    let r = http
        .put(format!("{}/api/sources/nas", f.base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());

    // source inconnue -> 400
    let r = http
        .put(format!("{}/api/sources/inconnu", f.base))
        .json(&serde_json::json!({"id": "inconnu", "name": "x", "enabled": true, "kind": "radio", "url": "http://x"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
}
