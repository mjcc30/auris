//! Tests d'intégration: endpoints radios web (`/api/radios*`).
//!
//! Aucun test ne lance de lecture réelle (pas de réseau externe) : on couvre
//! le listing (intégrées + source radio de la config), l'ajout/suppression de
//! favoris (persistés dans `<data-dir>/radios.json`), la validation d'URL et
//! l'erreur propre sur un id inconnu.

use std::sync::atomic::{AtomicU64, Ordering};
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

async fn spawn_server() -> (String, std::path::PathBuf) {
    let root = unique_tmp("radios");
    std::fs::create_dir_all(root.join("music")).unwrap();
    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
    let radio = Source::new_radio("Ma Radio", "https://example.com/flux.mp3");
    let sources = Arc::new(SourcesManager::new(vec![src, radio]).unwrap());
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
    (format!("http://{addr}"), root)
}

#[tokio::test]
async fn radios_list_builtin_et_source() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();
    let list: serde_json::Value = http
        .get(format!("{base}/api/radios"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list["builtin"].as_array().unwrap().len() >= 3);
    assert_eq!(list["favorites"].as_array().unwrap().len(), 0);
    let sources = list["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["name"], "Ma Radio");
    assert_eq!(sources[0]["url"], "https://example.com/flux.mp3");
    assert_eq!(sources[0]["origin"], "source");
    assert!(sources[0]["id"].as_str().unwrap().starts_with("src-"));
    let b = &list["builtin"][0];
    assert_eq!(b["origin"], "builtin");
    assert!(b["id"].as_str().unwrap().starts_with("builtin-"));
}

#[tokio::test]
async fn radios_crud_favoris_persistes() {
    let (base, root) = spawn_server().await;
    let http = reqwest::Client::new();

    // ajout
    let list: serde_json::Value = http
        .post(format!("{base}/api/radios"))
        .json(&serde_json::json!({ "name": "Test Radio", "url": "https://ice.test/stream" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let favs = list["favorites"].as_array().unwrap();
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0]["name"], "Test Radio");
    assert_eq!(favs[0]["origin"], "favorite");
    let id = favs[0]["id"].as_str().unwrap().to_string();

    // persistance : le fichier radios.json existe et survit à un re-GET
    assert!(root.join("radios.json").exists());
    let list2: serde_json::Value = http
        .get(format!("{base}/api/radios"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list2["favorites"].as_array().unwrap().len(), 1);

    // doublon d'URL refusé
    let dup = http
        .post(format!("{base}/api/radios"))
        .json(&serde_json::json!({ "name": "Autre", "url": "https://ice.test/stream" }))
        .send()
        .await
        .unwrap();
    assert!(!dup.status().is_success());

    // URL non http refusée
    let bad = http
        .post(format!("{base}/api/radios"))
        .json(&serde_json::json!({ "name": "X", "url": "ftp://x" }))
        .send()
        .await
        .unwrap();
    assert!(!bad.status().is_success());

    // suppression
    let list3: serde_json::Value = http
        .delete(format!("{base}/api/radios/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list3["favorites"].as_array().unwrap().len(), 0);

    // re-suppression -> erreur
    let again = http
        .delete(format!("{base}/api/radios/{id}"))
        .send()
        .await
        .unwrap();
    assert!(!again.status().is_success());

    // une radio intégrée n'est pas supprimable en favori
    let builtin = http
        .delete(format!("{base}/api/radios/builtin-fip"))
        .send()
        .await
        .unwrap();
    assert!(!builtin.status().is_success());

    // lecture d'un id inconnu -> erreur propre
    let play = http
        .post(format!("{base}/api/radios/builtin-inconnu/play"))
        .send()
        .await
        .unwrap();
    assert!(!play.status().is_success());
    let body: serde_json::Value = play.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("introuvable"));
}

#[tokio::test]
async fn radios_play_url_validation_et_reponse() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();

    // URL non http(s) refusée
    let bad = http
        .post(format!("{base}/api/radios/play-url"))
        .json(&serde_json::json!({ "name": "X", "url": "ftp://x/stream" }))
        .send()
        .await
        .unwrap();
    assert!(!bad.status().is_success());

    // nom vide refusé
    let noname = http
        .post(format!("{base}/api/radios/play-url"))
        .json(&serde_json::json!({ "name": "  ", "url": "https://ice.test/stream" }))
        .send()
        .await
        .unwrap();
    assert!(!noname.status().is_success());

    // URL valide : 200 + la piste radio finit dans la file (moteur async -> polling)
    let ok = http
        .post(format!("{base}/api/radios/play-url"))
        .json(&serde_json::json!({ "name": "Radio Test", "url": "https://ice.test/stream" }))
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success());
    // la piste est dans la file (moteur async -> polling) ; `current` n'est
    // renseigné qu'après ouverture du flux, impossible ici sans réseau.
    let mut st: serde_json::Value = serde_json::Value::Null;
    for _ in 0..40 {
        st = http
            .get(format!("{base}/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if st["queue"]
            .as_array()
            .map(|q| q.len() == 1)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let q = st["queue"].as_array().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0]["title"], "Radio Test");
    assert_eq!(q[0]["uri"], "https://ice.test/stream");
}

#[tokio::test]
async fn radios_dir_stations_kind_inconnu_refuse() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();

    // kind inconnu -> 400 (pas d'appel réseau à l'annuaire)
    let bad = http
        .get(format!("{base}/api/radios/dir/stations?kind=bogus"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status().as_u16(), 400);

    // genre/pays/recherche sans value -> 400
    for kind in ["genre", "country", "search"] {
        let r = http
            .get(format!("{base}/api/radios/dir/stations?kind={kind}"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status().as_u16(),
            400,
            "kind={kind} sans value doit échouer"
        );
    }
}
