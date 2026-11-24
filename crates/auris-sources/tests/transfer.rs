//! Tests d'import/export entre sources: copie de fichiers/dossiers,
//! stat, walk_audio, source USB, webradio, rejet des chemins dangereux.

use std::io::Read;

use auris_core::{Source, SourceKind};
use auris_sources::{discover_removable, SourcesManager};

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

fn build_tree() -> std::path::PathBuf {
    let root = unique_tmp("tree");
    std::fs::create_dir_all(root.join("1970/Black Sabbath/Paranoid")).unwrap();
    std::fs::create_dir_all(root.join("1970/Deep Purple")).unwrap();
    std::fs::write(
        root.join("1970/Black Sabbath/Paranoid/01 War Pigs.flac"),
        b"war-pigs-data",
    )
    .unwrap();
    std::fs::write(
        root.join("1970/Black Sabbath/Paranoid/02 Paranoid.flac"),
        b"paranoid-data!",
    )
    .unwrap();
    std::fs::write(root.join("1970/Black Sabbath/Paranoid/cover.jpg"), b"jpeg").unwrap();
    std::fs::write(root.join("1970/Deep Purple/smoke.dsf"), b"dsd-data").unwrap();
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    root
}

#[tokio::test]
async fn stat_walk_audio_et_browse() {
    let root = build_tree();
    let src = Source::new_local("nas", root.to_str().unwrap());
    let id = src.id.clone();
    let mgr = SourcesManager::new(vec![src]).unwrap();

    // stat de la racine = dossier virtuel
    let st = mgr.stat(&format!("src://{id}/")).await.unwrap();
    assert!(st.is_dir);
    assert_eq!(st.name, "nas");

    // stat d'un fichier
    let st = mgr
        .stat(&format!("src://{id}/1970/Deep Purple/smoke.dsf"))
        .await
        .unwrap();
    assert!(!st.is_dir);
    assert!(st.is_audio);
    assert_eq!(st.size, Some(8));

    // stat introuvable -> erreur
    assert!(mgr.stat(&format!("src://{id}/nope.flac")).await.is_err());

    // walk_audio ne retourne que les fichiers audio, recursif
    let audio = mgr.walk_audio(&format!("src://{id}/")).await.unwrap();
    let uris: Vec<&str> = audio.iter().map(|e| e.uri.as_str()).collect();
    assert_eq!(audio.len(), 3, "{uris:?}");
    assert!(uris.iter().any(|u| u.contains("War Pigs")));
    assert!(uris.iter().any(|u| u.contains("smoke.dsf")));
    assert!(!uris.iter().any(|u| u.contains("cover.jpg")));
    assert!(!uris.iter().any(|u| u.contains("notes.txt")));

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn copie_fichier_entre_sources_locales() {
    let a = build_tree();
    let b = unique_tmp("dst");
    std::fs::create_dir_all(&b).unwrap();
    let src_a = Source::new_local("a", a.to_str().unwrap());
    let src_b = Source::new_local("b", b.to_str().unwrap());
    let (ida, idb) = (src_a.id.clone(), src_b.id.clone());
    let mgr = SourcesManager::new(vec![src_a, src_b]).unwrap();

    let report = mgr
        .copy(
            &format!("src://{ida}/1970/Deep Purple/smoke.dsf"),
            &format!("src://{idb}/"),
        )
        .await
        .unwrap();
    assert_eq!(report.files, 1);
    assert_eq!(report.bytes, 8);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(std::fs::read(b.join("smoke.dsf")).unwrap(), b"dsd-data");

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[tokio::test]
async fn copie_dossier_recursif_vers_usb() {
    let a = build_tree();
    let usb_root = unique_tmp("usb");
    std::fs::create_dir_all(&usb_root).unwrap();
    let src_a = Source::new_local("nas", a.to_str().unwrap());
    let mut src_usb = Source::new_usb("CLE", usb_root.to_str().unwrap());
    src_usb.id = "cle-usb".into(); // id fixe pour l'URI
    let ida = src_a.id.clone();
    let mgr = SourcesManager::new(vec![src_a, src_usb]).unwrap();

    let report = mgr
        .copy(
            &format!("src://{ida}/1970/Black Sabbath"),
            "src://cle-usb/import",
        )
        .await
        .unwrap();
    assert_eq!(report.files, 3, "{:?}", report.errors); // 2 flac + cover.jpg
    assert!(report.dirs >= 2);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(usb_root
        .join("import/Black Sabbath/Paranoid/01 War Pigs.flac")
        .exists());
    assert_eq!(
        std::fs::read(usb_root.join("import/Black Sabbath/Paranoid/02 Paranoid.flac")).unwrap(),
        b"paranoid-data!"
    );

    // la source USB est bien ecrite via son backend
    let usb_backend = mgr.backend("cle-usb").unwrap();
    assert!(usb_backend.writable());
    let entries = usb_backend.list("import").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Black Sabbath");

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&usb_root);
}

#[tokio::test]
async fn copie_vers_source_lecture_seule_refusee() {
    let a = build_tree();
    let src_a = Source::new_local("nas", a.to_str().unwrap());
    let mut radio = Source::new_radio("FIP", "https://example.com/fip.aac");
    radio.id = "radio1".into();
    let ida = src_a.id.clone();
    let mgr = SourcesManager::new(vec![src_a, radio]).unwrap();

    let err = mgr
        .copy(&format!("src://{ida}/notes.txt"), "src://radio1/")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("lecture seule"), "{err}");

    let _ = std::fs::remove_dir_all(&a);
}

