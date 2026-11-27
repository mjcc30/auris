//! Tests d'intégration: facettes accueil façon Nova (`/api/library/facets`),
//! filtres `?genre=`/`?decade=` sur `/api/library/tracks`, et page artiste
//! enrichie (genres, années, durée, bio).

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

fn make_track(
    uri: &str,
    artist: &str,
    album: &str,
    title: &str,
    genre: Option<&str>,
    year: Option<u32>,
    has_cover: bool,
) -> Track {
    let mut t = Track::new(format!("src://local1/{uri}"), "local1".into());
    t.artist = artist.into();
    t.album_artist = artist.into();
    t.album = album.into();
    t.title = title.into();
    t.genre = genre.map(|g| g.into());
    t.year = year;
    t.duration_secs = Some(60.0);
    t.has_cover = has_cover;
    t.finalize_keys();
    t
}

async fn spawn_server() -> (String, std::path::PathBuf, Arc<Library>, Source) {
    let root = unique_tmp("facets");
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
async fn facets_genres_et_decennies() {
    let (base, _root, library, _src) = spawn_server().await;
    library
        .upsert_track(&make_track(
            "a/1.flac",
            "Radiohead",
            "OK Computer",
            "Airbag",
            Some("Rock"),
            Some(1997),
            true,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "a/2.flac",
            "Radiohead",
            "OK Computer",
            "Paranoid Android",
            Some("Rock; Alternative"),
            Some(1997),
            false,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "b/1.flac",
            "Miles Davis",
            "Kind of Blue",
            "So What",
            Some("Jazz"),
            Some(1959),
            true,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "b/2.flac",
            "Miles Davis",
            "Bitches Brew",
            "Pharaoh's Dance",
            Some("Jazz"),
            Some(1970),
            false,
        ))
        .unwrap();

    let http = reqwest::Client::new();
    let facets: serde_json::Value = http
        .get(format!("{base}/api/library/facets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let genres = facets["genres"].as_array().unwrap();
    let rock = genres
        .iter()
        .find(|g| g["name"] == "Rock")
        .expect("genre Rock");
    assert_eq!(rock["track_count"], 2);
    assert_eq!(rock["album_count"], 1);
    assert!(rock["cover_url"]
        .as_str()
        .unwrap()
        .contains("/api/library/cover/"));
    let alt = genres
        .iter()
        .find(|g| g["name"] == "Alternative")
        .expect("genre Alternative (split ;)");
    assert_eq!(alt["track_count"], 1);
    // tri par nombre de pistes décroissant : Rock/Jazz (2) avant Alternative (1)
    assert_eq!(genres[0]["name"], "Jazz"); // ex-aequo 2/2, tri alpha: Jazz < Rock
    assert!(genres
        .iter()
        .any(|g| g["name"] == "Jazz" && g["track_count"] == 2));

    let decades = facets["decades"].as_array().unwrap();
    let d90 = decades
        .iter()
        .find(|d| d["decade"] == 1990)
        .expect("décennie 1990");
    assert_eq!(d90["track_count"], 2);
    let d50 = decades
        .iter()
        .find(|d| d["decade"] == 1950)
        .expect("décennie 1950");
    assert_eq!(d50["track_count"], 1);
    let d70 = decades
        .iter()
        .find(|d| d["decade"] == 1970)
        .expect("décennie 1970");
    assert_eq!(d70["track_count"], 1);
    // triDécroissant : 1990 en premier
    assert_eq!(decades[0]["decade"], 1990);
}

#[tokio::test]
async fn tracks_filtres_genre_et_decennie() {
    let (base, _root, library, _src) = spawn_server().await;
    library
        .upsert_track(&make_track(
            "a/1.flac",
            "Radiohead",
            "OK Computer",
            "Airbag",
            Some("Rock"),
            Some(1997),
            true,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "b/1.flac",
            "Miles Davis",
            "Kind of Blue",
            "So What",
            Some("Jazz"),
            Some(1959),
            true,
        ))
        .unwrap();
    library
        .upsert_track(&make_track(
            "c/1.flac",
            "Daft Punk",
            "Discovery",
            "One More Time",
            Some("Electronic"),
            Some(2001),
            false,
        ))
        .unwrap();

    let http = reqwest::Client::new();
    let all: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let rock: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks?genre=rock")) // insensible à la casse
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rock.len(), 1);
    assert_eq!(rock[0]["title"], "Airbag");

    let d90: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks?decade=1990"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(d90.len(), 1);
    assert_eq!(d90[0]["title"], "Airbag");

    let both: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks?genre=Jazz&decade=1950"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(both.len(), 1);

    let none: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/tracks?genre=Metal&decade=1990"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(none.len(), 0);

    // decade invalide = ignoré (pas d'erreur 500)
    let res = http
        .get(format!("{base}/api/library/tracks?decade=abc"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn artiste_enrichi_champs_genres_annees_bio() {
    let (base, _root, _library, src) = spawn_server().await;
    let http = reqwest::Client::new();
    // scan du wav de test pour avoir un artiste indexé
    let scan_res = http
        .post(format!("{base}/api/library/scan"))
        .json(&serde_json::json!({ "source_id": src.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_res.status(), 200, "scan: {:?}", scan_res.text().await);
    for _ in 0..200 {
        let tracks: Vec<serde_json::Value> = http
            .get(format!("{base}/api/library/tracks"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !tracks.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let artists: Vec<serde_json::Value> = http
        .get(format!("{base}/api/library/artists"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(artists.len(), 1);
    let id = artists[0]["id"].as_str().unwrap();
    let detail: serde_json::Value = http
        .get(format!("{base}/api/library/artists/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // nouveaux champs de la page artiste enrichie
    assert!(detail.get("genres").is_some());
    assert!(detail.get("year_min").is_some());
    assert!(detail.get("year_max").is_some());
    assert!(detail.get("duration_secs").is_some());
    assert!(detail.get("bio").is_some());
    assert!(detail["duration_secs"].as_f64().unwrap() > 0.0);
    assert_eq!(detail["albums"].as_array().unwrap().len(), 1);
}
