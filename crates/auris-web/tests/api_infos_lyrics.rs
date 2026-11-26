//! Tests d'intégration: panneau « lecture en cours » (Pochette / Infos / Paroles).
//! - GET /api/lyrics/{track_id} : repli lrclib.net (fetcher HTTP injecté) avec
//!   paroles synchronisées LRC parsées en [[secs, ligne], ...].
//! - GET /api/infos/{track_id} : bio Wikipedia (fetcher injecté) mise en cache
//!   dans sled (2e appel -> source "cache", aucun nouvel appel HTTP).
//! - id inconnu -> 400 {"error": ...}.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use auris_core::Source;
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

/// Faux fetcher HTTP: lrclib renvoie des paroles LRC, Wikipedia une bio.
/// `wiki_calls` compte les appels Wikipedia (pour vérifier le cache sled).
fn fake_http(wiki_calls: Arc<AtomicUsize>) -> auris_web::ExtHttpGet {
    Arc::new(move |url: &str| {
        if url.contains("lrclib.net/api/get") {
            return Some(
                r#"{"syncedLyrics":"[00:01.00]Hello\n[00:05.50]World","plainLyrics":"Hello\nWorld"}"#
                    .to_string(),
            );
        }
        if url.contains("lrclib.net/api/search") {
            return Some("[]".to_string());
        }
        if url.contains("wikipedia.org") {
            wiki_calls.fetch_add(1, Ordering::Relaxed);
            return Some(r#"{"extract":"Bio test de l'artiste."}"#.to_string());
        }
        None
    })
}

async fn spawn_server() -> (String, String, Arc<AtomicUsize>) {
    let root = unique_tmp("np");
    let dir = root.join("music/1970/Test");
    std::fs::create_dir_all(&dir).unwrap();
    write_test_wav(&dir.join("01 Test.wav"), 1);

    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
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
    let wiki_calls = Arc::new(AtomicUsize::new(0));
    let mut state = AppState::new(engine, library, sources, updater, root.clone());
    state.ext_http = Some(fake_http(wiki_calls.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), src.id, wiki_calls)
}

async fn wait_scan_done(http: &reqwest::Client, base: &str, source_id: &str) {
    http.post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({"source_id": source_id}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for _ in 0..200 {
        let st: serde_json::Value = http
            .get(format!("{base}/api/library/scan/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["running"] == false {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("scan jamais termine");
}

async fn first_track(http: &reqwest::Client, base: &str) -> serde_json::Value {
    let tracks: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tracks.len(), 1);
    tracks.into_iter().next().unwrap()
}

#[tokio::test]
async fn paroles_via_lrclib_avec_lrc_synchronise() {
    let (base, src_id, _wiki) = spawn_server().await;
    let http = reqwest::Client::new();
    wait_scan_done(&http, &base, &src_id).await;
    let track = first_track(&http, &base).await;
    let id = track["id"].as_str().unwrap().to_string();

    // le WAV de test n'a pas de tag USLT -> repli lrclib.net
    let l: serde_json::Value = http
        .get(format!("{base}/api/lyrics/{id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(l["source"], "lrclib");
    assert_eq!(l["plain"], "Hello\nWorld");
    let synced = l["synced"].as_array().unwrap();
    assert_eq!(synced.len(), 2);
    assert_eq!(synced[0][0].as_f64().unwrap(), 1.0);
    assert_eq!(synced[0][1], "Hello");
    assert_eq!(synced[1][0].as_f64().unwrap(), 5.5);
    assert_eq!(synced[1][1], "World");

    // id inconnu -> 400 {"error": ...}
    let resp = http
        .get(format!(
            "{base}/api/lyrics/00000000-0000-0000-0000-000000000000"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("introuvable"));
}

#[tokio::test]
async fn infos_avec_bio_wikipedia_cachee_dans_sled() {
    let (base, src_id, wiki_calls) = spawn_server().await;
    let http = reqwest::Client::new();
    wait_scan_done(&http, &base, &src_id).await;
    let track = first_track(&http, &base).await;
    let id = track["id"].as_str().unwrap().to_string();

    let i: serde_json::Value = http
        .get(format!("{base}/api/infos/{id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(i["title"], "01 Test.wav");
    assert_eq!(i["bio"], "Bio test de l'artiste.");
    assert_eq!(i["bio_source"], "wikipedia");
    // WAV sans tags: album repli "Album inconnu" (le scan ne lit que les tags)
    assert_eq!(i["album_info"]["title"], "Album inconnu");
    let calls_after_first = wiki_calls.load(Ordering::Relaxed);
    assert!(calls_after_first >= 1, "Wikipedia a du être appelé");

    // 2e appel: bio servie depuis le cache sled, pas d'appel HTTP supplémentaire
    let i2: serde_json::Value = http
        .get(format!("{base}/api/infos/{id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(i2["bio"], "Bio test de l'artiste.");
    assert_eq!(i2["bio_source"], "cache");
    assert_eq!(
        wiki_calls.load(Ordering::Relaxed),
        calls_after_first,
        "le cache sled doit éviter un nouvel appel Wikipedia"
    );

    // id inconnu -> 400
    let resp = http
        .get(format!(
            "{base}/api/infos/00000000-0000-0000-0000-000000000000"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
