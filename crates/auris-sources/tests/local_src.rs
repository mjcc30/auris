//! Tests de la source locale (listage, walk, ouverture) — auris-sources.

use std::io::Read;

use auris_core::{Source, SourceKind};
use auris_sources::local::{walk_audio_files, LocalSource};
use auris_sources::SourceBackend;

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
async fn local_list_walk_open() {
    let root = unique_tmp("local");
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::write(root.join("a").join("b.flac"), b"flac-data").unwrap();
    std::fs::write(root.join("a").join("c.txt"), b"texte").unwrap();
    std::fs::write(root.join("d.wav"), b"wav-data").unwrap();

    let src = Source::new_local("t", root.to_str().unwrap());
    let backend = LocalSource::new(src).unwrap();

    // racine: dossier "a" + fichier audio "d.wav"
    let root_entries = backend.list("").await.unwrap();
    let names: Vec<&str> = root_entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"d.wav"));
    let d = root_entries.iter().find(|e| e.name == "d.wav").unwrap();
    assert!(d.is_audio && !d.is_dir);
    assert_eq!(d.size, Some(8));

    // sous-dossier: b.flac audio, c.txt non audio
    let a_entries = backend.list("a").await.unwrap();
    let b = a_entries.iter().find(|e| e.name == "b.flac").unwrap();
    assert!(b.is_audio);
    let c = a_entries.iter().find(|e| e.name == "c.txt").unwrap();
    assert!(!c.is_audio);

    // walk recursif ne garde que les fichiers audio
    let files = walk_audio_files(&root);
    assert_eq!(files.len(), 2);

    // open + read
    let mut r = backend.open("d.wav").await.unwrap();
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"wav-data");

    // source invalide (kind smb passé à new local) -> erreur
    let bad = Source {
        id: "x".into(),
        name: "x".into(),
        kind: SourceKind::Smb {
            host: "h".into(),
            share: "s".into(),
            user: "u".into(),
            password: "p".into(),
            base_path: "".into(),
        },
        enabled: true,
    };
    assert!(LocalSource::new(bad).is_err());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn local_path_renseigne_pour_source_locale() {
    let root = unique_tmp("lp");
    std::fs::create_dir_all(&root).unwrap();
    let src = Source::new_local("t", root.to_str().unwrap());
    let backend = LocalSource::new(src).unwrap();
    // chemin relatif résolu sous la racine
    assert_eq!(backend.local_path("a/b.flac"), Some(root.join("a/b.flac")));
    // sortie de racine refusée -> None
    assert_eq!(backend.local_path("../escape"), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn local_path_for_uri_manager() {
    let root = unique_tmp("lpm");
    std::fs::create_dir_all(&root).unwrap();
    let src = Source::new_local("t", root.to_str().unwrap());
    let id = src.id.clone();
    let mgr = auris_sources::SourcesManager::new(vec![src]).unwrap();
    // src:// local -> Some(chemin absolu)
    let uri = auris_sources::make_uri(&id, "x.opus");
    assert_eq!(mgr.local_path_for_uri(&uri), Some(root.join("x.opus")));
    // http / source inconnue / uri invalide -> None (retombe sur le pipe, #54)
    assert_eq!(mgr.local_path_for_uri("http://example.com/x.opus"), None);
    assert_eq!(mgr.local_path_for_uri("src://inconnue/x.opus"), None);
    assert_eq!(mgr.local_path_for_uri("pas-un-uri"), None);
    let _ = std::fs::remove_dir_all(&root);
}
