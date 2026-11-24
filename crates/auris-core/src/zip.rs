//! Ecrivain ZIP minimal, 100 % Rust et sans dependance externe.
//!
//! methode "stored" (sans compression): ideal pour l'export de fichiers audio
//! (FLAC/DSD deja compresses) et pour rester compilable sans compilateur C.
//! Les noms sont encodes en UTF-8 (flag 0x0800).

use std::io::{self, Read};
use std::sync::OnceLock;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const METHOD_STORED: u16 = 0;
const FLAG_UTF8: u16 = 0x0800;
/// 2026-01-01 00:00 en format DOS (les horodatages reels ne sont pas requis).
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;

fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    })
}

pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Nettoie un nom de fichier d'archive: separateurs `/`, pas de chemin absolu.
pub fn sanitize_name(name: &str) -> String {
    let n = name.replace('\\', "/");
    let n = n.trim_start_matches('/');
    n.split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect::<Vec<_>>()
        .join("/")
}

struct Central {
    name: Vec<u8>,
    crc: u32,
    size: u32,
    offset: u32,
}

/// Ecrivain ZIP en memoire (methode stored).
pub struct ZipWriter {
    buf: Vec<u8>,
    entries: Vec<Central>,
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter {
            buf: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// Ajoute un fichier complet (contenu en memoire).
    pub fn add_file(&mut self, name: &str, data: &[u8]) {
        let name = sanitize_name(name);
        let crc = crc32(data);
        self.write_entry(&name, crc, data.len() as u64, |out| {
            out.extend_from_slice(data);
            Ok(())
        })
        .expect("ecriture en memoire");
    }

    /// Ajoute un fichier en streaming depuis un reader (taille connue ou non).
    /// Retourne le nombre d'octets ecrits.
    pub fn add_stream<R: Read>(
        &mut self,
        name: &str,
        mut reader: R,
        _size: Option<u64>,
    ) -> io::Result<u64> {
        let name = sanitize_name(name);
        let mut written: u64 = 0;
        self.write_entry(&name, 0, 0, |out| {
            // on ecrit via un writer intermediaire qui calcule le crc a la volee
            let mut crc = 0xFFFF_FFFFu32;
            let table = crc_table();
            let mut chunk = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                for &b in &chunk[..n] {
                    crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
                }
                out.extend_from_slice(&chunk[..n]);
                written += n as u64;
            }
            // patch du crc dans le header local (offset+14) et des tailles
            let off = out.len() - written as usize - 30 - name.len();
            let final_crc = crc ^ 0xFFFF_FFFF;
            out[off + 14..off + 18].copy_from_slice(&final_crc.to_le_bytes());
            out[off + 18..off + 22].copy_from_slice(&(written as u32).to_le_bytes());
            out[off + 22..off + 26].copy_from_slice(&(written as u32).to_le_bytes());
            Ok(())
        })?;
        // write_entry a recu crc=0/size=0: on corrige l'entree centrale
        if let Some(e) = self.entries.last_mut() {
            e.size = written as u32;
            // le crc final est relu depuis le header local
            let off = e.offset as usize;
            e.crc = u32::from_le_bytes(self.buf[off + 14..off + 18].try_into().unwrap());
        }
        Ok(written)
    }

    /// Ecrit un header local + contenu via la closure, enregistre l'entree centrale.
    fn write_entry<F>(&mut self, name: &str, crc: u32, size: u64, write_data: F) -> io::Result<()>
    where
        F: FnOnce(&mut Vec<u8>) -> io::Result<()>,
    {
        let offset = self.buf.len() as u32;
        let name_b = name.as_bytes();
        self.buf.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        self.buf.extend_from_slice(&20u16.to_le_bytes()); // version
        self.buf.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        self.buf.extend_from_slice(&METHOD_STORED.to_le_bytes());
        self.buf.extend_from_slice(&DOS_TIME.to_le_bytes());
        self.buf.extend_from_slice(&DOS_DATE.to_le_bytes());
        self.buf.extend_from_slice(&crc.to_le_bytes());
        self.buf.extend_from_slice(&((size as u32).to_le_bytes())); // compressee
        self.buf.extend_from_slice(&((size as u32).to_le_bytes())); // non compressee
        self.buf
            .extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra
        self.buf.extend_from_slice(name_b);
        write_data(&mut self.buf)?;
        self.entries.push(Central {
            name: name_b.to_vec(),
            crc,
            size: size as u32,
            offset,
        });
        Ok(())
    }

