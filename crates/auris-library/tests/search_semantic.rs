//! Test d'integration de la recherche sémantique embarquée (#38) :
//! correspondance floue (fautes), expansion par humeur, ranking pondéré par champ.

use std::sync::Arc;

use auris_core::{Source, Track};
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

fn mk_track(
    uri: &str,
    source: &str,
    title: &str,
    artist: &str,
    album: &str,
    genre: Option<&str>,
) -> Track {
    let mut t = Track::new(uri.into(), source.into());
    t.title = title.into();
    t.artist = artist.into();
    t.album = album.into();
    t.album_artist = artist.into();
    t.album_key = format!("{}|{}", album.to_lowercase(), artist.to_lowercase());
    t.artist_key = artist.to_lowercase();
    t.genre = genre.map(str::to_string);
    t
}

#[test]
fn search_grouped_semantique() {
    let root = unique_tmp("libsearch");
    std::fs::create_dir_all(&root).unwrap();
    let src = Source::new_local("s1", root.to_str().unwrap());
    let mgr = Arc::new(SourcesManager::new(vec![src.clone()]).unwrap());
    let lib = Library::open(&root.join("db"), mgr).unwrap();

    lib.upsert_track(&mk_track(
        "src://s1/a.flac",
        "s1",
        "Nuit Blanche",
        "Éther",
        "Ambient One",
        Some("Ambient"),
    ))
    .unwrap();
    lib.upsert_track(&mk_track(
        "src://s1/b.flac",
        "s1",
        "Beetles Medley",
        "The Beatles",
        "Abbey Road",
        Some("Rock"),
    ))
    .unwrap();
    lib.upsert_track(&mk_track(
        "src://s1/c.flac",
        "s1",
        "Radio Ga Ga",
        "Queen",
        "The Works",
        Some("Pop"),
    ))
    .unwrap();

    // 1. Recherche exacte : le titre exact remonte en premier.
    let res = lib.search_grouped("Radio Ga Ga", None);
    assert_eq!(
        res.tracks.items.first().map(|t| t.title.as_str()),
        Some("Radio Ga Ga")
    );
    assert_eq!(res.tracks.total, 1);

    // 2. Faute de frappe tolérée par trigrammes : "beetles" retrouve "Beetles Medley".
    let res = lib.search_grouped("beetles", None);
    assert!(
        res.tracks.items.iter().any(|t| t.title == "Beetles Medley"),
        "tracks: {:?}",
        res.tracks
            .items
            .iter()
            .map(|t| &t.title)
            .collect::<Vec<_>>()
    );

    // 3. Expansion d'humeur : "calme" retrouve la piste de genre Ambient
    //    même si aucun champ ne contient le mot "calme".
    let res = lib.search_grouped("calme", None);
    assert!(
        res.tracks.items.iter().any(|t| t.title == "Nuit Blanche"),
        "tracks: {:?}",
        res.tracks
            .items
            .iter()
            .map(|t| &t.title)
            .collect::<Vec<_>>()
    );
    assert!(!res.tracks.items.iter().any(|t| t.title == "Beetles Medley"));

    // 4. Ranking pondéré : une requête "queen" retrouve la piste via le champ artiste.
    let res = lib.search_grouped("queen", None);
    assert!(res.tracks.items.iter().any(|t| t.artist == "Queen"));

    // 5. Requête sans aucun match.
    let res = lib.search_grouped("xylophone nucléaire", None);
    assert_eq!(res.tracks.total, 0);
    assert_eq!(res.albums.total, 0);

    // 6. Le groupe radios existe (rempli côté web, vide ici).
    assert_eq!(res.radios.total, 0);

    let _ = std::fs::remove_dir_all(&root);
}
