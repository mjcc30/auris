//! Tests du decodeur DSD -> DoP (auris-player): en-tete DSF, cadence DoP,
//! marqueurs 0x05/0xFA et packing 24 bits.

use std::io::Cursor;

use auris_player::decoder::{ChunkData, Decoder, TrackMeta};
use auris_player::dsd::{parse_header, DsdDecoder};

/// Construit un fichier DSF conforme au vrai format (little-endian).
/// Particularite DSF: la taille d'un chunk inclut son en-tete de 12 octets.
/// En-tete « DSD » de 28 octets, chunk « fmt » de 52 octets (corps 40) a 28,
/// chunk « data » a 80, donnees a 92.
fn make_dsf(rate: u32, channels: u32, block_bytes: u32, samples: u64, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"DSD "); // 0..4
    v.extend_from_slice(&28u64.to_le_bytes()); // 4..12 taille chunk DSD (en-tete comprise)
    v.extend_from_slice(&0u64.to_le_bytes()); // 12..20 taille totale du fichier
    v.extend_from_slice(&0u64.to_le_bytes()); // 20..28 pointeur metadonnees
    v.extend_from_slice(b"fmt "); // 28..32
    v.extend_from_slice(&52u64.to_le_bytes()); // 32..40 taille chunk fmt = 52 (en-tete comprise)
    v.extend_from_slice(&1u32.to_le_bytes()); // 40..44 version du format
    v.extend_from_slice(&0u32.to_le_bytes()); // 44..48 format id = 0 (DSD brut)
    v.extend_from_slice(&1u32.to_le_bytes()); // 48..52 type de canal (1 = stereo)
    v.extend_from_slice(&channels.to_le_bytes()); // 52..56
    v.extend_from_slice(&rate.to_le_bytes()); // 56..60 frequence DSD
    v.extend_from_slice(&1u32.to_le_bytes()); // 60..64 bits par sample = 1
    v.extend_from_slice(&samples.to_le_bytes()); // 64..72 echantillons par canal
    v.extend_from_slice(&block_bytes.to_le_bytes()); // 72..76 taille bloc par canal
    v.extend_from_slice(&0u32.to_le_bytes()); // 76..80 reserve (corps fmt = 40 octets)
    v.extend_from_slice(b"data"); // 80..84
    v.extend_from_slice(&((data.len() + 12) as u64).to_le_bytes()); // 84..92 taille chunk data (en-tete comprise)
    v.extend_from_slice(data); // data_offset = 92
    v
}

/// Construit un DFF minimal conforme (big-endian) : FRM8 + DSD + PROP(SND) + DSD data.
fn make_dff(rate: u32, channels: u16, data: &[u8]) -> Vec<u8> {
    let be32 = |v: u32| v.to_be_bytes();
    let be64 = |v: u64| v.to_be_bytes();
    // PROP: "SND " (4 octets) + count(u32 BE) + FS (4+8+4) + CHNL (4+8+2) + CMPR (4+8+4)
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&be32(3));
    prop.extend_from_slice(b"FS  ");
    prop.extend_from_slice(&be64(4));
    prop.extend_from_slice(&be32(rate));
    prop.extend_from_slice(b"CHNL");
    prop.extend_from_slice(&be64(2));
    prop.extend_from_slice(&channels.to_be_bytes());
    prop.extend_from_slice(b"CMPR");
    prop.extend_from_slice(&be64(4));
    prop.extend_from_slice(b"DSD ");
    let mut v = Vec::new();
    v.extend_from_slice(b"FRM8");
    v.extend_from_slice(&be64(0)); // taille form (peu importe pour le parse)
    v.extend_from_slice(b"DSD ");
    v.extend_from_slice(b"PROP");
    v.extend_from_slice(&be64(prop.len() as u64));
    v.extend_from_slice(&prop);
    v.extend_from_slice(b"DSD ");
    v.extend_from_slice(&be64(data.len() as u64));
    v.extend_from_slice(data);
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
    assert_eq!(info.data_offset, 92);
    assert_eq!(info.dop_rate(), 176_400); // DSD64 -> 176.4 kHz
}

#[test]
fn parse_dff_header_big_endian() {
    let data = vec![0u8; 8192];
    let dff = make_dff(2_822_400, 2, &data);
    let mut cur = Cursor::new(dff);
    let info = parse_header(&mut cur).unwrap();
    assert_eq!(info.rate, 2_822_400);
    assert_eq!(info.channels, 2);
    assert_eq!(info.block_bytes_per_channel, 4096);
    assert_eq!(info.samples_per_channel, 8192 * 8 / 2);
    assert_eq!(info.dop_rate(), 176_400);
}

