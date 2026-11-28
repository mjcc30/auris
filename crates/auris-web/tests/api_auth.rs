//! Token d'accès optionnel (#97) : sans token tout est ouvert ; avec token,
//! l'API exige un cookie de session, un Bearer ou ?access_token=, les liens
//! de partage restent publics et le token n'est jamais renvoyé en clair.

use std::sync::Arc;

use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{router, AppState};

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

async fn spawn_server(token: Option<&str>) -> (String, AppState) {
    let root = unique_tmp("webauth");
    std::fs::create_dir_all(root.join("music")).unwrap();
    let src = auris_core::Source::new_local("local1", root.join("music").to_str().unwrap());
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
    if let Some(t) = token {
        state.settings.write().unwrap().system.access_token = Some(t.to_string());
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let st = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(st)).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn sans_token_tout_ouvert() {
    let (base, _state) = spawn_server(None).await;
    let http = reqwest::Client::new();
    let st: serde_json::Value = http
        .get(format!("{base}/api/auth/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["required"], false);
    assert_eq!(st["authenticated"], true);
    let resp = http.get(format!("{base}/api/state")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn avec_token_api_protegee_et_login() {
    let (base, _state) = spawn_server(Some("motdepasse-solide-1")).await;
    let http = reqwest::Client::new();

    // API verrouillée sans auth
    let resp = http.get(format!("{base}/api/state")).send().await.unwrap();
    assert_eq!(resp.status(), 401);

    // status public
    let st: serde_json::Value = http
        .get(format!("{base}/api/auth/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["required"], true);
    assert_eq!(st["authenticated"], false);

    // mauvais token -> 401
    let resp = http
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "token": "faux" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // bon token -> 200 + cookie de session
    let resp = http
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "token": "motdepasse-solide-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("auris_session="));

    // cookie -> API ouverte
    let resp = http
        .get(format!("{base}/api/state"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Bearer -> API ouverte
    let resp = http
        .get(format!("{base}/api/state"))
        .bearer_auth("motdepasse-solide-1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // ?access_token= -> API ouverte (websocket navigateur)
    let resp = http
        .get(format!("{base}/api/state?access_token=motdepasse-solide-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // lien de partage public : pas de 401
    let resp = http
        .get(format!("{base}/api/public/shares/inexistant"))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 401);

    // le token n'est jamais renvoyé en clair par GET /api/settings
    let s: serde_json::Value = http
        .get(format!("{base}/api/settings"))
        .bearer_auth("motdepasse-solide-1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["system"]["access_token"], "********");

    // PUT avec "********" = inchangé ; avec "" = suppression
    let mut body = s.clone();
    body["system"]["access_token"] = serde_json::json!("********");
    let resp = http
        .put(format!("{base}/api/settings"))
        .bearer_auth("motdepasse-solide-1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = http
        .get(format!("{base}/api/state"))
        .bearer_auth("motdepasse-solide-1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "le token doit être conservé");

    let mut body = s.clone();
    body["system"]["access_token"] = serde_json::json!("");
    let resp = http
        .put(format!("{base}/api/settings"))
        .bearer_auth("motdepasse-solide-1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // token supprimé -> plus de protection
    let resp = http.get(format!("{base}/api/state")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn logout_invalide_la_session() {
    let (base, _state) = spawn_server(Some("motdepasse-solide-2")).await;
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "token": "motdepasse-solide-2" }))
        .send()
        .await
        .unwrap();
    let cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let resp = http
        .get(format!("{base}/api/state"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = http
        .post(format!("{base}/api/auth/logout"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = http
        .get(format!("{base}/api/state"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
