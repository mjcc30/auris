//! Tests du decodeur DSD -> DoP (auris-player): en-tete DSF, cadence DoP,
//! marqueurs 0x05/0xFA et packing 24 bits.

use std::io::Cursor;

use auris_player::decoder::{ChunkData, Decoder, TrackMeta};
use auris_player::dsd::{parse_header, DsdDecoder};

/// Construit un fichier DSF minimal mais valide (chunk fmt de 52 octets + data).
fn make_dsf(rate: u32, channels: u32, block_bytes: u32, samples: u64, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"DSD "); // 0..4
    v.extend_from_slice(&28u64.to_le_bytes()); // 4..12 taille chunk DSD
    v.extend_from_slice(b"fmt "); // 12..16
    v.extend_from_slice(&52u64.to_le_bytes()); // fmt size -> data chunk a 24+52=76
    v.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // 24..28 version
    v.extend_from_slice(&0u32.to_le_bytes()); // 28..32 format id = 0 (DSD brut)
    v.extend_from_slice(&rate.to_le_bytes()); // 32..36
    v.extend_from_slice(&channels.to_le_bytes()); // 36..40
    v.extend_from_slice(&block_bytes.to_le_bytes()); // 40..44
    v.extend_from_slice(&samples.to_le_bytes()); // 44..52
    v.extend_from_slice(&[0u8; 24]); // 52..76 reserves
    v.extend_from_slice(b"data"); // 76..80
    v.extend_from_slice(&(data.len() as u64).to_le_bytes()); // 80..88
    v.extend_from_slice(data); // data_offset = 88
    v
}

#[test]
fn parse_dsf_header() {
    let data = vec![0u8; 8192];
    let dsf = make_dsf(2_822_400, 2, 4096, 16 * 64, &data); // DSD64, 64 trames DoP
    let mut cur = Cursor::new(dsf);
    let info = parse_header(&mut cur).unwrap();
    assert_eq!(info.rate, 2_822_400);
    assert_eq!(info.channels, 2);
    assert_eq!(info.block_bytes_per_channel, 4096);
    assert_eq!(info.samples_per_channel, 16 * 64);
    assert_eq!(info.data_offset, 88);
    assert_eq!(info.dop_rate(), 176_400); // DSD64 -> 176.4 kHz
}

#[test]
fn dop_rate_par_niveau() {
    for (rate, dop) in [
        (44100 * 64, 176_400u32),
        (44100 * 128, 352_800),
        (44100 * 256, 705_600), // DSD256 -> 705.6 kHz
    ] {
        let dsf = make_dsf(rate, 2, 4096, 16, &vec![0u8; 8192]);
        let mut cur = Cursor::new(dsf);
        assert_eq!(parse_header(&mut cur).unwrap().dop_rate(), dop);
    }
}

#[test]
fn parse_header_erreurs() {
    let mut court = Cursor::new(vec![0u8; 4]);
    assert!(parse_header(&mut court).is_err());
    let mut magic = Cursor::new(b"RIFF....WAVE".to_vec());
    assert!(parse_header(&mut magic).is_err());
    // DSF compresse (DST, format id != 0) rejete
    let mut dsf = make_dsf(2_822_400, 2, 4096, 16, &vec![0u8; 8192]);
    dsf[28..32].copy_from_slice(&1u32.to_le_bytes());
    let mut cur = Cursor::new(dsf);
    assert!(parse_header(&mut cur).is_err());
}

#[test]
fn dop_packing_marqueurs() {
    // 64 trames DoP, 2 canaux, donnees = 0xAB partout
    let frames = 64u64;
    let data = vec![0xABu8; 4096 * 2];
    let dsf = make_dsf(2_822_400, 2, 4096, 16 * frames, &data);
    let mut dec = DsdDecoder::new(Box::new(Cursor::new(dsf)), TrackMeta::default()).unwrap();

    let spec = dec.spec();
    assert_eq!(spec.rate, 176_400);
    assert_eq!(spec.channels, 2);
    assert!(spec.is_dsd_dop);
    assert!((dec.duration_secs().unwrap() - frames as f64 * 16.0 / 2_822_400.0).abs() < 1e-9);

    let chunk = dec.next_chunk(frames as usize).unwrap().unwrap();
    let ChunkData::I24Dop(v) = chunk else {
        panic!("attendu I24Dop")
    };
    assert_eq!(v.len(), frames as usize * 2);
    for f in 0..frames as usize {
        for c in 0..2 {
            let s = v[f * 2 + c];
            let low = (s & 0xFFFF) as u16;
            let marker = s >> 16;
            assert_eq!(low, 0xABAB, "16 bits DSD dans les bits bas (trame {f})");
            let attendu = if f % 16 == 0 {
                if (f / 16) % 2 == 0 {
                    0x05
                } else {
                    0xFA
                }
            } else {
                0x00
            };
            assert_eq!(marker, attendu, "marqueur DoP trame {f}");
        }
    }
    // fin de piste ensuite
    assert!(dec.next_chunk(64).unwrap().is_none());
}

#[test]
fn dop_seek_reprend_marqueur() {
    let frames = 64u64;
    let data = vec![0xCDu8; 4096 * 2];
    let dsf = make_dsf(2_822_400, 2, 4096, 16 * frames, &data);
    let mut dec = DsdDecoder::new(Box::new(Cursor::new(dsf)), TrackMeta::default()).unwrap();
    // seek a la trame 32 : samples = 32*16, marqueur attendu 0x05 (32/16 pair)
    dec.seek(32.0 * 16.0 / 2_822_400.0).unwrap();
    let chunk = dec.next_chunk(1).unwrap().unwrap();
    let ChunkData::I24Dop(v) = chunk else {
        panic!("attendu I24Dop")
    };
    assert_eq!(v[0] >> 16, 0x05);
    assert_eq!((v[0] & 0xFFFF) as u16, 0xCDCD);
}
