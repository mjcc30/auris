//! Test d'integration de la bibliotheque: scan d'une source locale (WAV),
//! indexation, recherche, albums/artistes.

use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_sources::SourcesManager;

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

#[tokio::test]
async fn scan_index_recherche() {
    let root = unique_tmp("libscan");
    let album_dir = root.join("music").join("Album A");
    std::fs::create_dir_all(&album_dir).unwrap();
    write_test_wav(&album_dir.join("t1.wav"), 2);
    write_test_wav(&album_dir.join("t2.wav"), 3);
    std::fs::write(album_dir.join("notes.txt"), "pas de l'audio").unwrap();

    let src = Source::new_local("test", root.join("music").to_str().unwrap());
    let mgr = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let lib = Library::open(&root.join("db"), mgr).unwrap();
    assert_eq!(lib.track_count(), 0);

    lib.scan_source(&src.id).unwrap();
    for _ in 0..200 {
        let st = lib.scan_status();
        if !st.running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let st = lib.scan_status();
    assert!(!st.running, "scan toujours en cours");
    assert_eq!(st.errors, 0, "erreurs de scan: {:?}", st.last_error);

    // 2 wav indexés (le .txt est ignoré), durees ~2 s et ~3 s
    assert_eq!(lib.track_count(), 2, "tracks: {:?}", lib.tracks());
    let mut durs: Vec<u32> = lib
        .tracks()
        .iter()
        .map(|t| t.duration_secs.unwrap() as u32)
        .collect();
    durs.sort_unstable();
    assert_eq!(durs, vec![2, 3]);

    // recherche
    let found = lib.search("t1");
    assert_eq!(found.len(), 1);
    assert!(found[0].uri.ends_with("t1.wav"));
    assert!(lib.search("zzzz").is_empty());

    // album / artiste (metadonnees absentes -> "Album inconnu" / "Artiste inconnu")
    let albums = lib.albums();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].track_count, 2);
    let artists = lib.artists();
    assert_eq!(artists.len(), 1);

    // get_by_uri
    let uri = format!("src://{}/Album A/t2.wav", src.id);
    let t = lib.get_by_uri(&uri).expect("track by uri");
    assert!(t.uri.ends_with("t2.wav"));
    assert!(matches!(
        t.format.unwrap(),
        auris_core::AudioFormat::Pcm { .. }
    ));

    // re-scan idempotent: pas de doublons
    lib.scan_source(&src.id).unwrap();
    for _ in 0..200 {
        if !lib.scan_status().running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(lib.track_count(), 2, "le re-scan ne doit pas doublonner");

    let _ = std::fs::remove_dir_all(&root);
}
