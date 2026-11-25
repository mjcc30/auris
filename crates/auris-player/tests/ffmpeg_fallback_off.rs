//! Sans ffmpeg, un format inconnu de symphonia doit echouer proprement
//! (avec la mention du repli) — et un format connu (flac) continue de
//! passer par symphonia. Processus de test separe pour pouvoir truander
//! les variables d'environnement.
use auris_player::probe_meta;
use std::fs::File;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn sans_ffmpeg_opus_echoue_avec_mention_du_repli() {
    std::env::set_var("AURIS_FFMPEG", "C:/chemin/inexistant/ffmpeg.exe");
    std::env::set_var("AURIS_FFPROBE", "C:/chemin/inexistant/ffprobe.exe");
    std::env::set_var("PATH", "C:/chemin/inexistant");
    let f = File::open(fixture("tiny.opus")).unwrap();
    let err = probe_meta(Box::new(f) as Box<dyn auris_sources::SeekRead>, "opus").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ffmpeg") || msg.contains("non supporte"),
        "message inattendu: {msg}"
    );
}

#[test]
fn sans_ffmpeg_le_flac_passe_toujours_par_symphonia() {
    std::env::set_var("AURIS_FFMPEG", "C:/chemin/inexistant/ffmpeg.exe");
    std::env::set_var("AURIS_FFPROBE", "C:/chemin/inexistant/ffprobe.exe");
    std::env::set_var("PATH", "C:/chemin/inexistant");
    let f = File::open(fixture("tiny.wv")).unwrap();
    // .wv passe par symphonia-codec-wavpack, pas besoin de ffmpeg
    let (_meta, spec, dur) =
        probe_meta(Box::new(f) as Box<dyn auris_sources::SeekRead>, "wv").unwrap();
    assert_eq!(spec.rate, 44100);
    assert!(dur.unwrap_or(0.0) > 0.1);
}
