//! Tests d'intégration: endpoints packs d'interface (`/api/packs*`).
//!
//! Couvre le listing (thèmes réservés + pack embarqué `aurora`, langues fr/en),
//! le dépôt d'un pack communautaire dans `<data-dir>/themes|locales`, la
//! récupération des fichiers (data-dir prioritaire sur l'embarqué), et le
//! rejet des noms de fichiers invalides (traversée de chemin -> 400, inconnu -> 404).

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
    let root = unique_tmp("packs");
    std::fs::create_dir_all(root.join("music")).unwrap();
    let src = Source::new_local("local1", root.join("music").to_str().unwrap());
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
    let state = AppState::new(engine, library, sources, updater, root.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{addr}"), root)
}

#[tokio::test]
async fn packs_liste_builtins() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();
    let packs: serde_json::Value = http
        .get(format!("{base}/api/packs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let themes = packs["themes"].as_array().unwrap();
    let theme_ids: Vec<&str> = themes.iter().map(|t| t["id"].as_str().unwrap()).collect();
    // thèmes réservés gérés par data-theme
    for id in ["dark", "light", "system"] {
        assert!(
            theme_ids.contains(&id),
            "thème réservé {id} absent de {theme_ids:?}"
        );
    }
    // pack embarqué dans l'UI (ui/dist/themes/aurora.css)
    assert!(
        theme_ids.contains(&"aurora"),
        "pack embarqué aurora absent de {theme_ids:?}"
    );
    assert!(themes
        .iter()
        .any(|t| t["id"] == "aurora" && t["builtin"] == true));

    let locales = packs["locales"].as_array().unwrap();
    let locale_ids: Vec<&str> = locales.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert!(locale_ids.contains(&"fr") && locale_ids.contains(&"en"));
}

#[tokio::test]
async fn packs_theme_embarque_servi() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();
    let res = http
        .get(format!("{base}/api/packs/theme/aurora.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let ct = res
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.starts_with("text/css"), "content-type inattendu: {ct}");
    let body = res.text().await.unwrap();
    assert!(
        body.contains(":root"),
        "le pack aurora doit définir des variables CSS"
    );
}

#[tokio::test]
async fn packs_locale_embarquee_servie() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();
    let res = http
        .get(format!("{base}/api/packs/locale/en.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let j: serde_json::Value = res.json().await.unwrap();
    assert_eq!(j["Radios"], "Radios");
    assert_eq!(j["Réglages"], "Settings");
}

#[tokio::test]
async fn packs_communautaires_depuis_data_dir() {
    let (base, root) = spawn_server().await;
    // dépose un thème + une langue communautaires
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::create_dir_all(root.join("locales")).unwrap();
    std::fs::write(
        root.join("themes/mon-theme.css"),
        ":root { --accent: #ff0000; }\n",
    )
    .unwrap();
    std::fs::write(root.join("locales/de.json"), r#"{ "Radios": "Radios" }"#).unwrap();

    let http = reqwest::Client::new();

    // listés comme non builtin
    let packs: serde_json::Value = http
        .get(format!("{base}/api/packs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let theme = packs["themes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "mon-theme")
        .unwrap()
        .clone();
    assert_eq!(theme["builtin"], false);
    let locale = packs["locales"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == "de")
        .unwrap()
        .clone();
    assert_eq!(locale["builtin"], false);

    // servis depuis le data-dir
    let res = http
        .get(format!("{base}/api/packs/theme/mon-theme.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("--accent: #ff0000"));

    let res = http
        .get(format!("{base}/api/packs/locale/de.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let j: serde_json::Value = res.json().await.unwrap();
    assert_eq!(j["Radios"], "Radios");

    // le data-dir a priorité sur l'embarqué : on écrase le pack en.json
    std::fs::write(root.join("locales/en.json"), r#"{ "Radios": "Sender" }"#).unwrap();
    let res = http
        .get(format!("{base}/api/packs/locale/en.json"))
        .send()
        .await
        .unwrap();
    let j: serde_json::Value = res.json().await.unwrap();
    assert_eq!(
        j["Radios"], "Sender",
        "le pack du data-dir doit primer sur l'embarqué"
    );
}

#[tokio::test]
async fn packs_noms_invalides_rejetes() {
    let (base, _root) = spawn_server().await;
    let http = reqwest::Client::new();

    // traversée de chemin -> 400 (le nom n'est pas assaini)
    let res = http
        .get(format!("{base}/api/packs/theme/..%2F..%2Fconfig.toml.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        400,
        "traversée de chemin: attendu 400, reçu {}",
        res.status()
    );

    // caractères interdits -> 400
    let res = http
        .get(format!("{base}/api/packs/theme/evil%20name.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // pack inconnu -> 404
    let res = http
        .get(format!("{base}/api/packs/theme/inexistant.css"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    let res = http
        .get(format!("{base}/api/packs/locale/xx.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}
