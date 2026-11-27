//! Repli ffmpeg : les formats que symphonia 0.6 ne connait pas (Opus, WMA)
//! passent par un processus ffmpeg externe. Tests ignores si ffmpeg/ffprobe
//! sont absents (CI sans ffmpeg) ; l'empreinte "repli desactive" est dans
//! `ffmpeg_fallback_off.rs` (processus separe).
//!
//! Fixtures generees avec ffmpeg :
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.5 -c:a libopus tiny.opus`
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.5 -c:a wmav2 tiny.wma`
use auris_player::ffmpeg::{ffmpeg_path, ffprobe_path};
use auris_player::{open_decoder, open_decoder_with_path, probe_meta, ChunkData, TrackMeta};
use std::fs::File;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn ffmpeg_dispo() -> bool {
    ffmpeg_path().is_some() && ffprobe_path().is_some()
}

fn check_fallback_decode(path: &PathBuf, ext: &str, expected_rate: u32, min_nonzero: usize) {
    let f = File::open(path).unwrap();
    let (_meta, spec, dur) =
        probe_meta(Box::new(f) as Box<dyn auris_sources::SeekRead>, ext).unwrap();
    assert_eq!(spec.rate, expected_rate, "rate pour {}", path.display());
    assert!(!spec.is_dsd_dop);
    assert!(dur.unwrap_or(0.0) > 0.3, "duree pour {}", path.display());
    let f = File::open(path).unwrap();
    let mut dec = open_decoder(
        Box::new(f) as Box<dyn auris_sources::SeekRead>,
        ext,
        TrackMeta::default(),
    )
    .unwrap();
    let mut nonzero = 0usize;
    let mut frames = 0usize;
    while frames < spec.rate as usize {
        match dec.next_chunk(4096).unwrap() {
            Some(ChunkData::F32(v)) => {
                nonzero += v.iter().filter(|s| s.abs() > 1e-6).count();
                frames += v.len() / spec.channels.max(1) as usize;
            }
            Some(ChunkData::I24Dop(_)) => panic!("PCM attendu, DoP recu"),
            None => break,
        }
    }
    assert!(
        nonzero >= min_nonzero,
        "audio non nul attendu ({} nonzero) pour {}",
        nonzero,
        path.display()
    );
    // seek au milieu du fichier : le decode doit reprendre sans erreur
    dec.seek(0.2).unwrap();
    match dec.next_chunk(4096).unwrap() {
        Some(ChunkData::F32(v)) => assert!(!v.is_empty()),
        other => panic!("chunk attendu apres seek, recu {:?}", other.is_some()),
    }
}

#[test]
fn opus_via_repli_ffmpeg() {
    if !ffmpeg_dispo() {
        eprintln!("ffmpeg/ffprobe absent : test ignore");
        return;
    }
    // libopus encode en 48 kHz
    check_fallback_decode(&fixture("tiny.opus"), "opus", 48000, 20000);
}

#[test]
fn wma_via_repli_ffmpeg() {
    if !ffmpeg_dispo() {
        eprintln!("ffmpeg/ffprobe absent : test ignore");
        return;
    }
    check_fallback_decode(&fixture("tiny.wma"), "wma", 44100, 20000);
}

/// Genere un WMA long (600 s) avec ffmpeg lavfi dans un dossier temporaire.
fn make_long_wma() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("auris-seek-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("long.wma");
    let status = std::process::Command::new(ffmpeg_path().unwrap())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=600",
            "-c:a",
            "wmav2",
            "-b:a",
            "128k",
        ])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "generation du fixture WMA");
    out
}

/// #54 : avec le chemin local, le seek ffmpeg est un `-ss` en entree (natif,
/// instantané) ; sans chemin, on retombe sur le pipe avec `-ss` en sortie.
#[test]
fn seek_local_path_instantane_vs_pipe() {
    if !ffmpeg_dispo() {
        eprintln!("ffmpeg/ffprobe absent : test ignore");
        return;
    }
    let long = make_long_wma();

    // --- avec path local : seek a 80 % (480 s / 600 s) en < 1 s ---
    let f = File::open(&long).unwrap();
    let mut dec = open_decoder_with_path(
        Box::new(f) as Box<dyn auris_sources::SeekRead>,
        "wma",
        TrackMeta::default(),
        Some(long.clone()),
    )
    .unwrap();
    let t0 = std::time::Instant::now();
    dec.seek(480.0).unwrap();
    let chunk = dec.next_chunk(4096).unwrap();
    let dt = t0.elapsed();
    match chunk {
        Some(ChunkData::F32(v)) => assert!(!v.is_empty(), "chunk vide apres seek"),
        other => panic!("chunk attendu apres seek, recu present={}", other.is_some()),
    }
    assert!(
        dt.as_secs_f64() < 1.0,
        "seek avec path local trop lent ({dt:?}) : le -ss devrait etre en entree"
    );

    // --- sans path : retombe sur le pipe, le seek fonctionne toujours ---
    let f = File::open(&long).unwrap();
    let shared =
        auris_player::ffmpeg::SharedReader::new(Box::new(f) as Box<dyn auris_sources::SeekRead>);
    assert!(shared.local_path().is_none(), "pas de path = mode pipe");
    let f = File::open(&long).unwrap();
    let mut dec2 = open_decoder(
        Box::new(f) as Box<dyn auris_sources::SeekRead>,
        "wma",
        TrackMeta::default(),
    )
    .unwrap();
    dec2.seek(0.2).unwrap();
    assert!(dec2.next_chunk(4096).unwrap().is_some());

    let _ = std::fs::remove_dir_all(long.parent().unwrap());
}
