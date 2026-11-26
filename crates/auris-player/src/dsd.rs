//! Decodeur DSD (DSF / DFF non compresse) avec encodage DoP (DSD over PCM).
//!
//! Le DoP transporte le flux DSD natif dans des trames PCM 24 bits:
//! - 16 bits DSD par trame et par canal (2 octets bas)
//! - octet de marqueur 0x05 / 0xFA (alterne a chaque trame) dans l'octet haut
//! - cadence = rate_dsd / 16 : DSD64 -> 176.4 kHz, DSD128 -> 352.8 kHz, DSD256 -> 705.6 kHz
//!
//! C'est la methode standard pour delivrer le DSD natif en USB sur les DAC
//! compatibles DoP, sans pilote ASIO specifique.

use std::io::{Read, Seek, SeekFrom};

use auris_core::{AurisError, Result};
use auris_sources::SeekRead;

use crate::decoder::{ChunkData, Decoder, OutputSpec, TrackMeta};

/// Marqueur DoP (DoP Open Standard v1.1): 0x05 / 0xFA en alternance a chaque
/// trame, identique sur tous les canaux d'une trame. Le DAC exige 32 octets
/// marqueurs consecutifs par canal pour verrouiller le mode DoP.
fn dop_marker(global_frame: u64) -> i32 {
    if global_frame.is_multiple_of(2) {
        0x05
    } else {
        0xFA
    }
}

#[derive(Debug, Clone)]
pub struct DsdInfo {
    pub rate: u32, // rate DSD en Hz (44100 * 64 * n)
    pub channels: u8,
    pub samples_per_channel: u64,
    pub data_offset: u64,
    pub block_bytes_per_channel: usize,
}

impl DsdInfo {
    /// Cadence DoP (Hz): DSD64 -> 176400, DSD128 -> 352800, DSD256 -> 705600.
    pub fn dop_rate(&self) -> u32 {
        self.rate / 16
    }
    fn frames_per_block(&self) -> usize {
        self.block_bytes_per_channel * 8 / 16
    }
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}
// DFF est big-endian (contrairement au DSF).
fn rd_u16be(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}
fn rd_u32be(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u64be(b: &[u8], off: usize) -> u64 {
    u64::from_be_bytes(b[off..off + 8].try_into().unwrap())
}

/// Parse l'en-tete DSF ("DSD ") ou DFF ("FRM8"/"DSD ").
pub fn parse_header(reader: &mut dyn SeekRead) -> Result<DsdInfo> {
    reader.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 8192];
    let n = reader.read(&mut head)?;
    let head = &head[..n];
    if head.len() < 12 {
        return Err(AurisError::UnsupportedFormat(
            "fichier DSD trop court".into(),
        ));
    }
    if &head[0..4] == b"DSD " {
        parse_dsf(head)
    } else if &head[0..4] == b"FRM8" {
        parse_dff(reader, head)
    } else {
        Err(AurisError::UnsupportedFormat("ni DSF ni DFF".into()))
    }
}

/// Parse un chunk DSF selon le vrai layout du format (little-endian):
/// en-tete « DSD » de 28 octets, puis chunks a partir de l'offset 28.
/// Particularite DSF: la taille d'un chunk INCLUT son en-tete de 12 octets
/// (id 4 + taille u64), donc le chunk suivant est a `pos + size`.
/// Chunk « fmt » (52 octets dont 12 d'en-tete, corps de 40): version u32,
/// format id u32, type canal u32, canaux u32, frequence u32, bits/sample u32,
/// echantillons/canal u64, taille bloc/canal u32, reserve u32.
fn parse_dsf(head: &[u8]) -> Result<DsdInfo> {
    if head.len() < 28 {
        return Err(AurisError::UnsupportedFormat("DSF trop court".into()));
    }
    let mut pos = 28usize;
    let mut fmt: Option<(usize, usize)> = None; // (offset du corps, taille du corps)
    let mut data: Option<u64> = None; // offset du corps du chunk data
    while pos + 12 <= head.len() {
        let id = &head[pos..pos + 4];
        let size = rd_u64(head, pos + 4) as usize; // inclut l'en-tete de 12 octets
        let body = pos + 12;
        if body > head.len() {
            break;
        }
        match id {
            b"fmt " => fmt = Some((body, size.saturating_sub(12))),
            b"data" => {
                data = Some(body as u64);
                break;
            }
            _ => {}
        }
        if size < 12 {
            break;
        }
        pos += size;
    }
    let (foff, fsize) =
        fmt.ok_or_else(|| AurisError::UnsupportedFormat("DSF sans chunk fmt".into()))?;
    if fsize < 36 || foff + 36 > head.len() {
        return Err(AurisError::UnsupportedFormat(
            "chunk fmt DSF tronque".into(),
        ));
    }
    let format_id = rd_u32(head, foff + 4);
    if format_id != 0 {
        return Err(AurisError::UnsupportedFormat(
            "DSF compresse (DST) non supporte".into(),
        ));
    }
    let channels = rd_u32(head, foff + 12);
    let rate = rd_u32(head, foff + 16);
    let samples = rd_u64(head, foff + 24);
    let block_bytes = rd_u32(head, foff + 32) as usize;
    let data_offset =
        data.ok_or_else(|| AurisError::UnsupportedFormat("DSF sans chunk data".into()))?;
    Ok(DsdInfo {
        rate,
        channels: channels as u8,
        samples_per_channel: samples,
        data_offset,
        block_bytes_per_channel: if block_bytes > 0 { block_bytes } else { 4096 },
    })
}