/// 104 premiers octets d'un VRAI fichier DSF (DSD64 stereo, decaud.io) :
/// en-tete « DSD » de 28 octets, « fmt » a 28 (taille 52 en-tete comprise),
/// « data » a 80, donnees a 92. Garde-fou contre un parse aligne sur un faux
/// layout de test (tailles de chunks DSF = en-tete de 12 octets comprise).
const REAL_DSF_HEADER: [u8; 104] = [
    0x44, 0x53, 0x44, 0x20, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5f, 0xad, 0xf8, 0x03,
    0x00, 0x00, 0x00, 0x00, 0x5c, 0x40, 0xf6, 0x03, 0x00, 0x00, 0x00, 0x00, 0x66, 0x6d, 0x74, 0x20,
    0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x11, 0x2b, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0xdb, 0xd8, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x64, 0x61, 0x74, 0x61, 0x0c, 0x40, 0xf6, 0x03, 0x00, 0x00, 0x00, 0x00, 0x96, 0x69, 0x9a, 0x59,
    0x9a, 0x9a, 0x69, 0xa6, 0x6a, 0xa6, 0xa5, 0xa9,
];

#[test]
fn parse_dsf_header_fichier_reel() {
    // vrai en-tete DSF + 4096 octets de "donnees" factices
    let mut file = REAL_DSF_HEADER.to_vec();
    file.extend_from_slice(&vec![0u8; 4096 * 2]);
    let mut cur = Cursor::new(file);
    let info = parse_header(&mut cur).unwrap();
    assert_eq!(info.rate, 2_822_400); // DSD64
    assert_eq!(info.channels, 2);
    assert_eq!(info.block_bytes_per_channel, 4096);
    assert_eq!(info.samples_per_channel, 0x0FD8DB00);
    assert_eq!(info.data_offset, 92);
    assert_eq!(info.dop_rate(), 176_400);
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
    // DSF compresse (DST, format id != 0, a l'offset 44 dans le vrai layout) rejete
    let mut dsf = make_dsf(2_822_400, 2, 4096, 16, &vec![0u8; 8192]);
    dsf[44..48].copy_from_slice(&1u32.to_le_bytes());
    let mut cur = Cursor::new(dsf);
    assert!(parse_header(&mut cur).is_err());
}

#[test]
fn dop_packing_marqueurs() {
    // 64 trames DoP, 2 canaux. DSF = octets LSB-first (spec Sony) : le bit le
    // plus ancien (bit 0 du 1er octet) doit tomber dans le bit 15 du champ DoP
    // (slot t0 = MSB du champ) -> reverse_bits(0x8080) = 0x0101.
    let frames = 64u64;
    let data = vec![0x80u8; 4096 * 2];
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
            assert_eq!(
                low, 0x0101,
                "reverse_bits des 16 bits DSD dans les bits bas (trame {f})"
            );
            let attendu = if f % 2 == 0 { 0x05 } else { 0xFA };
            assert_eq!(marker, attendu, "marqueur DoP trame {f}");
        }
    }
    // condition de verrouillage du DAC: 32 trames consecutives, chacune avec
    // un marqueur valide 0x05/0xFA sur chaque canal (jamais 0x00)
    for f in 0..32usize {
        for c in 0..2 {
            let m = v[f * 2 + c] >> 16;
            assert!(
                m == 0x05 || m == 0xFA,
                "marqueur absent trame {f} canal {c}"
            );
        }
    }
    // fin de piste ensuite
    assert!(dec.next_chunk(64).unwrap().is_none());
}

#[test]
fn dop_packing_dff_echange_octets() {
    // DFF (DSDIFF) = octets MSB-first (convention Philips) : le bit 7 du 1er
    // octet (plus ancien) passe dans le bit 15 du champ DoP par echange des
    // deux octets. Motif 0x01,0x80 -> raw LE 0x8001 -> champ 0x0180.
    let mut data = Vec::with_capacity(4096 * 2);
    for _ in 0..4096 {
        data.push(0x01u8);
        data.push(0x80u8);
    }
    let dff = make_dff(2_822_400, 2, &data);
    let mut dec = DsdDecoder::new(Box::new(Cursor::new(dff)), TrackMeta::default()).unwrap();
    assert!(!dec.info().lsb_first);
    let chunk = dec.next_chunk(4).unwrap().unwrap();
    let ChunkData::I24Dop(v) = chunk else {
        panic!("attendu I24Dop")
    };
    assert_eq!(
        (v[0] & 0xFFFF) as u16,
        0x0180,
        "DFF MSB-first: echange des octets (raw 0x8001 -> 0x0180)"
    );
    assert_eq!(v[0] >> 16, 0x05);
}

#[test]
fn dop_seek_reprend_marqueur() {
    let frames = 64u64;
    let data = vec![0xCDu8; 4096 * 2];
    let dsf = make_dsf(2_822_400, 2, 4096, 16 * frames, &data);
    let mut dec = DsdDecoder::new(Box::new(Cursor::new(dsf)), TrackMeta::default()).unwrap();
    // seek a la trame 32 : samples = 32*16, trame paire -> marqueur 0x05
    dec.seek(32.0 * 16.0 / 2_822_400.0).unwrap();
    let chunk = dec.next_chunk(1).unwrap().unwrap();
    let ChunkData::I24Dop(v) = chunk else {
        panic!("attendu I24Dop")
    };
    assert_eq!(v[0] >> 16, 0x05);
    // DSF (LSB-first) 0xCDCD -> reverse_bits: 0xCDCD -> 0xB3B3
    assert_eq!((v[0] & 0xFFFF) as u16, 0xB3B3);
}
