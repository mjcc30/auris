//! DoP sous Windows (WASAPI).
//!
//! cpal ouvre toujours ses flux en mode partagé (`AUDCLNT_SHAREMODE_SHARED`
//! avec `AUTOCONVERTPCM`) — le moteur audio Windows ne délivre au DAC que le
//! « mix format » du périphérique et rééchantillonne tout le reste. Un flux
//! DoP à 176,4 kHz (DSD64) part donc en 48 kHz float32 sur l'iBasso et les
//! marqueurs DoP sont détruits avant le DAC. WASAPI laisse passer le flux
//! tel quel quand il est au mix format : `ensure_dop_carrier` écrit donc le
//! « Default Format » du device en PCM 24 bits à la porteuse DoP.
//! Le changement est persistant (registre MMDevices) mais le moteur audio
//! en cours ne l'adopte qu'à son prochain démarrage (service Audio / reboot).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Mix format courant d'un endpoint de rendu WASAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixFormat {
    pub rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// true si virgule flottante (tag IEEE_FLOAT ou extensible float).
    pub is_float: bool,
}

impl fmt::Display for MixFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} Hz, {} bits {}, {} ch",
            self.rate,
            self.bits_per_sample,
            if self.is_float { "float" } else { "PCM" },
            self.channels
        )
    }
}

/// Construit le WAVEFORMATEX (18 octets, tag PCM) du « Default Format »
/// WASAPI : PCM 24 bits à la porteuse DoP, `channels` canaux.
pub fn dop_default_format_bytes(rate: u32, channels: u16) -> Vec<u8> {
    let block_align = channels.saturating_mul(3);
    let byte_rate = rate.saturating_mul(block_align as u32);
    let mut b = Vec::with_capacity(18);
    b.extend_from_slice(&1u16.to_le_bytes()); // wFormatTag = WAVE_FORMAT_PCM
    b.extend_from_slice(&channels.to_le_bytes()); // nChannels
    b.extend_from_slice(&rate.to_le_bytes()); // nSamplesPerSec
    b.extend_from_slice(&byte_rate.to_le_bytes()); // nAvgBytesPerSec
    b.extend_from_slice(&block_align.to_le_bytes()); // nBlockAlign
    b.extend_from_slice(&24u16.to_le_bytes()); // wBitsPerSample
    b.extend_from_slice(&0u16.to_le_bytes()); // cbSize
    b
}

/// Comparaison insensible à la casse et aux ponctuations.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// cpal préfixe parfois le nom WASAPI par l'index de carte :
/// « Casque (2- iBasso-DC03-Pro) » côté cpal vs « Casque (iBasso-DC03-Pro) »
/// côté WASAPI — on retire le « (<chiffres>- » pour comparer.
fn strip_card_prefix(s: &str) -> String {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == b'(' && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'-' && b[j + 1] == b' ' {
                return format!("{}({}", &s[..i + 1], &s[j + 2..]);
            }
        }
        i += 1;
    }
    s.to_string()
}

/// Mix format du périphérique de rendu choisi (endpoint par défaut si
/// `None`). `None` hors Windows ou si le device est introuvable.
pub fn device_mix_format(device_name: Option<&str>) -> Option<MixFormat> {
    imp::device_mix_format(device_name)
}

/// Mix format de chaque endpoint de rendu actif : `(friendly name, mix)`.
/// Vide hors Windows.
pub fn all_mix_formats() -> Vec<(String, MixFormat)> {
    imp::all_mix_formats()
}

/// S'assure que le « Default Format » WASAPI du device est un PCM 24 bits à
/// `dop_rate` (porteuse DoP : 176 400 Hz pour DSD64, 352 800 pour DSD128…).
///
/// Retourne `Ok(true)` si le format a été écrit (un redémarrage du service
/// audio Windows peut être nécessaire pour que le moteur l'adopte),
/// `Ok(false)` si le mix format était déjà correct ou hors Windows.
pub fn ensure_dop_carrier(device_name: Option<&str>, dop_rate: u32) -> Result<bool, String> {
    imp::ensure_dop_carrier(device_name, dop_rate)
}

