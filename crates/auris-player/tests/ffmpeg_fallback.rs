//! Repli ffmpeg : les formats que symphonia 0.6 ne connait pas (Opus, WMA)
//! passent par un processus ffmpeg externe. Tests ignores si ffmpeg/ffprobe
//! sont absents (CI sans ffmpeg) ; l'empreinte "repli desactive" est dans
//! `ffmpeg_fallback_off.rs` (processus separe).
//!
//! Fixtures generees avec ffmpeg :
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.5 -c:a libopus tiny.opus`
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.5 -c:a wmav2 tiny.wma`
use auris_player::ffmpeg::{ffmpeg_path, ffprobe_path};
use auris_player::{open_decoder, probe_meta, ChunkData, TrackMeta};
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
