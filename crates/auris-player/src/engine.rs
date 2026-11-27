//! Moteur de lecture: file d'attente, commandes, worker de decode par piste,
//! pont vers la sortie cpal via un anneau, et horloge de position.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use auris_core::{
    BrowserFrame, OutputMode, PlayStatus, PlaybackSettings, PlayerState, RepeatMode, Track,
    VolumeCurve, VolumeMode, WsEvent,
};
use auris_sources::SourcesManager;
use ringbuf::traits::{Observer, Producer};
use tokio::sync::broadcast;

use crate::decoder::{open_decoder_with_path, Chunk, Decoder, OutputSpec, TrackMeta};
use crate::eq::{EqBandParams, EqConfig};
use crate::mixer::{spawn_mixer, MixerCmd, MixerHandle};
use crate::output::{make_ring, AudioOutput, Cons, Prod};

const RING_CHUNKS: usize = 96;
const CHUNK_FRAMES: usize = 8192;

/// Commandes envoyees au moteur (thread dedie).
#[derive(Debug)]
pub enum Command {
    PlayUri(String),
    PlayQueue(Vec<Track>, usize),
    Enqueue(Box<Track>),
    /// Insere des pistes juste apres la piste en cours (« Lire ensuite »).
    EnqueueNext(Vec<Track>),
    Pause,
    Resume,
    Stop,
    Next,
    Prev,
    Seek(f64),
    SetVolume(u8),
    SetMuted(bool),
    SetRepeat(RepeatMode),
    SetShuffle(bool),
    ClearQueue,
    RemoveQueueItem(usize),
    /// Joue la piste à l'index donné de la file (exposé pour socket.io #45).
    PlayQueueIndex(usize),
    SelectDevice(Option<String>),
    ApplyPlayback(Box<PlaybackSettings>),
    SetOutputMode(OutputMode),
    SetExclusive(bool),
}

enum WorkerCmd {
    Seek(f64),
    /// Gain de normalisation loudness appliqué aux échantillons PCM (1.0 = neutre).
    SetGain(f32),
    Stop,
}

/// Messages du worker de decode vers le moteur.
pub enum WorkerMsg {
    Ready {
        spec: OutputSpec,
        meta: Box<TrackMeta>,
        duration: Option<f64>,
        cons: Cons,
    },
    Seeked(f64),
    EndOfTrack,
    Error(String),
}

struct WorkerHandle {
    msg_rx: mpsc::Receiver<WorkerMsg>,
    cmd_tx: mpsc::Sender<WorkerCmd>,
    join: Option<JoinHandle<()>>,
}

/// Piste suivante préchargée (crossfade / gapless) en attente de promotion.
struct PendingNext {
    worker: WorkerHandle,
    index: usize,
    uri: String,
}

