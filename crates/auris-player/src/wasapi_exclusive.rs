//! Sortie WASAPI **exclusive** (Windows uniquement) — comportement « PotPlayer » :
//! bit-perfect pour tous les formats.
//!
//! cpal n'ouvre que le mode partagé WASAPI : le moteur audio Windows
//! rééchantillonne alors tout vers le « mix format » du device (les marqueurs
//! DoP sont détruits, aucun bit-perfect). En mode EXCLUSIF, l'application
//! parle directement au pilote du DAC au format demandé : aucun moteur entre
//! les deux, le flux arrive intact (PCM natif ou trames DoP à la porteuse).
//!
//! - `candidates()` choisit le format du device le plus fidèle à la piste
//!   (conteneur = bits de la source quand le DAC l'accepte en exclusif).
//! - `ExclusiveStream` ouvre l'endpoint en EXCLUSIF au format retenu et
//!   pousse les chunks décodés via un thread `IAudioRenderClient`. Deux modes
//!   d'initialisation sont essayés : **polling** (`hnsBufferDuration =
//!   hnsPeriodicity = période du device`, sans `EVENTCALLBACK` — seule forme
//!   acceptée par certains DAC USB comme l'iBasso DC03-Pro) puis
//!   **event-driven** (`EVENTCALLBACK` + `SetEventHandle`, la forme standard).
//! - Si aucun format n'est accepté en exclusif -> `Err` : l'appelant retombe
//!   sur le chemin cpal partagé (voir `output.rs`).
//!
//! Hors Windows : stubs (`probe_any` -> false, `open` -> Err).

use crate::decoder::OutputSpec;
use crate::output::RenderParams;
use crate::wasapi_dop::MixFormat;

/// Format de conteneur proposé au DAC en mode exclusif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFormat {
    /// PCM 16 bits (2 octets/échantillon).
    Pcm16,
    /// PCM 24 bits « packed » 3 octets (le standard DoP).
    Pcm24Packed,
    /// PCM 24 bits dans un conteneur 32 bits (WAVEFORMATEXTENSIBLE, valid=24).
    Pcm24In32,
    /// PCM 32 bits entiers.
    Pcm32,
    /// IEEE float 32 bits.
    F32,
}

impl DeviceFormat {
    /// Octets par échantillon (par canal).
    pub fn sample_bytes(self) -> usize {
        match self {
            DeviceFormat::Pcm16 => 2,
            DeviceFormat::Pcm24Packed => 3,
            DeviceFormat::Pcm24In32 | DeviceFormat::Pcm32 | DeviceFormat::F32 => 4,
        }
    }
    /// Octets par trame (tous canaux).
    pub fn frame_bytes(self, channels: u16) -> usize {
        self.sample_bytes() * channels.max(1) as usize
    }
    /// true si le conteneur 24 bits est « packed » 3 octets (sérialisation spéciale).
    pub fn packed24(self) -> bool {
        matches!(self, DeviceFormat::Pcm24Packed)
    }
    pub fn mix(self, rate: u32, channels: u16) -> MixFormat {
        let (bits, is_float) = match self {
            DeviceFormat::Pcm16 => (16, false),
            DeviceFormat::Pcm24Packed | DeviceFormat::Pcm24In32 => (24, false),
            DeviceFormat::Pcm32 => (32, false),
            DeviceFormat::F32 => (32, true),
        };
        MixFormat {
            rate,
            channels,
            bits_per_sample: bits,
            is_float,
        }
    }
    pub fn describe(self, rate: u32, channels: u16) -> String {
        let m = self.mix(rate, channels);
        format!("exclusif {m}")
    }
}