#[tokio::test]
async fn copie_source_identique_refusee() {
    let a = build_tree();
    let src = Source::new_local("nas", a.to_str().unwrap());
    let id = src.id.clone();
    let mgr = SourcesManager::new(vec![src]).unwrap();
    let err = mgr
        .copy(
            &format!("src://{id}/notes.txt"),
            &format!("src://{id}/notes.txt"),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("identiques"), "{err}");
    let _ = std::fs::remove_dir_all(&a);
}

#[tokio::test]
async fn chemins_dangereux_rejetes() {
    let a = build_tree();
    let secret = unique_tmp("secret");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(secret.join("secret.txt"), b"topsecret").unwrap();
    let src = Source::new_local("nas", a.to_str().unwrap());
    let id = src.id.clone();
    let mgr = SourcesManager::new(vec![src]).unwrap();

    // le ".." ne doit pas sortir de la racine de la source
    let r = mgr.browse(&format!("src://{id}/../secret")).await;
    assert!(r.is_err(), "le .. doit etre refuse: {r:?}");
    let r = mgr
        .open_uri(&format!("src://{id}/../secret/secret.txt"))
        .await;
    assert!(r.is_err());

    // ecriture hors racine refusee aussi
    let backend = mgr.backend(&id).unwrap();
    let mut data: Box<dyn Read + Send> = Box::new(&b"x"[..]);
    let r = backend
        .put(
            "../evil.txt",
            {
                let d: Box<dyn Read + Send> = Box::new(&b"x"[..]);
                d
            },
            Some(1),
        )
        .await;
    assert!(r.is_err());
    let _ = data.read(&mut [0u8; 1]);
    drop(data);
    assert!(!secret.join("evil.txt").exists());
    assert!(!a.parent().unwrap().join("evil.txt").exists());

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&secret);
}

#[tokio::test]
async fn source_radio_liste_un_flux() {
    let radio = Source::new_radio("FIP", "https://icecast.example.com/fip.aac");
    let id = radio.id.clone();
    let mgr = SourcesManager::new(vec![radio]).unwrap();
    let entries = mgr.browse(&format!("src://{id}/")).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_audio);
    assert_eq!(entries[0].uri, "https://icecast.example.com/fip.aac");
    // la lecture passe par l'URL http directe
    assert!(auris_sources::is_http_uri(&entries[0].uri));
}

#[tokio::test]
async fn source_usb_se_comporte_comme_local() {
    let root = build_tree();
    let mut usb = Source::new_usb("CLE", root.to_str().unwrap());
    usb.id = "usb1".into();
    let mgr = SourcesManager::new(vec![usb]).unwrap();
    let entries = mgr.browse("src://usb1/1970").await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Black Sabbath"));
    assert!(names.contains(&"Deep Purple"));
    // writable_sources inclut la cle USB
    let w = mgr.writable_sources();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].id, "usb1");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn usb_discovery_sans_panique() {
    // endpoint de decouverte: renvoie une liste (vide si rien n'est branche)
    let mounts = discover_removable();
    for m in &mounts {
        assert!(!m.label.is_empty());
        assert!(!m.path.is_empty());
    }
}

#[tokio::test]
async fn kind_usb_et_radio_serialisent() {
    let usb = Source {
        id: "u".into(),
        name: "CLE".into(),
        kind: SourceKind::Usb {
            path: "/media/moi/CLE".into(),
        },
        enabled: true,
    };
    let j = serde_json::to_string(&usb).unwrap();
    assert!(j.contains("\"kind\":\"usb\""), "{j}");
    let back: Source = serde_json::from_str(&j).unwrap();
    assert!(matches!(back.kind, SourceKind::Usb { .. }));
}

#[tokio::test]
async fn ajout_source_en_double_refuse() {
    let a = build_tree();
    let s1 = Source::new_local("nas", a.to_str().unwrap());
    let mut s2 = Source::new_local("autre", a.to_str().unwrap());
    s2.id = s1.id.clone();
    let mgr = SourcesManager::new(vec![s1]).unwrap();
    let err = mgr.add(s2).unwrap_err();
    assert!(err.to_string().contains("existe déjà"), "{err}");
    let _ = std::fs::remove_dir_all(&a);
}
