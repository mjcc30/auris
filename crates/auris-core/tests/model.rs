//! Tests du modele de donnees (auris-core).

use auris_core::{is_audio_file, AudioFormat, Source, SourceKind, Track};

#[test]
fn audio_extensions_reconnues() {
    assert!(is_audio_file("nas/rock/track01.FLAC"));
    assert!(is_audio_file("dsd/album.dsf"));
    assert!(is_audio_file("a/b/c.DFF"));
    assert!(is_audio_file("x.mp3"));
    assert!(!is_audio_file("notes.txt"));
    assert!(!is_audio_file("cover.jpg"));
    assert!(!is_audio_file("sans_extension"));
}

#[test]
fn dsd_multiplier_valide() {
    assert_eq!(AudioFormat::dsd_multiplier(44100 * 64), Some(1));
    assert_eq!(AudioFormat::dsd_multiplier(44100 * 128), Some(2));
    assert_eq!(AudioFormat::dsd_multiplier(44100 * 256), Some(4));
    assert_eq!(AudioFormat::dsd_multiplier(44100 * 512), Some(8));
    assert_eq!(AudioFormat::dsd_multiplier(48000 * 64), None);
    assert_eq!(AudioFormat::dsd_multiplier(44100), None);
    assert_eq!(AudioFormat::dsd_multiplier(0), None);
}

#[test]
fn format_label() {
    let pcm = AudioFormat::Pcm {
        sample_rate: 44100,
        bits: 16,
        channels: 2,
    };
    assert_eq!(pcm.label(), "16 bit / 44 kHz / 2 ch");
    let dsd = AudioFormat::Dsd {
        sample_rate: 44100 * 256,
        channels: 2,
    };
    assert_eq!(dsd.label(), "DSD256 / 2 ch");
}

#[test]
fn source_local_json_roundtrip() {
    let s = Source::new_local("NAS", "/mnt/nas/musique");
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"kind\":\"local\""), "kind tagge: {json}");
    let back: Source = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "NAS");
    assert!(back.enabled);
    match &back.kind {
        SourceKind::Local { path } => assert_eq!(path, "/mnt/nas/musique"),
        other => panic!("kind inattendu: {other:?}"),
    }
}

#[test]
fn source_smb_json_roundtrip() {
    let s = Source::new_smb(
        "NAS-SMB",
        "192.168.1.10",
        "music",
        "user",
        "secret",
        "/flac",
    );
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"kind\":\"smb\""));
    let back: Source = serde_json::from_str(&json).unwrap();
    match &back.kind {
        SourceKind::Smb {
            host,
            share,
            user,
            password,
            base_path,
        } => {
            assert_eq!(host, "192.168.1.10");
            assert_eq!(share, "music");
            assert_eq!(user, "user");
            assert_eq!(password, "secret");
            assert_eq!(base_path, "flac");
        }
        other => panic!("kind inattendu: {other:?}"),
    }
}

#[test]
fn track_keys_finalise() {
    let mut t = Track::new("src://nas/a/b.flac".into(), "nas".into());
    t.artist = "Édith Piaf".into();
    t.album_artist = "Édith Piaf".into();
    t.album = "La Vie en Rose".into();
    t.finalize_keys();
    assert_eq!(t.artist_key, "édith piaf");
    assert_eq!(t.album_key, "édith piaf|la vie en rose");
    assert_eq!(t.title, "b.flac");
}
