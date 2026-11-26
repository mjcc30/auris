//! Tests d'intégration: Parcourir façon Volumio — filtrage des non-audio,
//! lecture récursive d'un dossier, « Lire ensuite », ajout à une playlist.

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

/// Dossier racine avec a.wav + notes.txt (non audio) + sous-dossier sub/ (b.wav).
async fn spawn_server() -> (String, std::path::PathBuf, Source) {
    let root = unique_tmp("browseact");
    let music = root.join("music");
    let sub = music.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    write_test_wav(&music.join("a.wav"), 1);
    write_test_wav(&music.join("c.wav"), 1);
    write_test_wav(&sub.join("b.wav"), 1);
    std::fs::write(music.join("notes.txt"), b"pas de l'audio").unwrap();
    std::fs::write(sub.join("cover.jpg"), b"\xff\xd8\xff\xe0 not really").unwrap();

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

#[tokio::test]
async fn browse_filtre_les_non_audio_mais_garde_dossiers() {
    let (base, root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    let entries: serde_json::Value = http
        .get(format!("{base}/api/browse"))
        .query(&[("uri", format!("src://{}/", src.id))])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = entries.as_array().unwrap();
    let names: Vec<&str> = arr.iter().map(|e| e["name"].as_str().unwrap()).collect();
    // notes.txt absent, dossier sub + fichiers audio présents
    assert!(
        !names.contains(&"notes.txt"),
        "fichier non audio visible: {names:?}"
    );
    assert!(names.contains(&"sub"), "dossier absent: {names:?}");
    assert!(names.contains(&"a.wav") && names.contains(&"c.wav"));
    assert!(arr
        .iter()
        .all(|e| e["is_dir"].as_bool().unwrap() || e["is_audio"].as_bool().unwrap()));

    // le sous-dossier ne montre que b.wav (cover.jpg caché)
    let entries: serde_json::Value = http
        .get(format!("{base}/api/browse"))
        .query(&[("uri", format!("src://{}/sub", src.id))])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = entries.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "b.wav");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn play_dossier_recursif_remplit_la_file() {
    let (base, root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // « Tout lire » sur la racine: a.wav + c.wav + sub/b.wav (récursif)
    http.post(format!("{base}/api/player/play"))
        .json(&serde_json::json!({ "uri": format!("src://{}/", src.id) }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // la commande est asynchrone: on attend que la file soit remplie
    let mut q: Vec<serde_json::Value> = Vec::new();
    for _ in 0..100 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        q = st["queue"].as_array().cloned().unwrap_or_default();
        if q.len() == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(q.len(), 3, "file recursive: {q:?}");
    assert!(q
        .iter()
        .any(|t| t["uri"].as_str().unwrap().ends_with("sub/b.wav")));

    // « Lire ensuite » insère juste après la piste en cours (index 0 -> position 1)
    http.post(format!("{base}/api/queue/add-next"))
        .json(&serde_json::json!({ "uri": format!("src://{}/c.wav", src.id) }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut q: Vec<serde_json::Value> = Vec::new();
    for _ in 0..100 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        q = st["queue"].as_array().cloned().unwrap_or_default();
        if q.len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(q.len(), 4);
    assert!(
        q[1]["uri"].as_str().unwrap().ends_with("c.wav"),
        "lire ensuite doit inserer en position 1: {q:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn playlist_add_uri_et_creation_depuis_uri() {
    let (base, root, src) = spawn_server().await;
    let http = reqwest::Client::new();

    // création de playlist directement depuis un dossier (récursif)
    let p: serde_json::Value = http
        .post(format!("{base}/api/playlists"))
        .json(&serde_json::json!({ "name": "Dossier", "uri": format!("src://{}/", src.id) }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(p["name"], "Dossier");
    assert_eq!(p["track_ids"].as_array().unwrap().len(), 3);
    let pid = p["id"].as_str().unwrap().to_string();

    // ajout d'un fichier à la playlist existante
    let r: serde_json::Value = http
        .post(format!("{base}/api/playlists/{pid}/add"))
        .json(&serde_json::json!({ "uri": format!("src://{}/c.wav", src.id) }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // c.wav était déjà dans le dossier: pas de doublon, added compte les ids envoyés
    assert_eq!(r["added"], 1);
    let detail: serde_json::Value = http
        .get(format!("{base}/api/playlists/{pid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // 3 pistes uniques malgré l'ajout d'un doublon
    assert_eq!(detail["playlist"]["track_ids"].as_array().unwrap().len(), 3);
    assert_eq!(detail["tracks"].as_array().unwrap().len(), 3);

    // playlist inconnue -> 4xx
    let resp = http
        .post(format!("{base}/api/playlists/nope/add"))
        .json(&serde_json::json!({ "uri": format!("src://{}/a.wav", src.id) }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_client_error());

    let _ = std::fs::remove_dir_all(&root);
}
