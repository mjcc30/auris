//! Tests d'intégration: recherche groupée par type + filtre source (#71) —
//! `GET /api/library/search?q=&source=` retourne `{tracks, albums, artists, playlists}`
//! avec `total` + `items` plafonnés.

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

async fn spawn_server() -> (String, Arc<Library>, Source, Source) {
    let root = unique_tmp("search");
    let music_a = root.join("musicA");
    let music_b = root.join("musicB");
    std::fs::create_dir_all(&music_a).unwrap();
    std::fs::create_dir_all(&music_b).unwrap();
    write_test_wav(&music_a.join("alpha.wav"), 1);
    write_test_wav(&music_a.join("commun.wav"), 1);
    write_test_wav(&music_b.join("beta.wav"), 1);
    write_test_wav(&music_b.join("commun.wav"), 1);
    let src_a = Source::new_local("NAS-A", music_a.to_str().unwrap());
    let src_b = Source::new_local("NAS-B", music_b.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src_a.clone(), src_b.clone()]).unwrap());
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
    (format!("http://{addr}"), library, src_a, src_b)
}

async fn scan(http: &reqwest::Client, base: &str, source_id: &str, expected: usize) {
    let r = http
        .post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({ "source_id": source_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    for _ in 0..200 {
        let n: serde_json::Value = http
            .get(format!("{base}/api/library/tracks"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if n.as_array().unwrap().len() >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("scan incomplet");
}

#[tokio::test]
async fn recherche_groupee_par_type_et_filtre_source() {
    let (base, library, src_a, src_b) = spawn_server().await;
    let http = reqwest::Client::new();
    scan(&http, &base, &src_a.id, 2).await;
    scan(&http, &base, &src_b.id, 4).await;

    // playlists: une mixte (A+B), une uniquement B
    let tracks = library.tracks();
    let id_a = tracks
        .iter()
        .find(|t| t.source_id == src_a.id && t.title.contains("alpha"))
        .unwrap()
        .id
        .clone();
    let id_b = tracks
        .iter()
        .find(|t| t.source_id == src_b.id && t.title.contains("beta"))
        .unwrap()
        .id
        .clone();
    library
        .create_playlist("Ma sélection", vec![id_a.clone(), id_b.clone()])
        .unwrap();
    library
        .create_playlist("Seulement B", vec![id_b.clone()])
        .unwrap();

    // 1. recherche globale "commun" -> 2 pistes (une par source), groupées
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "commun")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["tracks"]["total"], 2);
    assert_eq!(r["tracks"]["items"].as_array().unwrap().len(), 2);
    // WAV sans tags -> album "Album inconnu" (commun aux 4 pistes, une seule clé)
    assert_eq!(r["albums"]["total"], 0);
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "album")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["albums"]["total"], 1);
    assert_eq!(r["albums"]["items"][0]["title"], "Album inconnu");
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "artiste")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["artists"]["total"], 1);
    assert_eq!(r["artists"]["items"][0]["name"], "Artiste inconnu");

    // 2. filtre source A -> 1 seule piste, celle de A
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "commun"), ("source", src_a.id.as_str())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["tracks"]["total"], 1);
    assert_eq!(r["tracks"]["items"][0]["source_id"], src_a.id);

    // 3. recherche "alpha" -> piste + album, pas de playlist
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "alpha")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["tracks"]["total"], 1);
    assert_eq!(r["albums"]["total"], 0);
    assert_eq!(r["playlists"]["total"], 0);

    // 4. playlists: "sélection" matche la mixte; filtrée par A elle reste (contient une piste A)
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "sélection")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["playlists"]["total"], 1);
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", ""), ("source", src_a.id.as_str())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // q vide -> toutes les playlists visibles, mais seules celles avec une piste de A
    assert_eq!(r["playlists"]["total"], 1);
    assert_eq!(r["playlists"]["items"][0]["name"], "Ma sélection");

    // 5. filtre source B -> les 2 playlists
    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", ""), ("source", src_b.id.as_str())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["playlists"]["total"], 2);
}

#[tokio::test]
async fn recherche_plafonnee_et_tri_pertinence() {
    let (base, library, _a, _b) = spawn_server().await;
    let http = reqwest::Client::new();
    let mk = |uri: &str, title: &str| {
        let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
        t.title = title.into();
        t.artist = "Divers".into();
        t.album_artist = "Divers".into();
        t.album = "Divers".into();
        t.finalize_keys();
        library.upsert_track(&t).unwrap();
        t
    };
    // 35 pistes "capNN" via upsert (index pistes seul suffit pour search_grouped)
    for i in 0..35 {
        mk(&format!("cap{i}.flac"), &format!("cap{i:02}"));
    }
    // pertinence: préfixe de titre (rang 0) avant simple contenu (rang 3)
    mk("xalpha.flac", "xalpha");
    mk("alphabet.flac", "alphabet");

    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "cap")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["tracks"]["total"], 35);
    // items plafonnés à 30
    assert_eq!(r["tracks"]["items"].as_array().unwrap().len(), 30);

    let r: serde_json::Value = http
        .get(format!("{base}/api/library/search"))
        .query(&[("q", "alpha")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = r["tracks"]["items"].as_array().unwrap();
    assert_eq!(r["tracks"]["total"], 2);
    assert_eq!(items[0]["title"], "alphabet");
    assert_eq!(items[1]["title"], "xalpha");
}