impl WorkerHandle {
    fn stop(mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Generateur pseudo-aleatoire leger (xorshift64), evite une dependance rand.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn track_from_uri(uri: &str, meta: &TrackMeta, duration: Option<f64>) -> Track {
    let source_id = auris_sources::parse_src_uri(uri)
        .map(|(id, _)| id)
        .unwrap_or_else(|| "http".to_string());
    let mut t = Track::new(uri.to_string(), source_id);
    if let Some(v) = &meta.title {
        t.title = v.clone();
    }
    if let Some(v) = &meta.artist {
        t.artist = v.clone();
    }
    if let Some(v) = &meta.album {
        t.album = v.clone();
    }
    if let Some(v) = &meta.album_artist {
        t.album_artist = v.clone();
    } else {
        t.album_artist = t.artist.clone();
    }
    t.track_number = meta.track_number;
    t.disc_number = meta.disc_number;
    t.year = meta.year;
    t.genre = meta.genre.clone();
    t.duration_secs = duration.or(meta.duration_secs);
    t.format = meta.format;
    t.has_cover = meta.has_cover;
    t.composer = meta.composer.clone();
    t.producer = meta.producer.clone();
    t.label = meta.label.clone();
    t.publisher = meta.publisher.clone();
    t.isrc = meta.isrc.clone();
    t.copyright = meta.copyright.clone();
    t.lyrics = meta.lyrics.clone();
    t.mb_track_id = meta.mb_track_id.clone();
    t.finalize_keys();
    t
}

/// Convertit une spec de sortie en [`auris_core::AudioFormat`] (pour les flux
/// sans métadonnées, ex. capture line-in #75).
fn spec_format(spec: crate::decoder::OutputSpec) -> auris_core::AudioFormat {
    if spec.is_dsd_dop {
        auris_core::AudioFormat::Dsd {
            sample_rate: spec.rate,
            channels: spec.channels,
        }
    } else {
        auris_core::AudioFormat::Pcm {
            sample_rate: spec.rate,
            bits: spec.bits,
            channels: spec.channels,
        }
    }
}

/// Lance le worker de decode d'une piste. Les chunks poussés par le worker
/// sont tagués avec la génération *courante* (lue à chaque push) : un seek
/// qui bump la génération purge ainsi les vieux chunks encore dans l'anneau.
fn spawn_worker(
    sources: Arc<SourcesManager>,
    uri: String,
    gen: Arc<AtomicU64>,
) -> (
    mpsc::Receiver<WorkerMsg>,
    mpsc::Sender<WorkerCmd>,
    JoinHandle<()>,
) {
    let (msg_tx, msg_rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                let _ = msg_tx.send(WorkerMsg::Error(format!("runtime: {e}")));
                return;
            }
        };
        let reader = match rt.block_on(sources.open_uri(&uri)) {
            Ok(r) => r,
            Err(e) => {
                let _ = msg_tx.send(WorkerMsg::Error(format!("ouverture {uri}: {e}")));
                return;
            }
        };
        let ext = uri
            .rsplit('.')
            .next()
            .unwrap_or("")
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .to_string();
        // Chemin local si la source est un montage local/USB : le repli ffmpeg
        // seek alors nativement dans le conteneur au lieu de re-decoder (#54).
        let local_path = sources.local_path_for_uri(&uri);
        let mut decoder: Box<dyn Decoder> =
            match open_decoder_with_path(reader, &ext, TrackMeta::default(), local_path) {
                Ok(d) => d,
                Err(e) => {
                    let _ = msg_tx.send(WorkerMsg::Error(format!("decode {uri}: {e}")));
                    return;
                }
            };
        let spec = decoder.spec();
        let meta = Box::new(decoder.meta().clone());
        let duration = decoder.duration_secs();
        let (mut prod, cons): (Prod, Cons) = make_ring(RING_CHUNKS);
        if msg_tx
            .send(WorkerMsg::Ready {
                spec,
                meta,
                duration,
                cons,
            })
            .is_err()
        {
            return;
        }
        let mut pushed_frames = 0usize;
        let mut gain: f32 = 1.0;
        loop {
            match cmd_rx.try_recv() {
                Ok(WorkerCmd::Seek(s)) => {
                    if let Err(e) = decoder.seek(s) {
                        let _ = msg_tx.send(WorkerMsg::Error(format!("seek: {e}")));
                    }
                    let _ = msg_tx.send(WorkerMsg::Seeked(s));
                }
                Ok(WorkerCmd::SetGain(g)) => gain = g,
                Ok(WorkerCmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            match decoder.next_chunk(CHUNK_FRAMES) {
                Ok(Some(mut data)) => {
                    pushed_frames += data.frames(spec.channels as usize);
                    if gain != 1.0 {
                        if let crate::decoder::ChunkData::F32(v) = &mut data {
                            for s in v.iter_mut() {
                                *s *= gain;
                            }
                        }
                    }
                    let mut chunk = Chunk {
                        gen: gen.load(Ordering::SeqCst),
                        data,
                    };
                    loop {
                        match prod.try_push(chunk) {
                            Ok(()) => break,
                            Err(c) => {
                                chunk = c;
                                if matches!(
                                    cmd_rx.try_recv(),
                                    Ok(WorkerCmd::Stop) | Err(mpsc::TryRecvError::Disconnected)
                                ) {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                    }
                }
                Ok(None) => {
                    // Fin du decode: on attend que le callback audio ait vide
                    // l'anneau avant de signaler EndOfTrack, sinon le moteur
                    // stopperait la lecture alors que du son reste buffered.
                    loop {
                        match cmd_rx.try_recv() {
                            Ok(WorkerCmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => return,
                            Ok(WorkerCmd::Seek(_)) | Ok(WorkerCmd::SetGain(_)) => {}
                            Err(mpsc::TryRecvError::Empty) => {}
                        }
                        if prod.is_empty() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    tracing::debug!(
                        "worker: EOF decode apres {pushed_frames} trames; occupied={} empty={}",
                        prod.occupied_len(),
                        prod.is_empty()
                    );
                    let _ = msg_tx.send(WorkerMsg::EndOfTrack);
                    break;
                }
                Err(e) => {
                    let _ = msg_tx.send(WorkerMsg::Error(e.to_string()));
                    break;
                }
            }
        }
    });
    (msg_rx, cmd_tx, handle)
}

/// Callback uri -> gain de normalisation loudness (None = pas de gain).
pub type LoudnessLookup = Arc<dyn Fn(&str) -> Option<f32> + Send + Sync>;

struct Engine {
    sources: Arc<SourcesManager>,
    cmd_rx: mpsc::Receiver<Command>,
    state: Arc<RwLock<PlayerState>>,
    events: broadcast::Sender<WsEvent>,
    volume: Arc<AtomicU8>,
    max_volume: Arc<AtomicU8>,
    /// 0 = logarithmique, 1 = linéaire.
    curve: Arc<AtomicU8>,
    /// 0 = logiciel, 1 = bit-perfect (désactivé).
    vol_mode: Arc<AtomicU8>,
    muted: Arc<AtomicBool>,
    frames_out: Arc<AtomicU64>,
    gen: Arc<AtomicU64>,
    cur_rate: Arc<AtomicU64>,
    output: Option<AudioOutput>,
    device_name: Option<String>,
    /// 0 = DAC local, 1 = « Jouer ici » (navigateur).
    out_mode: Arc<AtomicU8>,
    /// Mode WASAPI exclusif demandé (Windows, bit-perfect).
    exclusive: Arc<AtomicBool>,
    browser_tx: broadcast::Sender<BrowserFrame>,
    browser_spec: Arc<RwLock<Option<(u32, u16)>>>,
    /// Config de l'égaliseur paramétrique, poussée à chaud (versionnée).
    eq: Arc<RwLock<EqConfig>>,
    worker: Option<WorkerHandle>,
    /// Mixer de la piste courante (crossfade / gapless vers la suivante).
    mixer: Option<MixerHandle>,
    /// EndOfTrack reçu mais en attente que l'anneau de sortie soit drainé.
    pending_eot: bool,
    /// Piste suivante préchargée, en attente de promotion.
    pending_next: Option<PendingNext>,
    cur_spec: Option<OutputSpec>,
    pending_uri: Option<String>,
    seek_after_ready: Option<f64>,
    /// Durée du fondu enchaîné en ms (0 = enchaînement direct).
    crossfade_ms: Arc<AtomicU64>,
    /// Préchargement de la piste suivante (gapless).
    gapless: Arc<AtomicBool>,
    /// Lookup de gain loudness par uri (normalisation R128, #37).
    loudness_lookup: Arc<RwLock<Option<LoudnessLookup>>>,
    rng: XorShift,
}

impl Engine {
    fn publish(&self) {
        let st = self.state.read().unwrap().clone();
        let _ = self.events.send(WsEvent::state(&st));
    }

    fn publish_msg(&self, kind: &str, msg: &str) {
        let _ = self.events.send(WsEvent::message(kind, msg));
    }

    fn set_status(&self, status: PlayStatus) {
        self.state.write().unwrap().status = status;
    }

    fn start_track(&mut self, uri: String, seek_after: Option<f64>) {
        self.gen.fetch_add(1, Ordering::SeqCst);
        self.frames_out.store(0, Ordering::SeqCst);
        self.seek_after_ready = seek_after;
        self.pending_uri = Some(uri.clone());
        self.pending_eot = false;
        {
            let mut st = self.state.write().unwrap();
            st.is_live = false;
            st.stream_title = None;
        }
        if let Some(p) = self.pending_next.take() {
            p.worker.stop();
        }
        if let Some(m) = self.mixer.take() {
            m.stop();
        }
        if let Some(w) = self.worker.take() {
            w.stop();
        }
        let gain = self
            .loudness_lookup
            .read()
            .unwrap()
            .as_ref()
            .and_then(|f| f(&uri))
            .unwrap_or(1.0);
        let (msg_rx, cmd_tx, join) = spawn_worker(self.sources.clone(), uri, self.gen.clone());
        let _ = cmd_tx.send(WorkerCmd::SetGain(gain));
        self.worker = Some(WorkerHandle {
            msg_rx,
            cmd_tx,
            join: Some(join),
        });
    }

    fn stop_playback(&mut self) {
        self.pending_eot = false;
        if let Some(p) = self.pending_next.take() {
            p.worker.stop();
        }
        if let Some(m) = self.mixer.take() {
            m.stop();
        }
        if let Some(w) = self.worker.take() {
            w.stop();
        }
        if let Some(o) = &mut self.output {
            o.stop();
        }
        self.gen.fetch_add(1, Ordering::SeqCst);
        self.frames_out.store(0, Ordering::SeqCst);
        let mut st = self.state.write().unwrap();
        st.status = PlayStatus::Stopped;
        st.position_secs = 0.0;
        st.queue_index = None;
        st.is_live = false;
        st.stream_title = None;
        drop(st);
        self.publish();
    }

    fn play_queue_index(&mut self, idx: usize) {
        let uri = {
            let st = self.state.read().unwrap();
            st.queue.get(idx).map(|t| t.uri.clone())
        };
        if let Some(uri) = uri {
            self.state.write().unwrap().queue_index = Some(idx);
            self.start_track(uri, None);
            self.set_status(PlayStatus::Playing);
            self.publish();
        }
    }

    /// Enchaînement après EndOfTrack réellement consommé (anneau de sortie
    /// drainé) : repeat one, piste suivante, ou stop.
    fn advance_after_eot(&mut self) {
        // la piste s'est terminée avant que le préchargement de B soit prêt :
        // on abandonne le fondu, enchaînement normal
        if let Some(p) = self.pending_next.take() {
            p.worker.stop();
        }
        let repeat = self.state.read().unwrap().repeat;
        if repeat == RepeatMode::One {
            let uri = self
                .state
                .read()
                .unwrap()
                .current
                .as_ref()
                .map(|t| t.uri.clone());
            if let Some(uri) = uri {
                self.start_track(uri, Some(0.0));
            }
        } else if let Some(i) = self.next_index() {
            self.play_queue_index(i);
        } else {
            self.stop_playback();
        }
    }

    fn next_index(&mut self) -> Option<usize> {
        let st = self.state.read().unwrap();
        let i = st.queue_index?;
        let n = st.queue.len();
        if n == 0 {
            return None;
        }
        if st.shuffle && n > 1 {
            let mut j = (self.rng.next() as usize) % n;
            if j == i {
                j = (j + 1) % n;
            }
            return Some(j);
        }
        if i + 1 < n {
            Some(i + 1)
        } else if st.repeat == RepeatMode::All {
            Some(0)
        } else {
            None
        }
    }

    fn prev_index(&mut self) -> Option<usize> {
        let st = self.state.read().unwrap();
        let i = st.queue_index?;
        let n = st.queue.len();
        if st.position_secs > 3.0 {
            return Some(i); // on recommence la piste courante
        }
        if n == 0 {
            return None;
        }
        if st.shuffle && n > 1 {
            let mut j = (self.rng.next() as usize) % n;
            if j == i {
                j = (j + 1) % n;
            }
            return Some(j);
        }
        if i > 0 {
            Some(i - 1)
        } else if st.repeat == RepeatMode::All {
            Some(n - 1)
        } else {
            Some(0)
        }
    }

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::PlayUri(uri) => {
                let t = track_from_uri(&uri, &TrackMeta::default(), None);
                {
                    let mut st = self.state.write().unwrap();
                    st.queue = vec![t];
                    st.queue_index = Some(0);
                }
                self.start_track(uri, None);
                self.set_status(PlayStatus::Playing);
                self.publish();
            }
            Command::PlayQueue(tracks, idx) => {
                {
                    let mut st = self.state.write().unwrap();
                    st.queue = tracks;
                    st.queue_index = Some(idx.min(st.queue.len().saturating_sub(1)));
                }
                self.play_queue_index(idx);
            }
            Command::PlayQueueIndex(idx) => {
                self.play_queue_index(idx);
            }
            Command::Enqueue(t) => {
                let mut st = self.state.write().unwrap();
                st.queue.push(*t);
                drop(st);
                let q = self.state.read().unwrap().queue.clone();
                let _ = self.events.send(WsEvent::queue(&q));
            }
            Command::EnqueueNext(tracks) => {
                let mut st = self.state.write().unwrap();
                // insertion juste apres la piste en cours (debut si rien ne joue)
                let pos = st
                    .queue_index
                    .map(|i| (i + 1).min(st.queue.len()))
                    .unwrap_or(0);
                let n = tracks.len();
                st.queue.splice(pos..pos, tracks);
                drop(st);
                let q = self.state.read().unwrap().queue.clone();
                let _ = self.events.send(WsEvent::queue(&q));
                if n > 0 {
                    tracing::debug!("engine: {} piste(s) inseree(s) en position {pos}", n);
                }
            }
            Command::Pause => {
                if let Some(o) = &mut self.output {
                    o.pause();
                }
                self.set_status(PlayStatus::Paused);
                self.publish();
            }
            Command::Resume => {
                if let Some(o) = &mut self.output {
                    o.resume();
                }
                self.set_status(PlayStatus::Playing);
                self.publish();
            }
            Command::Stop => self.stop_playback(),
            Command::Next => {
                if let Some(i) = self.next_index() {
                    self.play_queue_index(i);
                } else {
                    self.stop_playback();
                }
            }
            Command::Prev => {
                if let Some(i) = self.prev_index() {
                    self.play_queue_index(i);
                }
            }
            Command::Seek(secs) => {
                // un flux live n'a pas de position fixe : seek ignoré
                if self.state.read().unwrap().is_live {
                    return;
                }
                let dur = self.state.read().unwrap().duration_secs;
                let secs = secs.max(0.0).min(if dur > 0.0 { dur } else { f64::MAX });
                // préchargement en cours : on l'annule (le seek casse le fondu)
                if let Some(p) = self.pending_next.take() {
                    p.worker.stop();
                }
                self.pending_eot = false;
                let rate = self.cur_rate.load(Ordering::SeqCst).max(1);
                if let Some(m) = &self.mixer {
                    let _ = m.cmd_tx.send(MixerCmd::CancelCrossfade);
                    let _ = m
                        .cmd_tx
                        .send(MixerCmd::SetCount((secs * rate as f64) as u64));
                }
                // bump de génération : les chunks encore bufferisés (vieux gen)
                // sont purgés par le mixer et le callback
                self.gen.fetch_add(1, Ordering::SeqCst);
                if let Some(w) = &self.worker {
                    let _ = w.cmd_tx.send(WorkerCmd::Seek(secs));
                }
                self.frames_out
                    .store((secs * rate as f64) as u64, Ordering::SeqCst);
                self.state.write().unwrap().position_secs = secs;
                self.publish();
            }
            Command::SetVolume(v) => {
                let max = self.max_volume.load(Ordering::SeqCst).max(1);
                let v = v.min(max);
                self.volume.store(v, Ordering::SeqCst);
                self.state.write().unwrap().volume = v;
                self.publish();
            }
            Command::ApplyPlayback(p) => {
                self.max_volume.store(p.max_volume.max(1), Ordering::SeqCst);
                self.curve.store(
                    if p.volume_curve == VolumeCurve::Linear {
                        1
                    } else {
                        0
                    },
                    Ordering::SeqCst,
                );
                self.vol_mode.store(
                    if p.volume_mode == VolumeMode::Disabled {
                        1
                    } else {
                        0
                    },
                    Ordering::SeqCst,
                );
                let mut st = self.state.write().unwrap();
                st.max_volume = p.max_volume;
                st.volume_mode = match p.volume_mode {
                    VolumeMode::Disabled => "disabled".into(),
                    VolumeMode::Software => "software".into(),
                };
                // re-borne le volume courant par le nouveau max
                let v = st.volume.min(p.max_volume.max(1));
                st.volume = v;
                drop(st);
                self.volume.store(v, Ordering::SeqCst);
                self.crossfade_ms.store(
                    (p.crossfade_secs.clamp(0.0, 12.0) * 1000.0) as u64,
                    Ordering::SeqCst,
                );
                self.gapless.store(p.gapless, Ordering::SeqCst);
                // EQ paramétrique : maj à chaud (bump de version -> chaîne reconstruite)
                {
                    let mut eq = self.eq.write().unwrap();
                    eq.enabled = p.eq.enabled;
                    eq.bands =
                        p.eq.bands
                            .iter()
                            .map(|b| EqBandParams {
                                freq_hz: b.freq_hz,
                                gain_db: b.gain_db,
                            })
                            .collect();
                    eq.version += 1;
                }
                self.publish();
            }
            Command::SetMuted(m) => {
                self.muted.store(m, Ordering::SeqCst);
                self.state.write().unwrap().muted = m;
                self.publish();
            }
            Command::SetRepeat(r) => {
                self.state.write().unwrap().repeat = r;
                self.publish();
            }
            Command::SetShuffle(s) => {
                self.state.write().unwrap().shuffle = s;
                self.publish();
            }
            Command::ClearQueue => {
                self.stop_playback();
                self.state.write().unwrap().queue.clear();
                let q = self.state.read().unwrap().queue.clone();
                let _ = self.events.send(WsEvent::queue(&q));
            }
            Command::RemoveQueueItem(i) => {
                let mut st = self.state.write().unwrap();
                if i < st.queue.len() {
                    st.queue.remove(i);
                    if let Some(qi) = st.queue_index {
                        if qi == i {
                            st.queue_index = None;
                        } else if qi > i {
                            st.queue_index = Some(qi - 1);
                        }
                    }
                }
                drop(st);
                let q = self.state.read().unwrap().queue.clone();
                let _ = self.events.send(WsEvent::queue(&q));
                self.publish();
            }
            Command::SelectDevice(name) => {
                match AudioOutput::new(name.as_deref()) {
                    Ok(mut out) => {
                        out.set_exclusive(self.exclusive.load(Ordering::SeqCst));
                        let pos = self.state.read().unwrap().position_secs;
                        let uri = self
                            .state
                            .read()
                            .unwrap()
                            .current
                            .as_ref()
                            .map(|t| t.uri.clone());
                        self.device_name = name.clone();
                        {
                            let mut st = self.state.write().unwrap();
                            st.audio_device = name.clone();
                            // Choisir un DAC = jouer dessus en local, meme si
                            // le mode « Jouer ici » etait actif.
                            st.output_mode = OutputMode::Local;
                        }
                        self.out_mode.store(0, Ordering::SeqCst);
                        if let Some(uri) = uri {
                            if self.state.read().unwrap().status != PlayStatus::Stopped {
                                self.output = Some(out);
                                self.start_track(uri, Some(pos));
                            } else {
                                self.output = Some(out);
                            }
                        } else {
                            self.output = Some(out);
                        }
                    }
                    Err(e) => self.publish_msg("error", &format!("peripherique audio: {e}")),
                }
                self.publish();
            }
            Command::SetOutputMode(m) => {
                self.out_mode.store(
                    if matches!(m, OutputMode::Browser) {
                        1
                    } else {
                        0
                    },
                    Ordering::SeqCst,
                );
                self.state.write().unwrap().output_mode = m;
                self.publish();
            }
            Command::SetExclusive(on) => {
                self.exclusive.store(on, Ordering::SeqCst);
                if let Some(out) = &mut self.output {
                    out.set_exclusive(on);
                }
                self.publish();
            }
        }
    }

    fn handle_worker_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Ready {
                spec,
                meta,
                duration,
                cons,
            } => {
                let uri = self.pending_uri.clone().unwrap_or_default();
                // Si la piste vient de la file (play_queue / playTrack), on
                // conserve son vrai Track (id de la bibliothèque) au lieu d'en
                // resynthétiser un à id aléatoire — sinon « Partager » sur la
                // piste en cours renvoie « piste introuvable ».
                let queued = {
                    let st = self.state.read().unwrap();
                    st.queue_index
                        .and_then(|i| st.queue.get(i))
                        .filter(|t| t.uri == uri)
                        .cloned()
                };
                let mut track = queued.unwrap_or_else(|| track_from_uri(&uri, &meta, duration));
                if track.duration_secs.is_none() {
                    track.duration_secs = duration.or(meta.duration_secs);
                }
                if meta.format.is_some() {
                    track.format = meta.format;
                }
                if self.output.is_none() {
                    match AudioOutput::new(self.device_name.as_deref()) {
                        Ok(mut o) => {
                            o.set_exclusive(self.exclusive.load(Ordering::SeqCst));
                            self.device_name = Some(o.name().to_string());
                            self.output = Some(o);
                        }
                        Err(e) => {
                            if self.out_mode.load(Ordering::SeqCst) == 1 {
                                // « Jouer ici » sans peripherique audio local
                                // (ex: binaire WSL, aucun DAC visible): on
                                // forward le PCM au navigateur via WebSocket.
                                self.output =
                                    Some(AudioOutput::browser_only("Navigateur (Jouer ici)"));
                            } else {
                                // DAC choisi indisponible au demarrage (debranche) :
                                // on retombe sur la sortie par defaut de l'OS avec un
                                // avertissement publie, plutot que de rester muet.
                                match AudioOutput::new(None) {
                                    Ok(mut o) => {
                                        o.set_exclusive(self.exclusive.load(Ordering::SeqCst));
                                        self.publish_msg(
                                            "warning",
                                            &format!(
                                                "sortie audio: {e} — bascule sur « {} » (defaut)",
                                                o.name()
                                            ),
                                        );
                                        self.device_name = Some(o.name().to_string());
                                        self.output = Some(o);
                                    }
                                    Err(e2) => {
                                        self.publish_msg(
                                            "error",
                                            &format!("sortie audio: {e} / defaut: {e2}"),
                                        );
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                // Pont worker -> mixer -> callback : le mixer gère le crossfade
                // vers la piste suivante dans le même anneau de sortie.
                let (mixer, cons_out) =
                    spawn_mixer(self.gen.clone(), RING_CHUNKS, self.frames_out.clone());
                let _ = mixer.cmd_tx.send(MixerCmd::SetSource {
                    cons,
                    channels: spec.channels as usize,
                    rate: spec.rate,
                });
                self.mixer = Some(mixer);
                let build_res = {
                    let out = self.output.as_mut().unwrap();
                    out.build_stream(
                        &spec,
                        cons_out,
                        self.volume.clone(),
                        self.muted.clone(),
                        self.frames_out.clone(),
                        self.gen.clone(),
                        self.max_volume.clone(),
                        self.curve.clone(),
                        self.vol_mode.clone(),
                        self.out_mode.clone(),
                        self.browser_tx.clone(),
                        self.eq.clone(),
                    )
                };
                if let Err(e) = build_res {
                    // Un flux qui ne s'ouvre pas ne doit pas laisser l'UI sur un
                    // « playing » fantôme à position 0 : on repasse en Stopped et
                    // on publie l'erreur. Pour un .dsf, on ajoute l'indice DoP si
                    // le mix format WASAPI est incompatible avec la porteuse.
                    let mut msg = format!("flux audio: {e}");
                    if spec.is_dsd_dop {
                        let mix = crate::wasapi_dop::device_mix_format(self.device_name.as_deref());
                        if let Some(hint) = crate::wasapi_dop::dop_mismatch_hint(mix, spec.rate) {
                            msg.push_str(&format!(" — {hint}"));
                        }
                    }
                    // Si un périphérique précis était sélectionné et refuse le flux
                    // (ex: « Ne pas autoriser » dans les réglages son Windows, ou DAC
                    // débranché), on relance la piste sur la sortie par défaut de
                    // l'OS avec un avertissement, plutôt que de rester muet.
                    let cur_name = self
                        .output
                        .as_ref()
                        .map(|o| o.name().to_string())
                        .unwrap_or_default();
                    let local_device = self.out_mode.load(Ordering::SeqCst) == 0
                        && self
                            .output
                            .as_ref()
                            .map(|o| o.has_device())
                            .unwrap_or(false);
                    if local_device && !cur_name.is_empty() {
                        if let Ok(mut o) = AudioOutput::new(None) {
                            o.set_exclusive(self.exclusive.load(Ordering::SeqCst));
                            let def = o.name().to_string();
                            if def != cur_name {
                                self.publish_msg(
                                    "warning",
                                    &format!(
                                        "{msg} — bascule sur « {def} » (sortie par défaut de l'OS)"
                                    ),
                                );
                                self.device_name = Some(def);
                                self.output = Some(o);
                                let seek = self.seek_after_ready;
                                self.start_track(uri, seek);
                                return;
                            }
                        }
                    }
                    self.publish_msg("error", &msg);
                    {
                        let mut st = self.state.write().unwrap();
                        st.status = PlayStatus::Stopped;
                        st.position_secs = 0.0;
                    }
                    self.publish();
                    return;
                }
                let out = self.output.as_mut().unwrap();
                self.cur_spec = Some(spec);
                self.cur_rate.store(spec.rate as u64, Ordering::SeqCst);
                *self.browser_spec.write().unwrap() = Some((spec.rate, u16::from(spec.channels)));
                let _ = self.browser_tx.send(BrowserFrame::Spec {
                    rate: spec.rate,
                    channels: u16::from(spec.channels),
                });
                {
                    let mut st = self.state.write().unwrap();
                    st.current = Some(track);
                    st.duration_secs = duration.unwrap_or(0.0);
                    // durée inconnue = flux live (webradio) : lecture infinie
                    st.is_live = duration.is_none() && meta.duration_secs.is_none();
                    st.stream_title = None;
                    st.position_secs = 0.0;
                    st.status = PlayStatus::Playing;
                    // flux sans tags (line-in #75, flux brut) : la spec du
                    // décodeur fait foi pour le format réellement joué
                    st.current_format = meta.format.or_else(|| Some(spec_format(spec)));
                    // Sans peripherique local (« Jouer ici » seul), on garde le
                    // nom de device choisi plutot que « Navigateur ».
                    st.audio_device = if out.has_device() {
                        Some(out.name().to_string())
                    } else {
                        self.device_name.clone()
                    };
                    st.output_backend = out.backend().to_string();
                }
                if let Some(s) = self.seek_after_ready.take() {
                    if let Some(w) = &self.worker {
                        let _ = w.cmd_tx.send(WorkerCmd::Seek(s));
                    }
                    let rate = spec.rate.max(1) as f64;
                    self.frames_out.store((s * rate) as u64, Ordering::SeqCst);
                    self.state.write().unwrap().position_secs = s;
                }
                self.publish();
            }
            WorkerMsg::Seeked(secs) => {
                let rate = self.cur_rate.load(Ordering::SeqCst).max(1);
                self.frames_out
                    .store((secs * rate as f64) as u64, Ordering::SeqCst);
                self.state.write().unwrap().position_secs = secs;
                self.publish();
            }
            WorkerMsg::EndOfTrack => {
                tracing::debug!(
                    "engine: EndOfTrack recu, frames_out={} pos={:.2} — attente drain anneau de sortie",
                    self.frames_out.load(Ordering::SeqCst),
                    self.state.read().unwrap().position_secs
                );
                // Le worker signale EOF quand SON anneau est vide, mais le
                // mixer a pu bufferiser la suite dans l'anneau de sortie :
                // on attend que celui-ci soit réellement consommé (drained)
                // avant d'enchaîner, sinon on couperait la fin de la piste.
                self.pending_eot = true;
            }
            WorkerMsg::Error(e) => {
                tracing::error!("worker: {e}");
                self.publish_msg("error", &e);
                if let Some(i) = self.next_index() {
                    self.play_queue_index(i);
                } else {
                    self.stop_playback();
                }
            }
        }
    }

    /// Précharge la piste suivante quand la position source atteint
    /// `durée - crossfade - marge` (crossfade) ou `durée - 1 s` (gapless seul).
    fn maybe_preload(&mut self) {
        if self.pending_next.is_some() {
            return;
        }
        let gapless = self.gapless.load(Ordering::SeqCst);
        let xf_ms = self.crossfade_ms.load(Ordering::SeqCst);
        if !gapless && xf_ms == 0 {
            return;
        }
        if self.state.read().unwrap().status != PlayStatus::Playing {
            return;
        }
        let Some(mixer) = &self.mixer else { return };
        let rate = self.cur_rate.load(Ordering::SeqCst).max(1);
        let dur = self.state.read().unwrap().duration_secs;
        if dur <= 0.0 {
            return; // flux live : pas de préchargement
        }
        let total = dur * rate as f64;
        let xf_frames = (xf_ms as f64 / 1000.0) * rate as f64;
        // marge d'ouverture/décodage de la piste suivante
        let threshold = total - xf_frames - rate as f64;
        if (mixer.frames_pushed.load(Ordering::SeqCst) as f64) < threshold {
            return;
        }
        let Some(idx) = self.next_index() else { return };
        let uri = self
            .state
            .read()
            .unwrap()
            .queue
            .get(idx)
            .map(|t| t.uri.clone());
        let Some(uri) = uri else { return };
        // même format uniquement : le crossfade ne resample pas
        if let Some(cur) = self.cur_spec {
            if cur.is_dsd_dop {
                return;
            }
        }
        tracing::debug!("engine: préchargement piste suivante #{idx} ({uri})");
        let (msg_rx, cmd_tx, join) =
            spawn_worker(self.sources.clone(), uri.clone(), self.gen.clone());
        let gain = self
            .loudness_lookup
            .read()
            .unwrap()
            .as_ref()
            .and_then(|f| f(&uri))
            .unwrap_or(1.0);
        let _ = cmd_tx.send(WorkerCmd::SetGain(gain));
        self.pending_next = Some(PendingNext {
            worker: WorkerHandle {
                msg_rx,
                cmd_tx,
                join: Some(join),
            },
            index: idx,
            uri,
        });
    }

    /// Messages du worker de la piste préchargée (B).
    fn handle_pending_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Ready {
                spec,
                meta,
                duration,
                cons,
            } => {
                let Some(p) = self.pending_next.take() else {
                    return;
                };
                let cur = self.cur_spec;
                let xf_ms = self.crossfade_ms.load(Ordering::SeqCst);
                let compat = cur
                    .map(|c| {
                        c.rate == spec.rate
                            && c.channels == spec.channels
                            && !c.is_dsd_dop
                            && !spec.is_dsd_dop
                    })
                    .unwrap_or(false)
                    && self.state.read().unwrap().status == PlayStatus::Playing;
                if !compat || (xf_ms == 0 && !self.gapless.load(Ordering::SeqCst)) {
                    // pas de fondu possible : B est abandonné, l'enchaînement
                    // se fera à EndOfTrack de A comme avant
                    p.worker.stop();
                    return;
                }
                // démarre le fondu. PAS de bump de génération ici : le contenu
                // déjà bufferisé dans l'anneau de sortie (fin de A) reste valide.
                // Le mixer distingue A et B via gen_a/gen_b ; c'est le bump au
                // seek (ou au prochain start_track) qui purgera les vieux chunks.
                let gen_cur = self.gen.load(Ordering::SeqCst);
                let gen_a = gen_cur;
                let gen_b = gen_cur;
                let rate = spec.rate.max(1);
                let xf_frames = ((xf_ms as f64 / 1000.0) * rate as f64) as usize;
                let dur_a = self.state.read().unwrap().duration_secs;
                let start_at = ((dur_a * rate as f64).max(0.0) - xf_frames as f64).max(0.0) as u64;
                tracing::debug!(
                    "engine: crossfade {xf_ms} ms vers #{} (start_at={start_at}, xf_frames={xf_frames})",
                    p.index
                );
                if let Some(m) = &self.mixer {
                    let _ = m.cmd_tx.send(MixerCmd::StartCrossfade {
                        cons_b: cons,
                        xf_frames,
                        channels: spec.channels as usize,
                        gen_a,
                        gen_b,
                        start_at,
                    });
                }
                // promotion : B devient la piste courante du worker principal,
                // A est stoppé (le mixer draine ce qui reste de A pour le fondu)
                if let Some(w) = self.worker.take() {
                    w.stop();
                }
                self.worker = Some(p.worker);
                let uri = p.uri.clone();
                let idx = p.index;
                let queued = self.state.read().unwrap().queue.get(idx).cloned();
                let mut track = queued.unwrap_or_else(|| track_from_uri(&uri, &meta, duration));
                if track.duration_secs.is_none() {
                    track.duration_secs = duration.or(meta.duration_secs);
                }
                if meta.format.is_some() {
                    track.format = meta.format;
                }
                self.cur_spec = Some(spec);
                self.cur_rate.store(spec.rate as u64, Ordering::SeqCst);
                // l'UI repart à 0 sur la nouvelle piste (comme Spotify au
                // démarrage du fondu) ; l'audio, lui, reste continu
                self.frames_out.store(0, Ordering::SeqCst);
                {
                    let mut st = self.state.write().unwrap();
                    st.queue_index = Some(idx);
                    st.current = Some(track);
                    st.duration_secs = duration.unwrap_or(0.0);
                    st.is_live = duration.is_none() && meta.duration_secs.is_none();
                    st.stream_title = None;
                    st.position_secs = 0.0;
                    st.status = PlayStatus::Playing;
                    // flux sans tags (line-in #75, flux brut) : la spec du
                    // décodeur fait foi pour le format réellement joué
                    st.current_format = meta.format.or_else(|| Some(spec_format(spec)));
                }
                self.publish();
            }
            WorkerMsg::Seeked(_) => {}
            WorkerMsg::EndOfTrack | WorkerMsg::Error(_) => {
                if let Some(p) = self.pending_next.take() {
                    p.worker.stop();
                }
            }
        }
    }

    fn run(mut self) {
        loop {
            match self.cmd_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(cmd) => self.handle_command(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            let mut msgs: Vec<(bool, WorkerMsg)> = Vec::new();
            if let Some(w) = &self.worker {
                while let Ok(m) = w.msg_rx.try_recv() {
                    msgs.push((false, m));
                }
            }
            if let Some(p) = &self.pending_next {
                while let Ok(m) = p.worker.msg_rx.try_recv() {
                    msgs.push((true, m));
                }
            }
            for (pending, m) in msgs {
                if pending {
                    self.handle_pending_msg(m);
                } else {
                    self.handle_worker_msg(m);
                }
            }
            // EndOfTrack en attente : on enchaîne seulement quand l'anneau de
            // sortie est réellement drainé (fin de piste jouée jusqu'au bout).
            if self.pending_eot {
                let drained = self
                    .mixer
                    .as_ref()
                    .map(|m| m.drained.load(Ordering::SeqCst))
                    .unwrap_or(true);
                if drained {
                    self.pending_eot = false;
                    self.advance_after_eot();
                }
            }
            self.maybe_preload();
        }
    }
}

/// Handle public vers le moteur de lecture.
pub struct PlayerEngine {
    cmd_tx: mpsc::Sender<Command>,
    state: Arc<RwLock<PlayerState>>,
    loudness_lookup: Arc<RwLock<Option<LoudnessLookup>>>,
    /// Canal d'evenements temps reel (WebSocket).
    pub events: broadcast::Sender<WsEvent>,
    /// Canal du flux « Jouer ici » (PCM f32 vers /ws/audio).
    pub browser_tx: broadcast::Sender<BrowserFrame>,
    browser_spec: Arc<RwLock<Option<(u32, u16)>>>,
    exclusive: Arc<AtomicBool>,
}

impl PlayerEngine {
    /// Demarre le moteur (thread dedie + horloge de position).
    pub fn start(
        sources: Arc<SourcesManager>,
        device: Option<String>,
        initial_volume: u8,
        exclusive: bool,
    ) -> Arc<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (events, _) = broadcast::channel::<WsEvent>(256);
        let state = Arc::new(RwLock::new(PlayerState::default()));
        state.write().unwrap().volume = initial_volume.min(100);
        state.write().unwrap().audio_device = device.clone();
        let volume = Arc::new(AtomicU8::new(initial_volume.min(100)));
        let max_volume = Arc::new(AtomicU8::new(100));
        let curve = Arc::new(AtomicU8::new(0)); // logarithmique par défaut
        let vol_mode = Arc::new(AtomicU8::new(0)); // logiciel par défaut
        let muted = Arc::new(AtomicBool::new(false));
        let frames_out = Arc::new(AtomicU64::new(0));
        let gen = Arc::new(AtomicU64::new(0));
        let cur_rate = Arc::new(AtomicU64::new(44100));
        let (browser_tx, _) = broadcast::channel::<BrowserFrame>(128);
        let out_mode = Arc::new(AtomicU8::new(0));
        let exclusive = Arc::new(AtomicBool::new(exclusive));
        let browser_spec = Arc::new(RwLock::new(None::<(u32, u16)>));
        let eq = Arc::new(RwLock::new(EqConfig::default()));
        let crossfade_ms = Arc::new(AtomicU64::new(0));
        let gapless = Arc::new(AtomicBool::new(true));
        let loudness_lookup: Arc<RwLock<Option<LoudnessLookup>>> = Arc::new(RwLock::new(None));

        // horloge de position: 2x / seconde
        {
            let state = state.clone();
            let frames_out = frames_out.clone();
            let cur_rate = cur_rate.clone();
            let events = events.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(500));
                let rate = cur_rate.load(Ordering::SeqCst).max(1) as f64;
                let pos = frames_out.load(Ordering::SeqCst) as f64 / rate;
                let mut publish = false;
                {
                    let mut st = state.write().unwrap();
                    if st.status == PlayStatus::Playing {
                        st.position_secs = pos;
                        publish = true;
                    }
                }
                if publish {
                    let st = state.read().unwrap().clone();
                    let _ = events.send(WsEvent::state(&st));
                }
            });
        }

        let engine = Engine {
            sources,
            cmd_rx,
            state: state.clone(),
            events: events.clone(),
            volume,
            max_volume,
            curve,
            vol_mode,
            muted,
            frames_out,
            gen,
            cur_rate,
            exclusive: exclusive.clone(),
            output: None,
            device_name: device,
            out_mode: out_mode.clone(),
            browser_tx: browser_tx.clone(),
            browser_spec: browser_spec.clone(),
            eq,
            worker: None,
            mixer: None,
            pending_next: None,
            pending_eot: false,
            cur_spec: None,
            pending_uri: None,
            seek_after_ready: None,
            crossfade_ms,
            gapless,
            loudness_lookup: loudness_lookup.clone(),
            rng: XorShift(
                0x9E3779B97F4A7C15
                    ^ std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(1),
            ),
        };
        std::thread::spawn(move || engine.run());

        Arc::new(PlayerEngine {
            cmd_tx,
            state,
            events,
            browser_tx,
            browser_spec,
            exclusive,
            loudness_lookup,
        })
    }

    /// Installe le lookup de gain loudness (uri -> gain linéaire) ; appelé par
    /// la couche web qui connaît la bibliothèque et les réglages (#37).
    pub fn set_loudness_lookup(&self, f: Option<LoudnessLookup>) {
        *self.loudness_lookup.write().unwrap() = f;
    }

    /// Gain loudness (linéaire) du flux d'URI donné, si mesuré et activé (#77).
    pub fn loudness_gain(&self, uri: &str) -> Option<f32> {
        let lk = self.loudness_lookup.read().unwrap();
        lk.as_ref().and_then(|f| f(uri))
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn state(&self) -> PlayerState {
        let mut st = self.state.read().unwrap().clone();
        // Sur un flux live, on injecte le titre ICY courant (StreamTitle) lu
        // dans le registre des radios, mis à jour en arrière-plan par le flux.
        if st.is_live {
            if let Some(c) = &st.current {
                st.stream_title = auris_sources::radio_title(&c.uri);
            }
        }
        st
    }
    pub fn play_uri(&self, uri: impl Into<String>) {
        self.send(Command::PlayUri(uri.into()));
    }
    pub fn play_queue(&self, tracks: Vec<Track>, index: usize) {
        self.send(Command::PlayQueue(tracks, index));
    }
    pub fn enqueue(&self, t: Track) {
        self.send(Command::Enqueue(Box::new(t)));
    }
    /// « Lire ensuite » : insere les pistes juste apres la piste en cours.
    pub fn enqueue_next(&self, tracks: Vec<Track>) {
        self.send(Command::EnqueueNext(tracks));
    }
    pub fn pause(&self) {
        self.send(Command::Pause);
    }
    pub fn resume(&self) {
        self.send(Command::Resume);
    }
    pub fn stop(&self) {
        self.send(Command::Stop);
    }
    pub fn next(&self) {
        self.send(Command::Next);
    }
    pub fn prev(&self) {
        self.send(Command::Prev);
    }
    pub fn seek(&self, secs: f64) {
        self.send(Command::Seek(secs));
    }
    /// Applique les réglages playback (volume max, courbe, mode bit-perfect) à chaud.
    pub fn apply_playback(&self, p: &PlaybackSettings) {
        let _ = self
            .cmd_tx
            .send(Command::ApplyPlayback(Box::new(p.clone())));
    }

    pub fn set_volume(&self, v: u8) {
        // maj synchrone de l'etat: le POST /api/player/volume renvoie l'etat a jour
        self.state.write().unwrap().volume = v.min(100);
        self.send(Command::SetVolume(v));
    }
    pub fn set_muted(&self, m: bool) {
        self.state.write().unwrap().muted = m;
        self.send(Command::SetMuted(m));
    }
    pub fn set_repeat(&self, r: RepeatMode) {
        self.state.write().unwrap().repeat = r;
        self.send(Command::SetRepeat(r));
    }
    pub fn set_shuffle(&self, s: bool) {
        self.state.write().unwrap().shuffle = s;
        self.send(Command::SetShuffle(s));
    }
    pub fn clear_queue(&self) {
        self.send(Command::ClearQueue);
    }
    pub fn remove_queue_item(&self, i: usize) {
        self.send(Command::RemoveQueueItem(i));
    }
    /// Joue la piste à l'index `idx` de la file d'attente.
    pub fn play_queue_index(&self, idx: usize) {
        self.send(Command::PlayQueueIndex(idx));
    }
    pub fn select_device(&self, name: Option<String>) {
        self.send(Command::SelectDevice(name));
    }
    /// Active/désactive le mode WASAPI exclusif (Windows, bit-perfect).
    pub fn set_exclusive(&self, on: bool) {
        self.send(Command::SetExclusive(on));
    }
    pub fn exclusive_enabled(&self) -> bool {
        self.exclusive.load(Ordering::SeqCst)
    }
    /// Bascule la sortie active : DAC local ou navigateur (« Jouer ici »).
    pub fn set_output_mode(&self, m: OutputMode) {
        self.state.write().unwrap().output_mode = m;
        self.send(Command::SetOutputMode(m));
    }
    /// Spec du flux navigateur courant (rate, channels), si une piste est lue.
    pub fn browser_spec(&self) -> Option<(u32, u16)> {
        *self.browser_spec.read().unwrap()
    }
}
