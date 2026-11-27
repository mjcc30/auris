//! Tests d'intégration: Last.fm (auth + test + signature) et MusicBrainz (#26).
//! Un mini-serveur axum joue le rôle de ws.audioscrobbler.com et de
//! musicbrainz.org ; les signatures api_sig sont revérifiées côté test.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auris_core::{LastfmSettings, MusicBrainzSettings, Source, Track};
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

const API_KEY: &str = "0123456789abcdef0123456789abcdef";
const SECRET: &str = "s3cr3t-test";

/// Recalcule la signature Last.fm attendue pour une requête capturée.
fn expected_sig(params: &HashMap<String, String>, secret: &str) -> String {
    use md5::{Digest, Md5};
    let mut sorted: BTreeMap<&String, &String> = BTreeMap::new();
    for (k, v) in params {
        if k == "format" || k == "callback" || k == "api_sig" {
            continue;
        }
        sorted.insert(k, v);
    }
    let mut h = Md5::new();
    for (k, v) in sorted {
        h.update(k.as_bytes());
        h.update(v.as_bytes());
    }
    h.update(secret.as_bytes());
    format!("{:x}", h.finalize())
}

/// Mini-serveur Last.fm : capture les appels POST form et répond selon `method`.
async fn spawn_fake_lastfm() -> (String, Arc<Mutex<Vec<HashMap<String, String>>>>) {
    use axum::{body::Bytes, Router};
    let calls: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::new(Mutex::new(Vec::new()));
    let c = calls.clone();
    let app = Router::new().route(
        "/",
        axum::routing::post(move |body: Bytes| {
            let c = c.clone();
            async move {
                let text = String::from_utf8_lossy(&body).to_string();
                let params: HashMap<String, String> = text
                    .split('&')
                    .filter_map(|kv| {
                        kv.split_once('=')
                            .map(|(k, v)| (k.to_string(), urldecode(v)))
                    })
                    .collect();
                let method = params.get("method").cloned().unwrap_or_default();
                // vérifie la signature reçue
                let sig_ok = params
                    .get("api_sig")
                    .map(|s| *s == expected_sig(&params, SECRET))
                    .unwrap_or(false);
                c.lock().unwrap().push(params.clone());
                let body = match method.as_str() {
                    "auth.getToken" => r#"{"token":"tok-123"}"#.to_string(),
                    "auth.getSession" => {
                        r#"{"session":{"name":"testeur","key":"sk-abc-789","subscriber":"0"}}"#
                            .to_string()
                    }
                    "user.getInfo" => {
                        r#"{"user":{"name":"testeur","playcount":"4242","url":"https://www.last.fm/user/testeur"}}"#
                            .to_string()
                    }
                    m if m.starts_with("track.") => format!(r#"{{"{m}":{{}}}}"#),
                    _ => r#"{"error":3,"message":"méthode inconnue"}"#.to_string(),
                };
                assert!(sig_ok, "api_sig invalide pour {method}: {params:?}");
                axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/"), calls)
}

fn urldecode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
            i += 3;
        } else if b[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Mini-serveur MusicBrainz WS2.
async fn spawn_fake_mb() -> String {
    use axum::{extract::Path, Router};
    let app = Router::new()
        .route(
            "/recording/{mbid}",
            axum::routing::get(move |Path(mbid): Path<String>| async move {
                assert_eq!(mbid, "mbid-rec-1");
                axum::Json(serde_json::json!({
                    "id": "mbid-rec-1",
                    "title": "Airbag",
                    "length": "287000",
                    "artist-credit": [{"name": "Radiohead"}],
                    "release-group": {
                        "title": "OK Computer",
                        "first-release-date": "1997-06-16",
                        "label-info": [{"label": {"name": "Parlophone"}}]
                    },
                    "tags": [{"name": "rock"}, {"name": "alternative rock"}],
                    "isrcs": ["GBAYE9700038"],
                    "rating": {"value": 4.6},
                    "rating-count": 900
                }))
            }),
        )
        .route(
            "/recording/",
            axum::routing::get(|| async move {
                axum::Json(serde_json::json!({
                    "recordings": [{
                        "id": "mbid-rec-1",
                        "title": "Airbag",
                        "artist-credit": [{"name": "Radiohead"}],
                        "release-group": {"title": "OK Computer", "first-release-date": "1997"},
                        "tags": [{"name": "rock"}],
                        "isrcs": ["GBAYE9700038"]
                    }]
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_server(
    tag: &str,
    lf_url: Option<String>,
    mb_url: Option<String>,
) -> (String, std::path::PathBuf, Arc<Library>, AppState, String) {
    let root = unique_tmp(tag);
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 2);
    std::fs::write(root.join("config.toml"), "port = 3000\n").unwrap();
    let src = Source::new_local("local1", music.to_str().unwrap());
    let src_id = src.id.clone();
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
    let mut state = AppState::new(engine, library.clone(), sources, updater, root.clone());
    // pas de bio Wikipedia dans les tests
    state.ext_http = Some(Arc::new(|_| None));
    {
        let mut s = state.settings.write().unwrap();
        s.lastfm = LastfmSettings {
            enabled: true,
            api_key: API_KEY.into(),
            shared_secret: SECRET.into(),
            session_key: "sk-abc-789".into(),
            api_url: lf_url,
        };
        s.musicbrainz = MusicBrainzSettings {
            enabled: true,
            api_url: mb_url,
        };
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn({
        let st = state.clone();
        async move {
            axum::serve(listener, router(st)).await.unwrap();
        }
    });
    (format!("http://{addr}"), root, library, state, src_id)
}

fn make_track(
    src_id: &str,
    uri: &str,
    artist: &str,
    album: &str,
    title: &str,
    mbid: Option<&str>,
) -> Track {
    let mut t = Track::new(format!("src://{src_id}/{uri}"), src_id.into());
    t.artist = artist.into();
    t.album_artist = artist.into();
    t.album = album.into();
    t.title = title.into();
    t.duration_secs = Some(287.0);
    t.mb_track_id = mbid.map(str::to_string);
    t.finalize_keys();
    t
}

#[tokio::test]
async fn lastfm_auth_flow_test_et_signatures() {
    let (lf_url, calls) = spawn_fake_lastfm().await;
    let (base, root, _lib, _st, _sid) = spawn_server("lf", Some(lf_url), None).await;
    let http = reqwest::Client::new();

    // non configuré -> 400 (on écrase les réglages avec des champs vides)
    // (ici configuré, donc on teste le flux nominal)

    // 1. token
    let r: serde_json::Value = http
        .post(format!("{base}/api/lastfm/token"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["token"], "tok-123");
    assert!(r["auth_url"].as_str().unwrap().contains("api_key="));
    assert!(r["auth_url"].as_str().unwrap().contains("token=tok-123"));

    // 2. session
    let r: serde_json::Value = http
        .post(format!("{base}/api/lastfm/session"))
        .json(&serde_json::json!({ "token": "tok-123" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["session_key"], "sk-abc-789");
    assert_eq!(r["name"], "testeur");

    // 3. test de connexion (user.getInfo, nécessite la session)
    let r: serde_json::Value = http
        .post(format!("{base}/api/lastfm/test"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(r["name"], "testeur");
    assert_eq!(r["playcount"], "4242");

    // toutes les requêtes capturées avaient une api_sig valide (assert dans le
    // handler) et les bons paramètres
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["method"], "auth.getToken");
    assert_eq!(calls[0]["api_key"], API_KEY);
    assert!(
        !calls[0].contains_key("sk"),
        "getToken ne doit pas envoyer sk"
    );
    assert_eq!(calls[1]["method"], "auth.getSession");
    assert_eq!(calls[1]["token"], "tok-123");
    assert_eq!(calls[2]["method"], "user.getInfo");
    assert_eq!(calls[2]["sk"], "sk-abc-789");
    for c in calls.iter() {
        assert_eq!(c["format"], "json");
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn lastfm_non_configure_renvoie_400() {
    let (base, root, _lib, st, _sid) = spawn_server("lf-off", None, None).await;
    {
        let mut s = st.settings.write().unwrap();
        s.lastfm.api_key = String::new();
        s.lastfm.shared_secret = String::new();
    }
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{base}/api/lastfm/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("api_key"));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn musicbrainz_enrichit_api_infos() {
    let mb_url = spawn_fake_mb().await;
    let (base, root, lib, st, sid) = spawn_server("mb", None, Some(mb_url)).await;
    let t = make_track(
        &sid,
        "t1.wav",
        "Radiohead",
        "OK Computer",
        "Airbag",
        Some("mbid-rec-1"),
    );
    lib.upsert_track(&t).unwrap();
    let http = reqwest::Client::new();

    let r: serde_json::Value = http
        .get(format!("{base}/api/infos/{}", t.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mb = &r["musicbrainz"];
    assert_eq!(mb["mbid"], "mbid-rec-1");
    assert_eq!(mb["artist"], "Radiohead");
    assert_eq!(mb["release"], "OK Computer");
    assert_eq!(mb["release_date"], "1997-06-16");
    assert_eq!(mb["label"], "Parlophone");
    assert_eq!(mb["isrcs"][0], "GBAYE9700038");
    assert_eq!(mb["tags"].as_array().unwrap().len(), 2);
    assert_eq!(mb["rating"], 4.6);
    assert!(mb["url"].as_str().unwrap().contains("mbid-rec-1"));

    // deuxième appel : servi par le cache (toujours OK, même contenu)
    let r2: serde_json::Value = http
        .get(format!("{base}/api/infos/{}", t.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r2["musicbrainz"]["mbid"], "mbid-rec-1");

    // piste sans MBID -> recherche artist+titre (fallback)
    let t2 = make_track(&sid, "t1.wav", "Radiohead", "OK Computer", "Airbag", None);
    lib.upsert_track(&t2).unwrap();
    let r3: serde_json::Value = http
        .get(format!("{base}/api/infos/{}", t2.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r3["musicbrainz"]["mbid"], "mbid-rec-1");

    // MusicBrainz désactivé -> null
    st.settings.write().unwrap().musicbrainz.enabled = false;
    let r4: serde_json::Value = http
        .get(format!("{base}/api/infos/{}", t.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r4["musicbrainz"].is_null());

    let _ = std::fs::remove_dir_all(&root);
}
