//! Formats PCM supportes: WavPack (.wv, via symphonia-codec-wavpack) et CAF
//! (necessite byte_len sur le MediaSource). Fixtures generees avec ffmpeg :
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.2 -c:a wavpack tiny.wv`
//! `ffmpeg -f lavfi -i sine=frequency=440:duration=0.2 -c:a pcm_s16le tiny.caf`
use auris_player::{open_decoder, probe_meta, ChunkData, TrackMeta};
use std::fs::File;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn check_decode_ok(path: &PathBuf, ext: &str, min_nonzero: usize) {
    let f = File::open(path).unwrap();
    let (meta, spec, dur) =
        probe_meta(Box::new(f) as Box<dyn auris_sources::SeekRead>, ext).unwrap();
    assert_eq!(spec.rate, 44100, "rate pour {}", path.display());
    assert!(dur.unwrap_or(0.0) > 0.1, "duree pour {}", path.display());
    let f = File::open(path).unwrap();
    let mut dec = open_decoder(
        Box::new(f) as Box<dyn auris_sources::SeekRead>,
        ext,
        TrackMeta::default(),
    )
    .unwrap();
    assert_eq!(dec.spec().rate, meta.format.map(|_| spec.rate).unwrap_or(0));
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
}

#[test]
fn wavpack_16bit_mono_probe_et_decode() {
    check_decode_ok(&fixture("tiny.wv"), "wv", 8000);
}

#[test]
fn caf_probe_et_decode() {
    // CAF exige byte_len() sur le MediaSource (sinon "unexpected end of file").
    check_decode_ok(&fixture("tiny.caf"), "caf", 8000);
}
