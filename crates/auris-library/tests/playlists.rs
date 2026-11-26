//! Tests des listes de lecture (creation, lecture avec pistes, suppression).

use std::sync::Arc;

use auris_core::Source;
use auris_library::Library;
use auris_sources::SourcesManager;

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

#[tokio::test]
async fn playlists_crud_avec_pistes_reelles() {
    let root = unique_tmp("pl");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 1);
    write_test_wav(&music.join("t2.wav"), 1);

    let src = Source::new_local("test", music.to_str().unwrap());
    let mgr = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let lib = Library::open(&root.join("db"), mgr).unwrap();
    lib.scan_source(&src.id).unwrap();
    for _ in 0..200 {
        if !lib.scan_status().running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let tracks = lib.tracks();
    assert_eq!(tracks.len(), 2);
    let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();

    // creation
    let p = lib.create_playlist("Ma soirée", ids.clone()).unwrap();
    assert!(!p.id.is_empty());
    assert_eq!(p.track_ids.len(), 2);

    // nom vide refuse
    assert!(lib.create_playlist("   ", vec![]).is_err());

    // listage
    let pls = lib.playlists();
    assert_eq!(pls.len(), 1);
    assert_eq!(pls[0].name, "Ma soirée");

    // lecture: pistes resolues dans l'ordre
    let (p2, t2) = lib.playlist(&p.id).unwrap();
    assert_eq!(p2.name, "Ma soirée");
    assert_eq!(t2.len(), 2);
    assert_eq!(t2[0].id, ids[0]);

    // les IDs inconnus sont ignores a la lecture
    let mut with_ghost = ids.clone();
    with_ghost.push("inconnu".into());
    let p3 = lib.create_playlist("avec fantome", with_ghost).unwrap();
    let (_, t3) = lib.playlist(&p3.id).unwrap();
    assert_eq!(t3.len(), 2);

    // suppression
    assert!(lib.delete_playlist(&p.id).unwrap());
    assert!(!lib.delete_playlist(&p.id).unwrap());
    assert_eq!(lib.playlists().len(), 1);

    // playlist inconnue -> None
    assert!(lib.playlist("nope").is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn add_to_playlist_ajoute_sans_doublon() {
    let root = unique_tmp("pladd");
    let music = root.join("music");
    std::fs::create_dir_all(&music).unwrap();
    write_test_wav(&music.join("t1.wav"), 1);
    write_test_wav(&music.join("t2.wav"), 1);

    let src = Source::new_local("test", music.to_str().unwrap());
    let mgr = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let lib = Library::open(&root.join("db"), mgr).unwrap();
    lib.scan_source(&src.id).unwrap();
    for _ in 0..200 {
        if !lib.scan_status().running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let ids: Vec<String> = lib.tracks().iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids.len(), 2);

    let p = lib.create_playlist("Mix", vec![ids[0].clone()]).unwrap();

    // ajout de la 2e piste + doublon de la 1re (ignore)
    let p2 = lib
        .add_to_playlist(&p.id, vec![ids[1].clone(), ids[0].clone()])
        .unwrap()
        .unwrap();
    assert_eq!(p2.track_ids, vec![ids[0].clone(), ids[1].clone()]);

    // la playlist persiste avec les 2 pistes resolues
    let (p3, t3) = lib.playlist(&p.id).unwrap();
    assert_eq!(p3.track_ids.len(), 2);
    assert_eq!(t3.len(), 2);

    // playlist inconnue -> Ok(None)
    assert!(lib.add_to_playlist("nope", ids.clone()).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&root);
}