/// « Default Format » persisté du device (registre MMDevices — lisible sans
/// admin). C'est le format que le moteur adoptera au prochain rebranchement du
/// DAC / redémarrage du service audio. `None` hors Windows ou si illisible.
pub fn device_default_format(device_name: Option<&str>) -> Option<MixFormat> {
    imp::device_default_format(device_name)
}

/// Redémarre le service Audio Windows en élévation (invite UAC) : c'est ce
/// qui force le moteur à réadopter le Default Format écrit au repos, sans
/// avoir à rebrancher le DAC.
pub fn restart_audio_service() -> std::result::Result<String, String> {
    imp::restart_audio_service()
}

/// GUID SubFormat IEEE float en little-endian (KSDATAFORMAT_SUBTYPE_IEEE_FLOAT).
const SUBFORMAT_IEEE_FLOAT_LE: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// Parse un blob contenant un WAVEFORMATEX (éventuellement précédé de
/// l'en-tête PROPVARIANT de 8 octets) en `MixFormat`.
pub fn parse_waveformatex(b: &[u8]) -> Option<MixFormat> {
    for off in [0usize, 8] {
        let w = b.get(off..off + 18)?;
        let tag = u16::from_le_bytes([w[0], w[1]]);
        if tag != 1 && tag != 3 && tag != 0xFFFE {
            continue;
        }
        let channels = u16::from_le_bytes([w[2], w[3]]);
        let rate = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
        let bits = u16::from_le_bytes([w[14], w[15]]);
        if !(1..=8).contains(&channels) || !(8_000..=1_536_000).contains(&rate) {
            continue;
        }
        let is_float = tag == 3
            || (tag == 0xFFFE
                && b.len() >= off + 40
                && b[off + 24..off + 40] == SUBFORMAT_IEEE_FLOAT_LE);
        return Some(MixFormat {
            rate,
            channels,
            bits_per_sample: bits,
            is_float,
        });
    }
    None
}

