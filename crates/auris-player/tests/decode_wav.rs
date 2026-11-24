//! Test d'integration: decode un WAV local de 5 s et verifie qu'on obtient
//! bien ~5 s de PCM (regression: fin prematuree du decodeur symphonia 0.6).

use std::io::Write;

use auris_core::Source;
use auris_player::decoder::{open_decoder, ChunkData};
use auris_sources::SourcesManager;

fn write_test_wav(path: &std::path::Path) {
    let rate = 44100u32;
    let secs = 5u32;
    let frames = rate * secs;
    let data_len = frames * 2 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // stéréo
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
    w.extend_from_slice(&4u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v =
            ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 12000.0) as i16;
        w.extend_from_slice(&v.to_le_bytes());
        w.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, w).unwrap();
}

#[tokio::test]
async fn decode_wav_5s() {
    let dir = std::env::temp_dir().join("auris-test-wav");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.wav");
    write_test_wav(&path);

    let src = Source::new_local("test", dir.to_str().unwrap());
    let mgr = SourcesManager::new(vec![src.clone()]).unwrap();
    let reader = mgr
        .open_uri(&format!("src://{}/test.wav", src.id))
        .await
        .unwrap();
    let mut dec = open_decoder(reader, "wav", Default::default()).unwrap();
    let spec = dec.spec();
    assert_eq!(spec.rate, 44100);
    assert_eq!(spec.channels, 2);
    let mut total_frames = 0usize;
    let mut chunks = 0usize;
    loop {
        match dec.next_chunk(8192).unwrap() {
            Some(ChunkData::F32(v)) => {
                total_frames += v.len() / spec.channels as usize;
                chunks += 1;
            }
            Some(ChunkData::I24Dop(_)) => panic!("wav ne doit pas produire de DoP"),
            None => break,
        }
    }
    let _ =
        std::io::stdout().write_all(format!("chunks={chunks} frames={total_frames}\n").as_bytes());
    // 5 s a 44100 Hz = 220500 trames; on tolere une marge de 1 chunk
    assert!(
        total_frames > 220500 - 8192,
        "trop peu de trames decodees: {total_frames}"
    );
    assert!(dec.duration_secs().unwrap() > 4.5);
}

#[tokio::test]
async fn decode_python_wav() {
    // fichier genere par le module wave de python (cas du smoke test serveur)
    let path = std::path::PathBuf::from("/tmp/auris-music/test.wav");
    if !path.exists() {
        eprintln!("skip: {:?} absent", path);
        return;
    }
    let src = Source::new_local("py", "/tmp/auris-music");
    let mgr = SourcesManager::new(vec![src.clone()]).unwrap();
    let reader = mgr
        .open_uri(&format!("src://{}/test.wav", src.id))
        .await
        .unwrap();
    let mut dec = open_decoder(reader, "wav", Default::default()).unwrap();
    let mut total = 0usize;
    let mut chunks = 0usize;
    loop {
        match dec.next_chunk(8192).unwrap() {
            Some(ChunkData::F32(v)) => {
                total += v.len() / 2;
                chunks += 1;
                if chunks <= 3 || chunks.is_multiple_of(50) {
                    eprintln!("chunk {chunks}: {} frames", v.len() / 2);
                }
            }
            Some(ChunkData::I24Dop(_)) => panic!("DoP inattendu"),
            None => break,
        }
    }
    eprintln!("TOTAL chunks={chunks} frames={total}");
    assert!(total > 220500 - 8192, "trop peu: {total}");
}