/// Candidats (format, taux, canaux) pour un spec de piste, du plus fidèle au
/// plus compatible. Pure : testable hors Windows.
///
/// - flux DoP : porteuse en PCM 24 bits (packed d'abord, standard DoP).
/// - PCM : conteneur = bits de la source si possible (bit-perfect : les
///   échantillons entiers 16/24 bits sont représentables exactement en f32,
///   donc l'aller-retour f32 -> entier est sans perte).
pub fn candidates(spec: &OutputSpec) -> Vec<(DeviceFormat, u32, u16)> {
    let ch = spec.channels.max(1) as u16;
    let rate = spec.rate.max(1);
    let mut v = Vec::new();
    if spec.is_dsd_dop {
        v.push((DeviceFormat::Pcm24Packed, rate, ch));
        v.push((DeviceFormat::Pcm24In32, rate, ch));
        v.push((DeviceFormat::Pcm32, rate, ch));
        return v;
    }
    let rest = [
        DeviceFormat::Pcm24Packed,
        DeviceFormat::Pcm24In32,
        DeviceFormat::Pcm32,
        DeviceFormat::Pcm16,
        DeviceFormat::F32,
    ];
    let first = match spec.bits {
        16 => DeviceFormat::Pcm16,
        24 => DeviceFormat::Pcm24Packed,
        32 => DeviceFormat::Pcm32,
        _ => DeviceFormat::Pcm24In32,
    };
    v.push((first, rate, ch));
    for f in rest {
        if f != first {
            v.push((f, rate, ch));
        }
    }
    v
}

/// Nombre de trames correspondant à une période en hns (100 ns) à `rate` Hz.
/// Pure : testable hors Windows.
pub fn period_frames(period_hns: i64, rate: u32) -> u32 {
    if period_hns <= 0 || rate == 0 {
        return 1;
    }
    ((period_hns as u128 * rate as u128) / 10_000_000).max(1) as u32
}