/// Parse un DFF (EA IFF 85, **big-endian**): « FRM8 » + taille form (u64 BE, 4..12)
/// puis type de forme « DSD » (12..16), puis chunks `id(4) + taille(u64 BE) + corps`
/// a partir de l'offset 16.
/// Chunk « PROP »: « SND » + count (u32 BE) + proprietes `id(4) + taille(u64 BE) + data`
/// (FS = frequence u32 BE, CHNL = canaux u16 BE, CMPR = type compression 4 octets).
/// Chunk « DSD »: donnees DSD brutes, blocs de 4096 octets par canal entrelaces.
fn parse_dff(reader: &mut dyn SeekRead, head: &[u8]) -> Result<DsdInfo> {
    if head.len() < 16 || &head[12..16] != b"DSD " {
        return Err(AurisError::UnsupportedFormat("DFF sans form DSD".into()));
    }
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut compressed = false;
    let mut pos = 16u64;
    let mut data_offset = None;
    let mut data_size = 0u64;
    loop {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 12];
        if reader.read(&mut hdr)? < 12 {
            break;
        }
        let id = &hdr[0..4];
        let size = rd_u64be(&hdr, 4);
        match id {
            b"PROP" => {
                let mut prop = vec![0u8; (size.min(65536)) as usize];
                let pn = reader.read(&mut prop)?;
                let prop = &prop[..pn];
                if prop.len() >= 8 && &prop[0..4] == b"SND " {
                    let count = rd_u32be(prop, 4);
                    let mut p = 8usize;
                    for _ in 0..count {
                        if p + 12 > prop.len() {
                            break;
                        }
                        let pid = &prop[p..p + 4];
                        let psize = rd_u64be(prop, p + 4) as usize;
                        let pdata = p + 12;
                        match pid {
                            b"FS  " if pdata + 4 <= prop.len() => rate = rd_u32be(prop, pdata),
                            b"CHNL" if pdata + 2 <= prop.len() => channels = rd_u16be(prop, pdata),
                            b"CMPR"
                                if pdata + 4 <= prop.len()
                                    && &prop[pdata..pdata + 4] != b"DSD " =>
                            {
                                compressed = true;
                            }
                            _ => {}
                        }
                        p = pdata + psize;
                    }
                }
            }
            b"DSD " => {
                data_offset = Some(pos + 12);
                data_size = size;
                break;
            }
            _ => {}
        }
        pos += 12 + size;
        if size == 0 {
            break;
        }
    }
    if compressed {
        return Err(AurisError::UnsupportedFormat(
            "DFF compresse (DST) non supporte".into(),
        ));
    }
    if rate == 0 || channels == 0 {
        return Err(AurisError::UnsupportedFormat("DFF sans FS/CHNL".into()));
    }
    let data_offset =
        data_offset.ok_or_else(|| AurisError::UnsupportedFormat("DFF sans chunk DSD".into()))?;
    let samples = if data_size > 0 {
        data_size * 8 / channels as u64
    } else {
        let total = reader.seek(SeekFrom::End(0))?;
        total.saturating_sub(data_offset) * 8 / channels as u64
    };
    Ok(DsdInfo {
        rate,
        channels: channels as u8,
        samples_per_channel: samples,
        data_offset,
        block_bytes_per_channel: 4096,
    })
}

