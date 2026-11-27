//! Test d'intégration du serveur MPD v1 (#80) : protocole ligne par ligne sur
//! port TCP dédié, piloté ici via `dispatch_for_test` + un vrai socket TCP.

use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_player::PlayerEngine;
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{mpd, AppState};

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

async fn spawn_state() -> (AppState, std::path::PathBuf, Source) {
    let root = unique_tmp("mpd");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 2);
    write_test_wav(&music.join("t2.wav"), 3);

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
    (state, root, src)
}

async fn scan(state: &AppState, src: &Source) {
    state
        .library
        .scan_source(&src.id)
        .expect("scan de la source de test");
    for _ in 0..200 {
        if state.library.stats().tracks >= 2 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("scan incomplet: {:?}", state.library.stats());
}

async fn cmd(state: &AppState, line: &str) -> (String, bool) {
    mpd::dispatch_for_test(state, line).await
}

/// Attend que la file atteigne `n` pistes (l'enqueue est asynchrone).
async fn wait_queue(state: &AppState, n: usize) {
    for _ in 0..100 {
        if state.engine.state().queue.len() == n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("queue non remplie: {}", state.engine.state().queue.len());
}

#[tokio::test]
async fn mpd_protocole_et_commandes() {
    let (state, _root, src) = spawn_state().await;
    scan(&state, &src).await;
    assert!(state.library.stats().tracks >= 2);

    // status initial
    let (out, ok) = cmd(&state, "status").await;
    assert!(ok, "status: {out}");
    assert!(out.contains("playlistlength: 0"), "{out}");
    assert!(out.contains("state: stop"), "{out}");
    assert!(out.contains("volume: 80"), "{out}");

    // add x2 -> playlistinfo
    let (out, ok) = cmd(&state, "add \"src://local1/t1.wav\"").await;
    assert!(ok, "{out}");
    let (out, ok) = cmd(&state, "addid \"src://local1/t2.wav\"").await;
    assert!(ok, "{out}");
    assert!(out.contains("Id: "), "addid doit renvoyer un Id: {out}");
    wait_queue(&state, 2).await;

    let (out, ok) = cmd(&state, "playlistinfo").await;
    assert!(ok);
    assert_eq!(out.matches("file: src://local1/t").count(), 2, "{out}");
    assert!(out.contains("Pos: 0"), "{out}");
    assert!(out.contains("Pos: 1"), "{out}");

    // volume
    let (_, ok) = cmd(&state, "setvol 30").await;
    assert!(ok);
    let (out, ok) = cmd(&state, "status").await;
    assert!(ok);
    assert!(out.contains("volume: 30"), "{out}");
    let (_, ok) = cmd(&state, "setvol 150").await;
    assert!(!ok, "setvol 150 doit echouer");

    // lecture (sans DAC en CI la lecture echoue mais la file avance)
    let (_, ok) = cmd(&state, "play 0").await;
    assert!(ok);
    let mut out = String::new();
    for _ in 0..100 {
        let (o, ok) = cmd(&state, "status").await;
        assert!(ok);
        out = o;
        if out.contains("song: 0") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(out.contains("song: 0"), "{out}");
    assert!(out.contains("playlistlength: 2"), "{out}");

    // currentsong
    let (out, ok) = cmd(&state, "currentsong").await;
    assert!(ok);
    assert!(out.contains("file: src://local1/t1.wav"), "{out}");

    // pause / stop / next / previous / clear
    for c in ["pause 1", "pause 0", "next", "previous", "seek 0 0.5"] {
        let (_, ok) = cmd(&state, c).await;
        assert!(ok, "{c} doit reussir");
    }
    let (_, ok) = cmd(&state, "stop").await;
    assert!(ok);
    let mut out = String::new();
    for _ in 0..100 {
        let (o, ok) = cmd(&state, "status").await;
        assert!(ok);
        out = o;
        if out.contains("state: stop") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(out.contains("state: stop"), "{out}");

    // delete + clear (avec attente, le clear est asynchrone)
    let (_, ok) = cmd(&state, "delete 0").await;
    assert!(ok);
    wait_queue(&state, 1).await;
    let (out, ok) = cmd(&state, "status").await;
    assert!(ok);
    assert!(out.contains("playlistlength: 1"), "{out}");
    let (_, ok) = cmd(&state, "clear").await;
    assert!(ok);
    wait_queue(&state, 0).await;
    let (out, _) = cmd(&state, "status").await;
    assert!(out.contains("playlistlength: 0"), "{out}");

    // outputs (2 sorties : DAC local + navigateur)
    let (out, ok) = cmd(&state, "outputs").await;
    assert!(ok);
    assert_eq!(out.matches("outputid:").count(), 2, "{out}");

    // recherche / list depuis la bibliotheque scannee
    let (out, ok) = cmd(&state, "search title wav").await;
    assert!(ok);
    assert!(out.contains("file: "), "{out}");
    let (out, ok) = cmd(&state, "list album").await;
    assert!(ok, "{out}");
    let (out, ok) = cmd(&state, "lsinfo").await;
    assert!(ok);
    assert!(out.contains("directory: local1"), "{out}");

    // commandes diverses
    for c in [
        "ping",
        "commands",
        "notcommands",
        "tagtypes",
        "urlhandlers",
        "stats",
        "idle",
        "noidle",
    ] {
        let (_, ok) = cmd(&state, c).await;
        assert!(ok, "{c} doit reussir");
    }
    let (out, ok) = cmd(&state, "commands").await;
    assert!(ok && out.contains("command: play"), "{out}");

    // commande inconnue -> ACK (ok == false)
    let (_, ok) = cmd(&state, "foobar").await;
    assert!(!ok, "foobar doit echouer");
}

#[tokio::test]
async fn mpd_socket_tcp_greeting_et_ok() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (state, _root, _src) = spawn_state().await;

    // port ephemere libre
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);

    let st = state.clone();
    tokio::spawn(async move { mpd::serve(st, port).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();

    // salutation
    let greeting = lines.next_line().await.unwrap().unwrap();
    assert_eq!(greeting, "OK MPD 0.24.0");

    // status -> paires + OK
    wr.write_all(b"status\n").await.unwrap();
    let mut buf = String::new();
    loop {
        let l = lines.next_line().await.unwrap().unwrap();
        if l == "OK" {
            break;
        }
        buf.push_str(&l);
        buf.push('\n');
    }
    assert!(buf.contains("state: stop"), "{buf}");
    assert!(buf.contains("playlistlength: 0"), "{buf}");

    // commande inconnue -> ACK
    wr.write_all(b"foobar\n").await.unwrap();
    let ack = loop {
        let l = lines.next_line().await.unwrap().unwrap();
        if l.starts_with("ACK [") {
            break l;
        }
    };
    assert!(ack.contains("{foobar}"), "{ack}");

    // ping -> OK ; close -> deconnexion
    wr.write_all(b"ping\n").await.unwrap();
    let l = lines.next_line().await.unwrap().unwrap();
    assert_eq!(l, "OK");
    wr.write_all(b"close\n").await.unwrap();
    // close repond OK puis coupe la connexion
    let mut saw_ok = false;
    for _ in 0..5 {
        match lines.next_line().await.unwrap() {
            None => break,
            Some(l) if l == "OK" => saw_ok = true,
            Some(_) => {}
        }
    }
    assert!(saw_ok, "close doit repondre OK avant de couper");
    assert!(
        lines.next_line().await.unwrap().is_none(),
        "close doit terminer la connexion"
    );
}