/// Indique si le mix format WASAPI courant est compatible avec un flux DoP à
/// `carrier` Hz : il faut un PCM entier (pas float) à exactement la porteuse,
/// sinon le moteur rééchantillonne et détruit les marqueurs DoP (ou le pilote
/// refuse carrément le flux en mode partagé, ex: Default Format 32 bits/384 kHz
/// sur certains DAC USB).
///
/// Retourne un message d'aide actionnable si incompatible, `None` sinon.
pub fn dop_mismatch_hint(mix: Option<MixFormat>, carrier: u32) -> Option<String> {
    let m = mix?;
    if m.is_float || m.rate != carrier {
        Some(format!(
            "DoP: mix format WASAPI actuel {m} incompatible avec la porteuse {carrier} Hz — \
             lancez `auris audio-dop-setup --rate {carrier}` au repos (aucune lecture en cours), \
             puis rebranchez le DAC USB ou redemarrez le service Audio Windows pour que le moteur adopte le PCM 24 bits / {carrier} Hz"
        ))
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::MixFormat;

    pub fn device_mix_format(_device_name: Option<&str>) -> Option<MixFormat> {
        None
    }

    pub fn all_mix_formats() -> Vec<(String, MixFormat)> {
        Vec::new()
    }

    pub fn ensure_dop_carrier(_device_name: Option<&str>, _dop_rate: u32) -> Result<bool, String> {
        Ok(false)
    }

    pub fn device_default_format(_device_name: Option<&str>) -> Option<MixFormat> {
        None
    }

    pub fn restart_audio_service() -> std::result::Result<String, String> {
        Err("Redémarrage du service audio : disponible sous Windows uniquement (sinon rebranchez le DAC USB).".into())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{dop_default_format_bytes, normalize, strip_card_prefix, MixFormat};
    use windows::core::{Result, GUID};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IAudioClient, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator,
        MMDeviceEnumerator, PKEY_AudioEngine_DeviceFormat, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
        WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromBuffer, PROPVARIANT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ, STGM_READWRITE,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);

    /// Initialise COM pour le thread courant (no-op si déjà initialisé).
    struct ComGuard(bool);

    impl ComGuard {
        fn new() -> Result<ComGuard> {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if hr.is_ok() {
                Ok(ComGuard(true))
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(ComGuard(false))
            } else {
                Err(windows::core::Error::from(hr))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() }
            }
        }
    }

    unsafe fn enumerator() -> Result<IMMDeviceEnumerator> {
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
    }

    unsafe fn endpoints() -> Result<Vec<IMMDevice>> {
        let en = unsafe { enumerator()? };
        let coll: IMMDeviceCollection =
            unsafe { en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)? };
        let n = coll.GetCount()?;
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n {
            v.push(coll.Item(i)?);
        }
        Ok(v)
    }

    unsafe fn friendly_name(dev: &IMMDevice) -> String {
        let Ok(store) = (unsafe { dev.OpenPropertyStore(STGM_READ) }) else {
            return String::new();
        };
        let Ok(pv) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return String::new();
        };
        if pv.vt() != VT_LPWSTR {
            return String::new();
        }
        let p = pv.Anonymous.Anonymous.Anonymous.pwszVal;
        if p.is_null() {
            String::new()
        } else {
            p.to_string().unwrap_or_default()
        }
    }

    /// Device correspondant au nom cpal, sinon l'endpoint de rendu par défaut.
    unsafe fn find_device(name: Option<&str>) -> std::result::Result<IMMDevice, String> {
        if let Some(n) = name
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != "default")
        {
            let want = normalize(&strip_card_prefix(n));
            let mut names = Vec::new();
            for dev in unsafe { endpoints() }.map_err(|e| e.to_string())? {
                let got_name = unsafe { friendly_name(&dev) };
                // le nom WASAPI énuméré contient lui aussi le préfixe carte
                // « (2- » : on le strip des deux côtés avant de comparer
                let got = normalize(&strip_card_prefix(&got_name));
                if !got_name.is_empty() {
                    names.push(got_name);
                }
                if !got.is_empty() && (got.contains(&want) || want.contains(&got)) {
                    return Ok(dev);
                }
            }
            // Nom explicite inconnu : PAS de repli silencieux sur l'endpoint
            // par défaut (sinon on reconfigurerait le mauvais DAC).
            return Err(format!(
                "périphérique '{n}' introuvable — disponibles : {}",
                names.join(", ")
            ));
        }
        let en = unsafe { enumerator() }.map_err(|e| e.to_string())?;
        unsafe { en.GetDefaultAudioEndpoint(eRender, eMultimedia) }.map_err(|e| e.to_string())
    }

    unsafe fn mix_format_of(dev: &IMMDevice) -> Result<MixFormat> {
        let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None)? };
        let mix = client.GetMixFormat()?;
        let wf: &WAVEFORMATEX = unsafe { &*mix };
        let is_float = wf.wFormatTag == 3
            || (wf.wFormatTag == 0xFFFE
                && unsafe {
                    core::ptr::addr_of!(
                        (*(mix as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE)).SubFormat
                    )
                    .read_unaligned()
                } == SUBTYPE_IEEE_FLOAT);
        let r = MixFormat {
            rate: wf.nSamplesPerSec,
            channels: wf.nChannels,
            bits_per_sample: wf.wBitsPerSample,
            is_float,
        };
        unsafe { CoTaskMemFree(Some(mix as *const _ as *const core::ffi::c_void)) };
        Ok(r)
    }

    pub fn device_mix_format(device_name: Option<&str>) -> Option<MixFormat> {
        let _guard = ComGuard::new().ok()?;
        let dev = unsafe { find_device(device_name) }.ok()?;
        unsafe { mix_format_of(&dev) }.ok()
    }

    pub fn all_mix_formats() -> Vec<(String, MixFormat)> {
        let Ok(_guard) = ComGuard::new() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(devs) = unsafe { endpoints() } {
            for d in devs {
                let name = unsafe { friendly_name(&d) };
                if let Ok(m) = unsafe { mix_format_of(&d) } {
                    out.push((name, m));
                }
            }
        }
        out
    }

    /// Écriture directe du registre MMDevices (persiste même si le moteur
    /// audio tourne, mais exige les droits admin sur HKLM). Best-effort.
    unsafe fn write_default_format_registry(dev: &IMMDevice, blob: &[u8]) -> bool {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegQueryValueExW, RegSetValueExW, HKEY_LOCAL_MACHINE,
            KEY_WRITE, REG_BINARY, REG_OPTION_NON_VOLATILE,
        };
        const VALUE_NAME: &str = "{f19f064d-082c-4e27-bc73-6882a1bb8e4c},0";
        unsafe {
            let id = match dev.GetId() {
                Ok(p) => p.to_string().unwrap_or_default(),
                Err(_) => return false,
            };
            // GetId() -> "{0.0.0.00000000}.{357f0e79-...}" ; la clé registre
            // ne contient que le second GUID (rsplit : le premier segment
            // contient lui-même des points).
            let guid = id.rsplit_once('.').map(|(_, g)| g).unwrap_or(&id);
            let path = windows::core::HSTRING::from(format!(
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Render\\{guid}\\Properties"
            ));
            let mut hkey = windows::Win32::System::Registry::HKEY::default();
            if RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                &path,
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
            .0 != 0
            {
                return false; // pas admin: le property store suffit (endpoint au repos)
            }
            // réutilise l'en-tête PROPVARIANT (8 octets) de la valeur existante
            let name = windows::core::HSTRING::from(VALUE_NAME);
            let mut prefix = [0x41u8, 0, 0, 0, 0, 0, 0, 0];
            let mut buf = [0u8; 64];
            let mut len = buf.len() as u32;
            if RegQueryValueExW(
                hkey,
                &name,
                None,
                None,
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            )
            .0 == 0
                && (len as usize) >= 8
            {
                prefix.copy_from_slice(&buf[..8]);
            }
            let mut data = Vec::with_capacity(8 + blob.len());
            data.extend_from_slice(&prefix);
            data.extend_from_slice(blob);
            let ok = RegSetValueExW(hkey, &name, None, REG_BINARY, Some(&data)).0 == 0;
            let _ = RegCloseKey(hkey);
            ok
        }
    }

    pub fn ensure_dop_carrier(
        device_name: Option<&str>,
        dop_rate: u32,
    ) -> std::result::Result<bool, String> {
        let _guard = ComGuard::new().map_err(|e| e.to_string())?;
        unsafe {
            let dev =
                find_device(device_name).map_err(|e| format!("énumération du device: {e}"))?;
            let mix = mix_format_of(&dev).map_err(|e| format!("lecture du mix format: {e}"))?;
            if mix.rate == dop_rate && !mix.is_float && mix.bits_per_sample == 24 {
                return Ok(false);
            }
            let channels = mix.channels.max(2);
            let blob = dop_default_format_bytes(dop_rate, channels);
            let store: IPropertyStore = dev
                .OpenPropertyStore(STGM_READWRITE)
                .map_err(|e| format!("OpenPropertyStore: {e}"))?;
            let pv: PROPVARIANT = InitPropVariantFromBuffer(
                blob.as_ptr() as *const core::ffi::c_void,
                blob.len() as u32,
            )
            .map_err(|e| format!("InitPropVariantFromBuffer: {e}"))?;
            store
                .SetValue(&PKEY_AudioEngine_DeviceFormat, &pv)
                .map_err(|e| format!("SetValue(Default Format {dop_rate} Hz / 24 bits): {e}"))?;
            // best-effort: le moteur audio révoque la propriété tant qu'un flux
            // tourne sur l'endpoint; le registre, lui, persiste (si admin).
            let _ = write_default_format_registry(&dev, &blob);
            Ok(true)
        }
    }

    const REG_VALUE_NAME: &str = "{f19f064d-082c-4e27-bc73-6882a1bb8e4c},0";

    /// Lit le « Default Format » persisté dans le registre MMDevices
    /// (lecture HKLM autorisée sans admin).
    pub fn device_default_format(device_name: Option<&str>) -> Option<MixFormat> {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ,
        };
        let _guard = ComGuard::new().ok()?;
        let dev = unsafe { find_device(device_name) }.ok()?;
        let id = unsafe { dev.GetId().ok()?.to_string().ok()? };
        // GetId() -> "{0.0.0.00000000}.{357f0e79-...}" ; la clé registre ne
        // contient que le second GUID (rsplit : le premier segment contient
        // lui-même des points).
        let guid = id.rsplit_once('.').map(|(_, g)| g).unwrap_or(&id);
        let path = windows::core::HSTRING::from(format!(
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Render\\{guid}\\Properties"
        ));
        unsafe {
            let mut hkey = windows::Win32::System::Registry::HKEY::default();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, &path, None, KEY_READ, &mut hkey).0 != 0 {
                return None;
            }
            let name = windows::core::HSTRING::from(REG_VALUE_NAME);
            let mut buf = [0u8; 128];
            let mut len = buf.len() as u32;
            let ok = RegQueryValueExW(
                hkey,
                &name,
                None,
                None,
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            )
            .0 == 0;
            let _ = RegCloseKey(hkey);
            if ok {
                super::parse_waveformatex(&buf[..len as usize])
            } else {
                None
            }
        }
    }

    pub fn restart_audio_service() -> std::result::Result<String, String> {
        // Redémarrage en élévation (UAC) : le moteur audio réadopte alors le
        // Default Format persisté, sans rebrancher le DAC.
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Start-Process -Verb RunAs -FilePath powershell.exe -ArgumentList '-NoProfile','-Command','Restart-Service Audiosrv -Force'",
            ])
            .spawn()
            .map(|_| {
                "Demande de redémarrage du service Audio Windows envoyée — validez l'invite UAC.".to_string()
            })
            .map_err(|e| format!("lancement de PowerShell: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_pcm24_18_octets() {
        let b = dop_default_format_bytes(176_400, 2);
        assert_eq!(b.len(), 18);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 1); // WAVE_FORMAT_PCM
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), 2); // canaux
        assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]), 176_400);
        assert_eq!(u32::from_le_bytes([b[8], b[9], b[10], b[11]]), 176_400 * 6); // nAvgBytesPerSec
        assert_eq!(u16::from_le_bytes([b[12], b[13]]), 6); // nBlockAlign
        assert_eq!(u16::from_le_bytes([b[14], b[15]]), 24); // wBitsPerSample
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 0); // cbSize
    }

    #[test]
    fn format_bytes_porteuses_dsd() {
        // DSD64 -> 176400, DSD128 -> 352800, DSD256 -> 705600 (4 canaux max)
        for (rate, ch) in [(176_400u32, 2u16), (352_800, 2), (705_600, 4), (44_100, 1)] {
            let b = dop_default_format_bytes(rate, ch);
            assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]), rate);
            assert_eq!(u16::from_le_bytes([b[2], b[3]]), ch);
            assert_eq!(u16::from_le_bytes([b[12], b[13]]), ch * 3);
            assert_eq!(
                u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
                rate * ch as u32 * 3
            );
        }
    }

    #[test]
    fn parse_waveformatex_pcm24_176400() {
        // blob registre observé pour le iBasso (Default Format 24 bits / 176400 Hz)
        let b = dop_default_format_bytes(176_400, 2);
        let m = parse_waveformatex(&b).expect("parse");
        assert_eq!(m.rate, 176_400);
        assert_eq!(m.channels, 2);
        assert_eq!(m.bits_per_sample, 24);
        assert!(!m.is_float);
        // avec l'en-tête PROPVARIANT de 8 octets devant
        let mut prefixed = vec![0x41u8, 0, 0, 0, 0, 0, 0, 0];
        prefixed.extend_from_slice(&b);
        let m = parse_waveformatex(&prefixed).expect("parse avec prefixe");
        assert_eq!(m.rate, 176_400);
        assert!(!m.is_float);
    }

    #[test]
    fn parse_waveformatex_extensible_float() {
        // WAVEFORMATEXTENSIBLE 32 bits float 48000 Hz (mix format typique)
        let mut b = Vec::new();
        b.extend_from_slice(&0xFFFEu16.to_le_bytes()); // tag extensible
        b.extend_from_slice(&2u16.to_le_bytes()); // canaux
        b.extend_from_slice(&48_000u32.to_le_bytes()); // rate
        b.extend_from_slice(&(48_000u32 * 8).to_le_bytes()); // byte rate
        b.extend_from_slice(&8u16.to_le_bytes()); // block align
        b.extend_from_slice(&32u16.to_le_bytes()); // bits
        b.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        b.extend_from_slice(&32u16.to_le_bytes()); // valid bits
        b.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        b.extend_from_slice(&SUBFORMAT_IEEE_FLOAT_LE); // subformat float
        let m = parse_waveformatex(&b).expect("parse extensible");
        assert_eq!(m.rate, 48_000);
        assert_eq!(m.bits_per_sample, 32);
        assert!(m.is_float);
        // garbage -> None
        assert!(parse_waveformatex(&[0u8; 4]).is_none());
        assert!(parse_waveformatex(&[]).is_none());
    }

    #[test]
    fn dop_mismatch_hint_incompatible() {
        // mix float -> hint
        let m = MixFormat {
            rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            is_float: true,
        };
        let h = dop_mismatch_hint(Some(m), 176_400).expect("hint");
        assert!(h.contains("48000 Hz, 32 bits float"));
        assert!(h.contains("176400"));
        assert!(h.contains("audio-dop-setup"));
        // mix PCM mais mauvaise cadence (ex: Default Format 24/384000) -> hint
        let m = MixFormat {
            rate: 384_000,
            channels: 2,
            bits_per_sample: 24,
            is_float: false,
        };
        assert!(dop_mismatch_hint(Some(m), 176_400).is_some());
        // mix = porteuse en PCM 24 bits -> pas de hint
        let m = MixFormat {
            rate: 176_400,
            channels: 2,
            bits_per_sample: 24,
            is_float: false,
        };
        assert!(dop_mismatch_hint(Some(m), 176_400).is_none());
        // DSD128
        let m = MixFormat {
            rate: 352_800,
            channels: 2,
            bits_per_sample: 24,
            is_float: false,
        };
        assert!(dop_mismatch_hint(Some(m), 352_800).is_none());
        // mix inconnu (hors Windows) -> pas de hint
        assert!(dop_mismatch_hint(None, 176_400).is_none());
    }

    #[test]
    fn mix_format_display() {
        let m = MixFormat {
            rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            is_float: true,
        };
        assert_eq!(format!("{m}"), "48000 Hz, 32 bits float, 2 ch");
        let m = MixFormat {
            rate: 176_400,
            channels: 2,
            bits_per_sample: 24,
            is_float: false,
        };
        assert_eq!(format!("{m}"), "176400 Hz, 24 bits PCM, 2 ch");
    }

    #[test]
    fn matching_nom_cpal_vs_wasapi() {
        // côté WASAPI, le friendly name énuméré contient AUSSI le préfixe
        // carte : « Casque (2- iBasso-DC03-Pro) »
        let cpal = "Casque (2- iBasso-DC03-Pro)";
        let wasapi = "Casque (2- iBasso-DC03-Pro)";
        let want = normalize(&strip_card_prefix(cpal));
        let got = normalize(&strip_card_prefix(wasapi));
        assert_eq!(want, got);
        assert!(got.contains(&want) || want.contains(&got));
        // sans strip côté énuméré, le préfixe « 2- » empêcherait la
        // correspondance (régression : find_device retournait « introuvable »
        // pour un nom pourtant présent dans la liste)
        let got_sans_strip = normalize(wasapi);
        assert!(!got_sans_strip.contains(&want) && !want.contains(&got_sans_strip));
        // les autres devices ne doivent pas matcher
        assert!(!normalize(&strip_card_prefix(
            "Voicemeeter In 3 (VB-Audio Voicemeeter VAIO)"
        ))
        .contains(&want));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn stubs_hors_windows() {
        assert!(device_mix_format(Some("x")).is_none());
        assert!(all_mix_formats().is_empty());
        assert_eq!(ensure_dop_carrier(Some("x"), 176_400), Ok(false));
    }
}