/// Construit les octets WAVEFORMATEX (ou WAVEFORMATEXTENSIBLE) d'un candidat.
/// Pure : testable hors Windows. Retourne (octets, longueur utile).
pub fn build_wfx(fmt: DeviceFormat, rate: u32, channels: u16) -> (Vec<u8>, usize) {
    let ch = channels.max(1);
    let (tag, container_bits, valid_bits, sub) = match fmt {
        DeviceFormat::Pcm16 => (1u16, 16u16, 0u16, None),
        DeviceFormat::Pcm24Packed => (1, 24, 0, None),
        DeviceFormat::Pcm32 => (1, 32, 0, None),
        DeviceFormat::F32 => (3, 32, 0, None),
        DeviceFormat::Pcm24In32 => (0xFFFE, 32, 24, Some(0x0000_0001u32)), // KSDATAFORMAT_SUBTYPE_PCM
    };
    let block_align = ch * (container_bits / 8);
    let avg = rate * block_align as u32;
    let mut b = Vec::with_capacity(40);
    b.extend_from_slice(&tag.to_le_bytes());
    b.extend_from_slice(&ch.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&avg.to_le_bytes());
    b.extend_from_slice(&block_align.to_le_bytes());
    b.extend_from_slice(&container_bits.to_le_bytes());
    if tag == 0xFFFE {
        b.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        b.extend_from_slice(&valid_bits.to_le_bytes()); // wValidBitsPerSample
        b.extend_from_slice(&0x3u32.to_le_bytes()); // dwChannelMask (stéréo)
                                                    // SubFormat GUID little-endian : Data1, Data2, Data3, Data4
        b.extend_from_slice(&sub.unwrap_or(0).to_le_bytes());
        b.extend_from_slice(&0x0000u16.to_le_bytes());
        b.extend_from_slice(&0x0010u16.to_le_bytes());
        b.extend_from_slice(&[0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
        (b, 40)
    } else {
        b.extend_from_slice(&0u16.to_le_bytes()); // cbSize
        (b, 18)
    }
}

/// Sonde sans ouvrir de flux : true si le device accepte AU MOINS un format
/// en mode EXCLUSIF (IsFormatSupported uniquement, aucun Initialize).
pub fn probe_any(device_name: Option<&str>) -> bool {
    imp::probe_any(device_name)
}

/// Ouvre le device en mode EXCLUSIF au meilleur format pour `spec`.
/// En cas d'échec, rend le `Cons` intact pour repli sur le chemin cpal.
pub fn open(
    device_name: Option<&str>,
    spec: &OutputSpec,
    cons: crate::output::Cons,
    params: RenderParams,
) -> Result<ExclusiveStream, (String, crate::output::Cons)> {
    imp::open(device_name, spec, cons, params).map(|inner| ExclusiveStream { inner })
}

pub struct ExclusiveStream {
    inner: imp::ExclusiveStream,
}

impl ExclusiveStream {
    pub fn pause(&self) {
        self.inner.pause()
    }
    pub fn resume(&self) {
        self.inner.resume()
    }
    /// Libère le device (Stop + fermeture du thread).
    pub fn stop(&mut self) {
        self.inner.stop()
    }
    /// Description du backend actif, ex. « exclusif 176400 Hz, 24 bits PCM, 2 ch ».
    pub fn label(&self) -> String {
        self.inner.label()
    }
    pub fn format(&self) -> (DeviceFormat, u32, u16) {
        self.inner.format()
    }
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

// ---------------------------------------------------------------------------
// Implémentation Windows
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use super::{build_wfx, candidates, DeviceFormat};
    use crate::decoder::OutputSpec;
    use crate::eq::EqProcessor;
    use crate::output::{compute_gain, render_samples, Cons, RenderParams, Stash, WriteOut};
    use crate::wasapi_dop::{find_device_by_name, ComGuard};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use windows::core::GUID;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::Media::Audio::{
        IAudioClient, IAudioRenderClient, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_EXCLUSIVE,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::CLSCTX_ALL;
    use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};

    /// Tampon aligné pour caster vers *const WAVEFORMATEX (align 4).
    #[repr(C, align(4))]
    struct WfxBuf([u8; 40]);

    /// Wrapper Send pour handles/pointeurs COM : l'apartment est initialisé en
    /// COINIT_MULTITHREADED, l'usage cross-thread est donc sûr.
    struct Send<T>(T);
    unsafe impl<T> std::marker::Send for Send<T> {}

    // Depile le wrapper par appel de fonction : la capture precise (edition 2021)
    // prend alors la variable entiere (Send) et non le champ interne.
    fn unwrap<T>(w: Send<T>) -> T {
        w.0
    }

    fn wfx_ptr(fmt: DeviceFormat, rate: u32, ch: u16) -> (WfxBuf, usize) {
        let (bytes, len) = build_wfx(fmt, rate, ch);
        let mut buf = WfxBuf([0u8; 40]);
        buf.0[..len].copy_from_slice(&bytes);
        (buf, len)
    }

    enum Cmd {
        Pause,
        Resume,
        Stop,
    }

    pub struct ExclusiveStream {
        cmd_tx: mpsc::Sender<Cmd>,
        stop_ev: Send<HANDLE>,
        handle: Option<std::thread::JoinHandle<()>>,
        label: String,
        fmt: DeviceFormat,
        rate: u32,
        channels: u16,
    }

    impl ExclusiveStream {
        pub fn pause(&self) {
            let _ = self.cmd_tx.send(Cmd::Pause);
        }
        pub fn resume(&self) {
            let _ = self.cmd_tx.send(Cmd::Resume);
        }
        pub fn stop(&mut self) {
            let _ = self.cmd_tx.send(Cmd::Stop);
            let _ = unsafe { windows::Win32::System::Threading::SetEvent(self.stop_ev.0) };
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            let _ = unsafe { CloseHandle(self.stop_ev.0) };
        }
        pub fn label(&self) -> String {
            self.label.clone()
        }
        pub fn format(&self) -> (DeviceFormat, u32, u16) {
            (self.fmt, self.rate, self.channels)
        }
    }

    pub fn probe_any(device_name: Option<&str>) -> bool {
        let Ok(_guard) = ComGuard::new() else {
            return false;
        };
        let Ok(dev) = (unsafe { find_device_by_name(device_name) }) else {
            return false;
        };
        let Ok(client) = (unsafe { dev.Activate::<IAudioClient>(CLSCTX_ALL, None) }) else {
            return false;
        };
        for fmt in [
            DeviceFormat::Pcm16,
            DeviceFormat::Pcm24Packed,
            DeviceFormat::Pcm32,
            DeviceFormat::F32,
        ] {
            for rate in [44_100u32, 48_000, 96_000, 176_400, 192_000] {
                let (buf, len) = wfx_ptr(fmt, rate, 2);
                let wfx = unsafe { &*(buf.0.as_ptr() as *const WAVEFORMATEX) };
                let _ = len;
                // En mode EXCLUSIF, pClosestMatch DOIT etre NULL :
                // S_OK = format accepte tel quel, sinon erreur.
                if unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, wfx, None) }
                    .is_ok()
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn open(
        device_name: Option<&str>,
        spec: &OutputSpec,
        cons: Cons,
        params: RenderParams,
    ) -> Result<ExclusiveStream, (String, Cons)> {
        macro_rules! fail {
            ($e:expr) => {
                return Err((($e).to_string(), cons))
            };
        }
        let _guard = match ComGuard::new() {
            Ok(g) => g,
            Err(e) => fail!(format!("COM: {e}")),
        };
        let dev = match unsafe { find_device_by_name(device_name) } {
            Ok(d) => d,
            Err(e) => fail!(e),
        };
        let client: IAudioClient = match unsafe { dev.Activate(CLSCTX_ALL, None) } {
            Ok(c) => c,
            Err(e) => fail!(format!("Activate: {e}")),
        };

        // Périodes du device (hns, 100 ns) : le mode polling exige
        // hnsBufferDuration == hnsPeriodicity ; on essaie la période par
        // défaut (10 ms) puis la période min, certaines cartes USB n'acceptent
        // que l'une des deux.
        let mut def_period: i64 = 0;
        let mut min_period: i64 = 0;
        let _ = unsafe { client.GetDevicePeriod(Some(&mut def_period), Some(&mut min_period)) };
        let mut periods: Vec<i64> = Vec::new();
        for p in [def_period, min_period, def_period / 2] {
            if p > 0 && !periods.contains(&p) {
                periods.push(p);
            }
        }
        if periods.is_empty() {
            periods.push(100_000); // 10 ms par défaut
        }

        // Essaie chaque candidat en EXCLUSIF : IsFormatSupported puis Initialize,
        // d'abord en mode POLLING (iBasso DC03-Pro et consorts), puis en mode
        // EVENTCALLBACK (forme standard des autres DAC).
        let mut opened: Option<(DeviceFormat, u32, u16, bool, i64)> = None;
        'cand: for (fmt, rate, ch) in candidates(spec) {
            let (buf, _len) = wfx_ptr(fmt, rate, ch);
            let wfx = unsafe { &*(buf.0.as_ptr() as *const WAVEFORMATEX) };
            // IsFormatSupported est purement consultatif : certains pilotes
            // (iBasso DC03-Pro) y repondent UNSUPPORTED_FORMAT pour du 24 bits
            // a 176400 Hz alors que Initialize l'accepte. On tente donc
            // Initialize quoi qu'il arrive — c'est lui qui fait foi.
            let sup = unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, wfx, None) };
            if sup.is_err() {
                tracing::debug!(
                    "exclusif: IsFormatSupported refuse {fmt:?} {rate} Hz (continue quand meme)"
                );
            }
            // 1) POLLING : pas de flag EVENTCALLBACK, buffer == périodicité.
            for &p in &periods {
                let hr =
                    unsafe { client.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, 0, p, p, wfx, None) };
                if hr.is_ok() {
                    opened = Some((fmt, rate, ch, false, p));
                    break 'cand;
                }
            }
            // 2) EVENTCALLBACK : buffer = 2 périodes, périodicité = 1 période.
            for &p in &periods {
                let hr = unsafe {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_EXCLUSIVE,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        2 * p,
                        p,
                        wfx,
                        None,
                    )
                };
                if hr.is_ok() {
                    opened = Some((fmt, rate, ch, true, p));
                    break 'cand;
                }
            }
        }
        let (fmt, rate, ch, event_mode, period_hns) = match opened {
            Some(x) => x,
            None => fail!(format!(
                "aucun format accepté en mode EXCLUSIF sur '{}' pour {} Hz",
                device_name.unwrap_or("défaut"),
                spec.rate
            )),
        };

        let buf_frames = match unsafe { client.GetBufferSize() } {
            Ok(n) => n,
            Err(e) => fail!(format!("GetBufferSize: {e}")),
        };
        // Le handle d'événement n'existe qu'en mode event-driven.
        let event = if event_mode {
            match unsafe { CreateEventW(None, false, false, None) } {
                Ok(h) => Some(h),
                Err(e) => fail!(format!("CreateEventW: {e}")),
            }
        } else {
            None
        };
        let stop_ev = match unsafe { CreateEventW(None, false, false, None) } {
            Ok(h) => h,
            Err(e) => fail!(format!("CreateEventW(stop): {e}")),
        };
        if let Some(ev) = event {
            if let Err(e) = unsafe { client.SetEventHandle(ev) } {
                let _ = unsafe { CloseHandle(ev) };
                let _ = unsafe { CloseHandle(stop_ev) };
                fail!(format!("SetEventHandle: {e}"));
            }
        }
        let render: IAudioRenderClient = match unsafe { client.GetService() } {
            Ok(r) => r,
            Err(e) => fail!(format!("GetService(render): {e}")),
        };
        if let Err(e) = unsafe { client.Start() } {
            fail!(format!("Start: {e}"));
        }

        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let frame_bytes = fmt.frame_bytes(ch);
        let out_ch = ch as usize;
        let src_ch = spec.channels.max(1) as usize;
        let dop = spec.is_dsd_dop;
        let label = fmt.describe(rate, ch);

        // Dispatch du thread worker sur le type d'échantillon du conteneur.
        // Les handles/objets COM sont wrappés en Send (apartment MTA).
        let handle = match fmt {
            DeviceFormat::Pcm16 => start_worker::<i16>(
                fmt,
                rate,
                ch,
                buf_frames,
                frame_bytes,
                out_ch,
                src_ch,
                dop,
                event_mode,
                period_hns,
                event,
                stop_ev,
                cmd_rx,
                cons,
                params,
                client,
                render,
            ),
            DeviceFormat::Pcm24Packed | DeviceFormat::Pcm24In32 | DeviceFormat::Pcm32 => {
                start_worker::<i32>(
                    fmt,
                    rate,
                    ch,
                    buf_frames,
                    frame_bytes,
                    out_ch,
                    src_ch,
                    dop,
                    event_mode,
                    period_hns,
                    event,
                    stop_ev,
                    cmd_rx,
                    cons,
                    params,
                    client,
                    render,
                )
            }
            DeviceFormat::F32 => start_worker::<f32>(
                fmt,
                rate,
                ch,
                buf_frames,
                frame_bytes,
                out_ch,
                src_ch,
                dop,
                event_mode,
                period_hns,
                event,
                stop_ev,
                cmd_rx,
                cons,
                params,
                client,
                render,
            ),
        };
        Ok(ExclusiveStream {
            cmd_tx,
            stop_ev: Send(stop_ev),
            handle: Some(handle),
            label,
            fmt,
            rate,
            channels: ch,
        })
    }

    // Thread worker, générique sur le type d'échantillon du conteneur.
    // `event_mode` = true -> attente sur handle d'événement WASAPI ;
    // false -> mode polling (le DAC ne signale rien, on cadence nous-mêmes).
    #[allow(clippy::too_many_arguments)]
    fn start_worker<T: WriteOut>(
        fmt: DeviceFormat,
        rate: u32,
        _ch: u16,
        buf_frames: u32,
        frame_bytes: usize,
        out_ch: usize,
        src_ch: usize,
        dop: bool,
        event_mode: bool,
        period_hns: i64,
        event: Option<HANDLE>,
        stop_ev: HANDLE,
        cmd_rx: mpsc::Receiver<Cmd>,
        cons: Cons,
        params: RenderParams,
        client: IAudioClient,
        render: IAudioRenderClient,
    ) -> std::thread::JoinHandle<()> {
        let client = Send(client);
        let render = Send(render);
        let event = Send(event);
        let stop_ev = Send(stop_ev);
        let cons = Send(cons);
        std::thread::Builder::new()
            .name("auris-wasapi-exclusive".into())
            .spawn(move || {
                let client = unwrap(client);
                let render = unwrap(render);
                let event = unwrap(event);
                let stop_ev = unwrap(stop_ev);
                let mut cons = unwrap(cons);
                let forward_browser = !dop;
                let mut eq_proc = EqProcessor::new();
                let mut stash: Option<Stash> = None;
                let mut paused = false;
                // En mode polling on pousse au plus ~1 période par itération ;
                // en dessous d'un seuil on dort un quart de période.
                let period_frames = super::period_frames(period_hns, rate);
                let nap = std::time::Duration::from_micros(((period_hns as u64) / 40).max(100));
                let close_event = |ev: Option<HANDLE>| {
                    if let Some(ev) = ev {
                        let _ = unsafe { CloseHandle(ev) };
                    }
                };
                loop {
                    while let Ok(c) = cmd_rx.try_recv() {
                        match c {
                            Cmd::Stop => {
                                let _ = unsafe { client.Stop() };
                                close_event(event);
                                return;
                            }
                            Cmd::Pause => {
                                if !paused {
                                    let _ = unsafe { client.Stop() };
                                    paused = true;
                                }
                            }
                            Cmd::Resume => {
                                if paused {
                                    let _ = unsafe { client.Start() };
                                    paused = false;
                                }
                            }
                        }
                    }
                    if paused {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        continue;
                    }
                    if event_mode {
                        let ev = event.expect("handle événement requis en mode event");
                        let handles = [stop_ev, ev];
                        let w = unsafe { WaitForMultipleObjects(&handles, false, 50) };
                        if w == WAIT_OBJECT_0 {
                            let _ = unsafe { client.Stop() };
                            close_event(event);
                            return;
                        }
                        if w == WAIT_TIMEOUT {
                            continue;
                        }
                    } else {
                        // Mode polling : stop non bloquant, puis cadence.
                        let w = unsafe {
                            windows::Win32::System::Threading::WaitForSingleObject(stop_ev, 0)
                        };
                        if w == WAIT_OBJECT_0 {
                            let _ = unsafe { client.Stop() };
                            return;
                        }
                    }
                    let padding = unsafe { client.GetCurrentPadding() }.unwrap_or(0);
                    let writable = (buf_frames.saturating_sub(padding)) as usize;
                    let n = if event_mode {
                        writable
                    } else {
                        writable.min(period_frames as usize)
                    };
                    if n == 0 || (!event_mode && n < 32) {
                        if !event_mode {
                            std::thread::sleep(nap);
                        }
                        continue;
                    }
                    let mut buf = vec![T::zero(); n * out_ch];
                    let gain = compute_gain(
                        params.volume.load(Ordering::Relaxed),
                        params.max_volume.load(Ordering::Relaxed),
                        params.curve.load(Ordering::Relaxed),
                        params.vol_mode.load(Ordering::Relaxed),
                        params.muted.load(Ordering::Relaxed),
                    );
                    let _ = render_samples::<T>(
                        &mut buf,
                        &mut stash,
                        &mut cons,
                        &params.gen,
                        src_ch,
                        out_ch,
                        &params.eq,
                        &mut eq_proc,
                        rate,
                        gain,
                        forward_browser,
                        &params.out_mode,
                        &params.browser_tx,
                        &params.frames_out,
                    );
                    let mut bytes = Vec::with_capacity(n * frame_bytes);
                    let packed = fmt.packed24();
                    for s in &buf {
                        s.push_le(&mut bytes, packed);
                    }
                    match unsafe { render.GetBuffer(n as u32) } {
                        Ok(ptr) => {
                            let dst =
                                unsafe { std::slice::from_raw_parts_mut(ptr, n * frame_bytes) };
                            let copy = bytes.len().min(dst.len());
                            dst[..copy].copy_from_slice(&bytes[..copy]);
                            for b in &mut dst[copy..] {
                                *b = 0;
                            }
                            let flags: u32 = if copy == 0 {
                                AUDCLNT_BUFFERFLAGS_SILENT.0 as u32
                            } else {
                                0
                            };
                            let _ = unsafe { render.ReleaseBuffer(n as u32, flags) };
                        }
                        Err(e) => {
                            tracing::error!("wasapi exclusif: GetBuffer: {e}");
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }
            })
            .expect("spawn worker wasapi exclusif")
    }

    // Ré-export pour tests internes
    #[allow(dead_code)]
    fn _guid() -> GUID {
        GUID::zeroed()
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::DeviceFormat;
    use crate::decoder::OutputSpec;
    use crate::output::{Cons, RenderParams};

    pub struct ExclusiveStream;

    impl ExclusiveStream {
        pub fn pause(&self) {}
        pub fn resume(&self) {}
        pub fn stop(&mut self) {}
        pub fn label(&self) -> String {
            String::new()
        }
        pub fn format(&self) -> (DeviceFormat, u32, u16) {
            (DeviceFormat::F32, 0, 0)
        }
    }

    pub fn probe_any(_device_name: Option<&str>) -> bool {
        false
    }

    pub fn open(
        _device_name: Option<&str>,
        _spec: &OutputSpec,
        cons: Cons,
        _params: RenderParams,
    ) -> Result<ExclusiveStream, (String, Cons)> {
        Err((
            "mode exclusif WASAPI disponible sous Windows uniquement".into(),
            cons,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_pcm(rate: u32, bits: u8) -> OutputSpec {
        OutputSpec {
            rate,
            channels: 2,
            bits,
            is_dsd_dop: false,
        }
    }

    #[test]
    fn candidates_dop_porteuse_24_bits_packed_dabord() {
        let c = candidates(&OutputSpec {
            rate: 176_400,
            channels: 2,
            bits: 24,
            is_dsd_dop: true,
        });
        assert_eq!(c[0], (DeviceFormat::Pcm24Packed, 176_400, 2));
        assert!(c.iter().all(|(_, r, _)| *r == 176_400));
    }

    #[test]
    fn candidates_pcm16_prefere_le_conteneur_16_bits() {
        let c = candidates(&spec_pcm(44_100, 16));
        assert_eq!(c[0], (DeviceFormat::Pcm16, 44_100, 2));
    }

    #[test]
    fn candidates_pcm24_prefere_le_24_bits_packed() {
        let c = candidates(&spec_pcm(96_000, 24));
        assert_eq!(c[0], (DeviceFormat::Pcm24Packed, 96_000, 2));
        // tous les candidats au même taux (pas de rééchantillonnage)
        assert!(c.iter().all(|(_, r, _)| *r == 96_000));
    }

    #[test]
    fn wfx_pcm24_packed_18_octets() {
        let (b, len) = build_wfx(DeviceFormat::Pcm24Packed, 176_400, 2);
        assert_eq!(len, 18);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 1); // WAVE_FORMAT_PCM
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), 2);
        assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]), 176_400);
        assert_eq!(u16::from_le_bytes([b[12], b[13]]), 6); // block align
        assert_eq!(u16::from_le_bytes([b[14], b[15]]), 24); // bits
    }

    #[test]
    fn wfx_extensible_24in32_40_octets_guid_pcm() {
        let (b, len) = build_wfx(DeviceFormat::Pcm24In32, 176_400, 2);
        assert_eq!(len, 40);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 0xFFFE);
        assert_eq!(u16::from_le_bytes([b[14], b[15]]), 32); // conteneur 32
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 22); // cbSize
        assert_eq!(u16::from_le_bytes([b[18], b[19]]), 24); // valid bits
        assert_eq!(
            &b[24..40],
            &[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]
        ); // KSDATAFORMAT_SUBTYPE_PCM
    }

    #[test]
    fn period_frames_conversion_hns() {
        // 10 ms à 48000 Hz = 480 trames ; 3 ms à 176400 Hz = 529 trames.
        assert_eq!(period_frames(100_000, 48_000), 480);
        assert_eq!(period_frames(30_000, 176_400), 529);
        assert_eq!(period_frames(0, 48_000), 1);
        assert_eq!(period_frames(100_000, 0), 1);
    }

    #[test]
    fn frame_bytes() {
        assert_eq!(DeviceFormat::Pcm16.frame_bytes(2), 4);
        assert_eq!(DeviceFormat::Pcm24Packed.frame_bytes(2), 6);
        assert_eq!(DeviceFormat::Pcm24In32.frame_bytes(2), 8);
        assert_eq!(DeviceFormat::F32.frame_bytes(2), 8);
    }

    #[test]
    fn describe_inclus_taux_et_bits() {
        assert_eq!(
            DeviceFormat::Pcm24Packed.describe(176_400, 2),
            "exclusif 176400 Hz, 24 bits PCM, 2 ch"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn stub_hors_windows() {
        assert!(!probe_any(None));
    }
}