    /// Termine l'archive et retourne les octets ZIP complets.
    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;
        for e in &self.entries {
            self.buf.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.buf.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            self.buf.extend_from_slice(&METHOD_STORED.to_le_bytes());
            self.buf.extend_from_slice(&DOS_TIME.to_le_bytes());
            self.buf.extend_from_slice(&DOS_DATE.to_le_bytes());
            self.buf.extend_from_slice(&e.crc.to_le_bytes());
            self.buf.extend_from_slice(&e.size.to_le_bytes());
            self.buf.extend_from_slice(&e.size.to_le_bytes());
            self.buf
                .extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // commentaire
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // disque
            self.buf.extend_from_slice(&0u16.to_le_bytes()); // attrs internes
            self.buf.extend_from_slice(&0u32.to_le_bytes()); // attrs externes
            self.buf.extend_from_slice(&e.offset.to_le_bytes());
            self.buf.extend_from_slice(&e.name);
        }
        let cd_size = self.buf.len() as u32 - cd_offset;
        let count = self.entries.len() as u16;
        self.buf.extend_from_slice(&SIG_EOCD.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // disque
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // disque cd
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&count.to_le_bytes());
        self.buf.extend_from_slice(&cd_size.to_le_bytes());
        self.buf.extend_from_slice(&cd_offset.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes()); // commentaire
        self.buf
    }
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_valeur_de_reference() {
        // valeur de reference standard CRC-32
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn sanitize_noms() {
        assert_eq!(sanitize_name("a/b\\c.flac"), "a/b/c.flac");
        assert_eq!(sanitize_name("/abs/x.flac"), "abs/x.flac");
        assert_eq!(sanitize_name("../evil.flac"), "evil.flac");
        assert_eq!(sanitize_name("éàü.flac"), "éàü.flac");
    }

    /// Mini-lecteur ZIP (stored uniquement) pour verifier le roundtrip.
    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert!(bytes.len() > 22);
        let eocd = &bytes[bytes.len() - 22..];
        assert_eq!(u32::from_le_bytes(eocd[0..4].try_into().unwrap()), SIG_EOCD);
        let count = u16::from_le_bytes(eocd[10..12].try_into().unwrap()) as usize;
        let cd_off = u32::from_le_bytes(eocd[16..20].try_into().unwrap()) as usize;
        let mut out = Vec::new();
        let mut p = cd_off;
        for _ in 0..count {
            assert_eq!(
                u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()),
                SIG_CENTRAL
            );
            let crc = u32::from_le_bytes(bytes[p + 16..p + 20].try_into().unwrap());
            let size = u32::from_le_bytes(bytes[p + 24..p + 28].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes(bytes[p + 28..p + 30].try_into().unwrap()) as usize;
            let local_off = u32::from_le_bytes(bytes[p + 42..p + 46].try_into().unwrap()) as usize;
            let name = String::from_utf8(bytes[p + 46..p + 46 + nlen].to_vec()).unwrap();
            // local header
            let l = local_off;
            assert_eq!(
                u32::from_le_bytes(bytes[l..l + 4].try_into().unwrap()),
                SIG_LOCAL
            );
            let lnlen = u16::from_le_bytes(bytes[l + 26..l + 28].try_into().unwrap()) as usize;
            let data_off = l + 30 + lnlen;
            let data = bytes[data_off..data_off + size].to_vec();
            assert_eq!(crc32(&data), crc, "crc de {name}");
            out.push((name, data));
            p += 46 + nlen;
        }
        out
    }

    #[test]
    fn zip_roundtrip_fichiers() {
        let mut z = ZipWriter::new();
        z.add_file("album/titre 01.flac", b"RIFF....data");
        z.add_file("album/titre 02.flac", &[0u8; 100_000]);
        let bytes = z.finish();
        let files = read_zip(&bytes);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "album/titre 01.flac");
        assert_eq!(files[0].1, b"RIFF....data");
        assert_eq!(files[1].1.len(), 100_000);
    }

    #[test]
    fn zip_roundtrip_stream() {
        let data: Vec<u8> = (0..500_000u32).map(|i| (i % 251) as u8).collect();
        let mut z = ZipWriter::new();
        let n = z
            .add_stream("gros/fichier.dsf", &data[..], Some(data.len() as u64))
            .unwrap();
        assert_eq!(n, data.len() as u64);
        let bytes = z.finish();
        let files = read_zip(&bytes);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "gros/fichier.dsf");
        assert_eq!(files[0].1, data);
    }

    #[test]
    fn zip_nom_utf8_avec_accents() {
        let mut z = ZipWriter::new();
        z.add_file("François - Été.flac", b"x");
        let files = read_zip(&z.finish());
        assert_eq!(files[0].0, "François - Été.flac");
    }
}
