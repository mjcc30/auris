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

/// Mix format courant d'un endpoint de rendu WASAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    unsafe fn find_device(name: Option<&str>) -> Result<IMMDevice> {
        if let Some(n) = name
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != "default")
        {
            let want = normalize(&strip_card_prefix(n));
            for dev in unsafe { endpoints()? } {
                let got = normalize(&unsafe { friendly_name(&dev) });
                if !got.is_empty() && (got.contains(&want) || want.contains(&got)) {
                    return Ok(dev);
                }
            }
        }
        let en = unsafe { enumerator()? };
        unsafe { en.GetDefaultAudioEndpoint(eRender, eMultimedia) }
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
            // ne contient que le second GUID.
            let guid = id.split_once('.').map(|(_, g)| g).unwrap_or(&id);
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
        let cpal = "Casque (2- iBasso-DC03-Pro)";
        let wasapi = "Casque (iBasso-DC03-Pro)";
        let want = normalize(&strip_card_prefix(cpal));
        let got = normalize(wasapi);
        assert_eq!(want, got);
        assert!(got.contains(&want) || want.contains(&got));
        // sans le strip, le préfixe "2-" empêcherait la correspondance
        assert_ne!(normalize(cpal), normalize(wasapi));
        // les autres devices ne doivent pas matcher
        assert!(!normalize("Voicemeeter In 3 (VB-Audio Voicemeeter VAIO)").contains(&want));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn stubs_hors_windows() {
        assert!(device_mix_format(Some("x")).is_none());
        assert!(all_mix_formats().is_empty());
        assert_eq!(ensure_dop_carrier(Some("x"), 176_400), Ok(false));
    }
}