/// Decodeur DSD -> trames DoP 24 bits (i32 dont seuls les 24 bits bas sont significatifs).
pub struct DsdDecoder {
    reader: Box<dyn SeekRead>,
    info: DsdInfo,
    block_buf: Vec<u8>,
    block_valid: usize,
    block_pos: usize,      // trames DoP deja consommees dans le bloc courant
    block_base_frame: u64, // index global de la 1re trame du bloc courant
    dop_frames_out: u64,
    meta: TrackMeta,
}

impl DsdDecoder {
    pub fn new(reader: Box<dyn SeekRead>, meta: TrackMeta) -> Result<Self> {
        let mut r = reader;
        let info = parse_header(&mut *r)?;
        if info.channels == 0 || info.rate == 0 {
            return Err(AurisError::UnsupportedFormat("DSD invalide".into()));
        }
        r.seek(SeekFrom::Start(info.data_offset))?;
        Ok(DsdDecoder {
            reader: r,
            info,
            block_buf: Vec::new(),
            block_valid: 0,
            block_pos: 0,
            block_base_frame: 0,
            dop_frames_out: 0,
            meta,
        })
    }

    pub fn info(&self) -> &DsdInfo {
        &self.info
    }

    fn load_next_block(&mut self) -> Result<bool> {
        let bs = self.info.block_bytes_per_channel;
        let ch = self.info.channels as usize;
        let mut buf = vec![0u8; bs * ch];
        let n = self.reader.read(&mut buf)?;
        if n == 0 {
            return Ok(false);
        }
        self.block_base_frame = self.dop_frames_out;
        self.block_buf = buf;
        self.block_valid = n;
        self.block_pos = 0;
        Ok(true)
    }

    fn frames_avail_in_block(&self) -> usize {
        let bs = self.info.block_bytes_per_channel;
        let ch = self.info.channels as usize;
        if self.block_valid >= bs * ch {
            bs / 2
        } else {
            // bloc partiel: seule la derniere plane peut etre tronquee
            self.block_valid.saturating_sub(bs * (ch - 1)).min(bs) / 2
        }
    }
}

impl Decoder for DsdDecoder {
    fn spec(&self) -> OutputSpec {
        OutputSpec {
            rate: self.info.dop_rate(),
            channels: self.info.channels,
            bits: 24,
            is_dsd_dop: true,
        }
    }

    fn duration_secs(&self) -> Option<f64> {
        Some(self.info.samples_per_channel as f64 / self.info.rate as f64)
    }

    fn meta(&self) -> &TrackMeta {
        &self.meta
    }

    fn next_chunk(&mut self, max_frames: usize) -> Result<Option<ChunkData>> {
        let ch = self.info.channels as usize;
        let fpb = self.info.frames_per_block();
        let total_frames = if self.info.samples_per_channel > 0 {
            self.info.samples_per_channel / 16
        } else {
            u64::MAX
        };
        let mut out: Vec<i32> = Vec::with_capacity(max_frames * ch);
        while out.len() / ch < max_frames && self.dop_frames_out < total_frames {
            if self.block_pos >= self.frames_avail_in_block() {
                if !self.load_next_block()? {
                    break;
                }
                if self.frames_avail_in_block() == 0 {
                    break;
                }
            }
            let avail = self.frames_avail_in_block() - self.block_pos;
            let take = avail.min(max_frames - out.len() / ch).min(fpb);
            if take == 0 {
                break;
            }
            let bs = self.info.block_bytes_per_channel;
            for i in 0..take {
                let global_frame = self.block_base_frame + (self.block_pos + i) as u64;
                let marker = dop_marker(global_frame);
                for c in 0..ch {
                    let off = c * bs + (self.block_pos + i) * 2;
                    let bits =
                        u16::from_le_bytes([self.block_buf[off], self.block_buf[off + 1]]) as i32;
                    out.push((marker << 16) | bits);
                }
            }
            self.block_pos += take;
            self.dop_frames_out += take as u64;
        }
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ChunkData::I24Dop(out)))
        }
    }

    fn seek(&mut self, secs: f64) -> Result<()> {
        let samples = (secs.max(0.0) * self.info.rate as f64) as u64;
        let frame = samples / 16;
        let fpb = self.info.frames_per_block() as u64;
        let bs = self.info.block_bytes_per_channel as u64;
        let ch = self.info.channels as u64;
        let block_index = frame / fpb;
        self.reader.seek(SeekFrom::Start(
            self.info.data_offset + block_index * bs * ch,
        ))?;
        self.dop_frames_out = frame;
        self.block_base_frame = block_index * fpb;
        self.block_valid = 0;
        self.block_pos = (frame % fpb) as usize;
        Ok(())
    }
}
