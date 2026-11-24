//! Tests des liens de partage (creation, lecture, expiration, revocation).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use auris_core::{ShareKind, Source};
use auris_library::Library;
use auris_sources::SourcesManager;

fn unique_tmp(tag: &str) -> std::path::PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("auris-{tag}-{}-{nanos}-{n}", std::process::id()))
}

fn library(root: &std::path::Path) -> Library {
    let src = Source::new_local("local1", root.to_str().unwrap());
    let sources = Arc::new(SourcesManager::new(vec![src]).unwrap());
    Library::open(root, sources).unwrap()
}

#[test]
fn shares_crud() {
    let root = unique_tmp("shares");
    std::fs::create_dir_all(&root).unwrap();
    let lib = library(&root);

    assert!(lib.shares().is_empty());
    assert!(lib.share("inconnu").is_none());
    assert!(!lib.delete_share("inconnu").unwrap());

    let s = lib
        .create_share(ShareKind::Track, "t1", "War Pigs", None)
        .unwrap();
    assert_eq!(s.token.len(), 32);
    assert!(s.token.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(s.kind, ShareKind::Track);
    assert!(s.expires_at.is_none());

    let got = lib.share(&s.token).expect("lien valide");
    assert_eq!(got.title, "War Pigs");

    let all = lib.shares();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].token, s.token);

    assert!(lib.delete_share(&s.token).unwrap());
    assert!(lib.share(&s.token).is_none());
    assert!(lib.shares().is_empty());

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn share_expire_est_purge() {
    let root = unique_tmp("shares-exp");
    std::fs::create_dir_all(&root).unwrap();
    let lib = library(&root);

    // expire dans 1 seconde puis attend pour depasser l'expiration
    let s = lib
        .create_share(ShareKind::Album, "a1", "Album", Some(1))
        .unwrap();
    assert!(s.expires_at.is_some());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(lib.share(&s.token).is_none(), "lien expire = None");
    // la purge a eu lieu lors du share()
    assert!(lib.shares().is_empty(), "le lien expire est purge");

    // expiration deja passee a la creation
    let s2 = lib
        .create_share(ShareKind::Playlist, "p1", "Playlist", Some(-5))
        .unwrap();
    assert!(lib.share(&s2.token).is_none());

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tokens_uniques() {
    let root = unique_tmp("shares-uniq");
    std::fs::create_dir_all(&root).unwrap();
    let lib = library(&root);
    let a = lib.create_share(ShareKind::Track, "t", "A", None).unwrap();
    let b = lib.create_share(ShareKind::Track, "t", "A", None).unwrap();
    assert_ne!(a.token, b.token);
    assert_eq!(lib.shares().len(), 2);
    std::fs::remove_dir_all(&root).ok();
}
