//! Fondu enchaîné (crossfade) entre deux pistes — thread « mixer » placé entre
//! le worker de décodage et l'anneau de sortie consommé par le callback audio.
//!
//! - Mode normal : forward les chunks de la source courante tels quels
//!   (les chunks d'une génération périmée — seek, changement de piste — sont
//!   jetés).
//! - Mode crossfade : quand le moteur précharge la piste suivante, il envoie
//!   [`MixerCmd::StartCrossfade`] ; le mixer superpose alors la fin de la piste
//!   A et le début de la piste B avec une courbe *equal-power*
//!   (`cos`/`sin`), frame par frame, et pousse le mix dans l'anneau de sortie
//!   avec la génération courante (le callback audio ne voit aucune couture).
//! - [`MixerHandle::frames_pushed`] compte les trames de la source courante
//!   poussées dans l'anneau de sortie : c'est la « position source » utilisée
//!   par le moteur pour déclencher le préchargement à `durée - crossfade`.

use std::f64::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use ringbuf::traits::{Consumer, Observer, Producer};

use crate::decoder::{Chunk, ChunkData};
use crate::output::{make_ring, Cons, Prod};

/// Commandes envoyées au thread mixer.
pub enum MixerCmd {
    /// Nouvelle source principale (piste A), sans fondu. `channels` sert au
    /// comptage de trames, `rate` au cadencement temps réel (marge de 2 s).
    /// Le compteur est remis à zéro.
    SetSource {
        cons: Cons,
        channels: usize,
        rate: u32,
    },
    /// Démarre un crossfade vers la source B. `xf_frames` = durée du fondu en
    /// trames, `start_at` = position absolue (trames poussées) où le fondu
    /// commence — le mixer forward A tel quel jusque-là. `gen_a`/`gen_b` =
    /// générations des chunks de A et B. `xf_frames == 0` = simple bascule.
    StartCrossfade {
        cons_b: Cons,
        xf_frames: usize,
        channels: usize,
        gen_a: u64,
        gen_b: u64,
        start_at: u64,
    },
    /// Annule un crossfade en cours : on repasse sur la source B seule.
    CancelCrossfade,
    /// Recale le compteur de trames poussées (après un seek, en trames source).
    SetCount(u64),
    Stop,
}

struct Xf {
    cons_b: Cons,
    xf_frames: usize,
    channels: usize,
    done: usize,
    gen_a: u64,
    gen_b: u64,
    /// Position (trames poussées) à partir de laquelle le fondu démarre.
    start_at: u64,
}

/// Handle du thread mixer.
pub struct MixerHandle {
    pub cmd_tx: mpsc::Sender<MixerCmd>,
    /// Trames de la source courante poussées dans l'anneau de sortie.
    pub frames_pushed: Arc<AtomicU64>,
    /// Vrai quand tout a été poussé ET consommé par le callback (fin réelle
    /// de la lecture) : source vide, pas de fondu en cours, anneau de sortie
    /// vide. Le moteur s'en sert pour différer `EndOfTrack`.
    pub drained: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl MixerHandle {
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(MixerCmd::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Lance le thread mixer. Retourne le handle et le consommateur de l'anneau
/// de sortie (à remettre au callback audio via `WorkerMsg::Ready`).
/// `frames_out` = trames réellement consommées par le callback : le mixer
/// se cadence dessus (marge ~2 s) pour ne pas bufferiser toute la piste en
/// avance — sinon la queue de la piste A serait déjà partie dans l'anneau de
/// sortie au moment du crossfade et le fondu serait inaudible.
pub fn spawn_mixer(
    gen: Arc<AtomicU64>,
    ring_chunks: usize,
    frames_out: Arc<AtomicU64>,
) -> (MixerHandle, Cons) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (prod, cons) = make_ring(ring_chunks);
    let frames_pushed = Arc::new(AtomicU64::new(0));
    let fp = frames_pushed.clone();
    let g = gen.clone();
    let fo = frames_out;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let sf = stop_flag.clone();
    let drained = Arc::new(AtomicBool::new(true));
    let dr = drained.clone();
    let join = std::thread::Builder::new()
        .name("auris-mixer".into())
        .spawn(move || run(cmd_rx, prod, g, fp, fo, sf, dr))
        .expect("spawn mixer");
    (
        MixerHandle {
            cmd_tx,
            frames_pushed,
            drained,
            stop_flag,
            join: Some(join),
        },
        cons,
    )
}

/// Pousse un chunk dans l'anneau de sortie (avec backpressure). Retourne
/// false si un Stop arrive pendant l'attente. Ne consomme JAMAIS le canal de
/// commandes (sinon une commande serait perdue pendant l'attente).
fn push_chunk(prod: &mut Prod, stop: &AtomicBool, mut chunk: Chunk) -> bool {
    loop {
        match prod.try_push(chunk) {
            Ok(()) => return true,
            Err(c) => {
                chunk = c;
                if stop.load(Ordering::SeqCst) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn run(
    cmd_rx: mpsc::Receiver<MixerCmd>,
    mut prod: Prod,
    gen: Arc<AtomicU64>,
    frames: Arc<AtomicU64>,
    frames_out: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
) {
    let mut src: Option<Cons> = None;
    let mut src_channels = 1usize;
    let mut pace_margin: u64 = 2 * 48000;
    let mut xf: Option<Xf> = None;
    // stash F32 partiellement consommé: (échantillons interleaved, offset)
    let mut stash_a: Option<(Vec<f32>, usize)> = None;
    let mut stash_b: Option<(Vec<f32>, usize)> = None;

    'outer: loop {
        // draine les commandes en attente
        loop {
            match cmd_rx.try_recv() {
                Ok(MixerCmd::SetSource {
                    cons,
                    channels,
                    rate,
                }) => {
                    src = Some(cons);
                    src_channels = channels.max(1);
                    pace_margin = 2 * rate.max(1) as u64;
                    xf = None;
                    stash_a = None;
                    stash_b = None;
                    frames.store(0, Ordering::SeqCst);
                }
                Ok(MixerCmd::StartCrossfade {
                    cons_b,
                    xf_frames,
                    channels,
                    gen_a,
                    gen_b,
                    start_at,
                }) => {
                    if xf_frames == 0 || src.is_none() {
                        // gapless sans fondu : B devient la source directement
                        src = Some(cons_b);
                        src_channels = channels.max(1);
                        xf = None;
                        stash_a = None;
                        stash_b = None;
                        frames.store(0, Ordering::SeqCst);
                    } else {
                        xf = Some(Xf {
                            cons_b,
                            xf_frames,
                            channels: channels.max(1),
                            done: 0,
                            gen_a,
                            gen_b,
                            start_at,
                        });
                        stash_b = None;
                        tracing::debug!(
                            "mixer: StartCrossfade recu (frames={}, start_at={start_at}, xf_frames={xf_frames}, gen={})",
                            frames.load(Ordering::SeqCst),
                            gen.load(Ordering::SeqCst)
                        );
                    }
                }
                Ok(MixerCmd::CancelCrossfade) => {
                    if let Some(x) = xf.take() {
                        src = Some(x.cons_b);
                        src_channels = x.channels;
                        stash_a = None;
                        stash_b = None;
                    }
                }
                Ok(MixerCmd::SetCount(n)) => frames.store(n, Ordering::SeqCst),
                Ok(MixerCmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => break 'outer,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        // Fin réelle de lecture : plus rien à pousser (source vide, pas de
        // fondu, pas de stash) et l'anneau de sortie est vide (tout consommé
        // par le callback). Posé à false à chaque push ci-dessous.
        let is_drained = xf.is_none()
            && stash_a.is_none()
            && stash_b.is_none()
            && prod.is_empty()
            && src.as_ref().map(|c| c.is_empty()).unwrap_or(true);
        drained.store(is_drained, Ordering::SeqCst);

        if let Some(x) = xf.as_mut() {
            // --- mode crossfade ---
            // Avant start_at, on forward A tel quel (B est juste pré-décodé).
            if frames.load(Ordering::SeqCst) < x.start_at {
                if let Some(cons) = src.as_mut() {
                    match cons.try_pop() {
                        Some(chunk)
                            if chunk.gen == x.gen_a || chunk.gen == gen.load(Ordering::SeqCst) =>
                        {
                            let fr = chunk.data.frames(x.channels) as u64;
                            frames.fetch_add(fr, Ordering::SeqCst);
                            // re-taggé avec la génération courante (bump au fondu)
                            let gnow = gen.load(Ordering::SeqCst);
                            let chunk = Chunk {
                                gen: gnow,
                                data: chunk.data,
                            };
                            if !push_chunk(&mut prod, &stop, chunk) {
                                break 'outer;
                            }
                        }
                        Some(_) => {} // génération périmée : jeté
                        None => std::thread::sleep(Duration::from_millis(1)),
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
                continue;
            }
            // Il faut un chunk de B pour avancer (A reste bufferisé sinon).
            if stash_b.is_none() {
                match x.cons_b.try_pop() {
                    Some(Chunk {
                        gen: cg,
                        data: ChunkData::F32(v),
                    }) if cg == x.gen_b || cg == gen.load(Ordering::SeqCst) => {
                        stash_b = Some((v, 0));
                    }
                    Some(Chunk {
                        data: ChunkData::I24Dop(_),
                        ..
                    }) => {
                        // inattendu (le moteur ne lance un fondu que sur du PCM) :
                        // on abandonne le fondu et B devient la source
                        let x = xf.take().unwrap();
                        src = Some(x.cons_b);
                        src_channels = x.channels;
                        stash_a = None;
                        continue;
                    }
                    Some(_) => {} // génération périmée : jeté
                    None => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                }
            }
            // A restant (optionnel)
            if stash_a.is_none() {
                if let Some(cons) = src.as_mut() {
                    match cons.try_pop() {
                        Some(Chunk {
                            gen: cg,
                            data: ChunkData::F32(v),
                        }) if cg == x.gen_a => stash_a = Some((v, 0)),
                        Some(_) => {} // jeté
                        None => {}
                    }
                }
            }
            // mixe le chunk B complet avec la courbe equal-power
            let channels = x.channels;
            let (bv, boff) = stash_b.as_mut().unwrap();
            let n = bv.len();
            let mut out = Vec::with_capacity(n);
            let mut i = *boff;
            while i < n {
                let t = (x.done as f64 / x.xf_frames as f64).min(1.0);
                let wa = (t * FRAC_PI_2).cos() as f32;
                let wb = (t * FRAC_PI_2).sin() as f32;
                let b = bv[i];
                let a = match stash_a.as_mut() {
                    Some((av, ao)) if *ao < av.len() => {
                        let v = av[*ao];
                        *ao += 1;
                        Some(v)
                    }
                    _ => None,
                };
                // si A est épuisé en cours de fondu, B passe à pleine puissance
                out.push(match a {
                    Some(av) => av * wa + b * wb,
                    None => b,
                });
                i += 1;
                if i % channels == 0 {
                    x.done += 1;
                }
            }
            *boff = n;
            stash_b = None;
            // stash_a entièrement consommé -> on le remet à None pour que le
            // prochain tour en re-poppe un depuis la source A (sinon A ne
            // contribuerait que sur son premier chunk).
            if matches!(&stash_a, Some((av, ao)) if *ao >= av.len()) {
                stash_a = None;
            }
            let pushed_frames = (n / channels) as u64;
            frames.fetch_add(pushed_frames, Ordering::SeqCst);
            let gnow = gen.load(Ordering::SeqCst);
            if !push_chunk(
                &mut prod,
                &stop,
                Chunk {
                    gen: gnow,
                    data: ChunkData::F32(out),
                },
            ) {
                break 'outer;
            }
            if x.done >= x.xf_frames {
                // fondu terminé : B devient la source principale, le compteur
                // repart à la position courante dans B (les xf_frames mixées)
                let x = xf.take().unwrap();
                src = Some(x.cons_b);
                src_channels = x.channels;
                stash_a = None;
                frames.store(x.xf_frames as u64, Ordering::SeqCst);
                tracing::debug!(
                    "mixer: fondu termine, B devient la source (frames={})",
                    x.xf_frames
                );
            }
        } else if let Some(cons) = src.as_mut() {
            // --- mode normal : forward ---
            // Cadencement temps réel : on ne pousse pas plus de `pace_margin`
            // trames au-delà de ce que le callback a déjà consommé, pour
            // garder la fin de piste disponible dans l'anneau source (le
            // crossfade la mixe avec le début de la piste suivante).
            let ahead = frames
                .load(Ordering::SeqCst)
                .saturating_sub(frames_out.load(Ordering::SeqCst));
            if ahead >= pace_margin {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            match cons.try_pop() {
                Some(chunk) => {
                    let g = gen.load(Ordering::SeqCst);
                    if chunk.gen == g {
                        let fr = chunk.data.frames(src_channels) as u64;
                        frames.fetch_add(fr, Ordering::SeqCst);
                        if !push_chunk(&mut prod, &stop, chunk) {
                            break 'outer;
                        }
                    }
                    // sinon : génération périmée (seek / changement de piste) -> jeté
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::make_ring;
    use ringbuf::traits::Consumer;

    fn f32_chunk(gen: u64, v: Vec<f32>) -> Chunk {
        Chunk {
            gen,
            data: ChunkData::F32(v),
        }
    }

    fn pop_f32(cons: &mut Cons) -> (u64, Vec<f32>) {
        for _ in 0..200 {
            if let Some(Chunk {
                gen,
                data: ChunkData::F32(v),
            }) = cons.try_pop()
            {
                return (gen, v);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("aucun chunk reçu sur l'anneau de sortie");
    }

    #[test]
    fn crossfade_mix_equal_power_puis_b_seul() {
        let gen = Arc::new(AtomicU64::new(1));
        let (handle, mut cons_out) = spawn_mixer(gen.clone(), 16, Arc::new(AtomicU64::new(0)));
        let (mut prod_a, cons_a) = make_ring(16);
        let (mut prod_b, cons_b) = make_ring(16);

        handle
            .cmd_tx
            .send(MixerCmd::SetSource {
                cons: cons_a,
                channels: 1,
                rate: 44100,
            })
            .unwrap();
        // le moteur bump la génération au démarrage du fondu
        gen.store(2, Ordering::SeqCst);
        handle
            .cmd_tx
            .send(MixerCmd::StartCrossfade {
                cons_b,
                xf_frames: 4,
                channels: 1,
                gen_a: 1,
                gen_b: 2,
                start_at: 0,
            })
            .unwrap();
        prod_a.try_push(f32_chunk(1, vec![0.5; 8])).unwrap();
        prod_b.try_push(f32_chunk(2, vec![-0.5; 8])).unwrap();

        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 2);
        assert_eq!(v.len(), 8);
        let wa = |t: f64| (t * FRAC_PI_2).cos() as f32;
        let wb = |t: f64| (t * FRAC_PI_2).sin() as f32;
        for (i, s) in v.iter().enumerate() {
            let t = (i as f64 / 4.0).min(1.0);
            let expected = 0.5 * wa(t) + (-0.5) * wb(t);
            assert!(
                (*s - expected).abs() < 1e-6,
                "frame {i}: attendu {expected}, reçu {s}",
            );
        }
        // le fondu est terminé (done=8 >= xf_frames=4) : B seul, compteur = xf_frames
        assert_eq!(handle.frames_pushed.load(Ordering::SeqCst), 4);
        handle.stop();
    }

    #[test]
    fn normal_forward_et_purge_generation_perimee() {
        let gen = Arc::new(AtomicU64::new(1));
        let (handle, mut cons_out) = spawn_mixer(gen.clone(), 16, Arc::new(AtomicU64::new(0)));
        let (mut prod_a, cons_a) = make_ring(16);
        handle
            .cmd_tx
            .send(MixerCmd::SetSource {
                cons: cons_a,
                channels: 2,
                rate: 44100,
            })
            .unwrap();

        prod_a.try_push(f32_chunk(1, vec![0.1; 4])).unwrap(); // 2 frames stéréo, gen courante
        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 1);
        assert_eq!(v, vec![0.1; 4]);
        assert_eq!(handle.frames_pushed.load(Ordering::SeqCst), 2);

        // seek : le moteur bump la génération -> les vieux chunks sont jetés
        gen.store(2, Ordering::SeqCst);
        prod_a.try_push(f32_chunk(1, vec![9.0; 4])).unwrap(); // périmé
        prod_a.try_push(f32_chunk(2, vec![0.2; 4])).unwrap(); // nouveau
        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 2);
        assert_eq!(v, vec![0.2; 4]);
        assert_eq!(handle.frames_pushed.load(Ordering::SeqCst), 4);
        handle.stop();
    }

    #[test]
    fn set_count_recale_le_compteur() {
        let gen = Arc::new(AtomicU64::new(1));
        let (handle, _cons_out) = spawn_mixer(gen, 8, Arc::new(AtomicU64::new(0)));
        handle.cmd_tx.send(MixerCmd::SetCount(44100 * 30)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(handle.frames_pushed.load(Ordering::SeqCst), 44100 * 30);
        handle.stop();
    }

    #[test]
    fn cancel_crossfade_repasse_sur_b_sans_mixer() {
        let gen = Arc::new(AtomicU64::new(1));
        let (handle, mut cons_out) = spawn_mixer(gen.clone(), 16, Arc::new(AtomicU64::new(0)));
        let (_prod_a, cons_a) = make_ring(16);
        let (mut prod_b, cons_b) = make_ring(16);
        handle
            .cmd_tx
            .send(MixerCmd::SetSource {
                cons: cons_a,
                channels: 1,
                rate: 44100,
            })
            .unwrap();
        gen.store(2, Ordering::SeqCst);
        handle
            .cmd_tx
            .send(MixerCmd::StartCrossfade {
                cons_b,
                xf_frames: 100,
                channels: 1,
                gen_a: 1,
                gen_b: 2,
                start_at: 0,
            })
            .unwrap();
        handle.cmd_tx.send(MixerCmd::CancelCrossfade).unwrap();
        prod_b.try_push(f32_chunk(2, vec![0.7; 4])).unwrap();
        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 2);
        assert_eq!(v, vec![0.7; 4]); // B tel quel, pas de fondu
        handle.stop();
    }

    #[test]
    fn crossfade_attend_start_at_avant_de_mixer() {
        let gen = Arc::new(AtomicU64::new(1));
        let (handle, mut cons_out) = spawn_mixer(gen.clone(), 16, Arc::new(AtomicU64::new(0)));
        let (mut prod_a, cons_a) = make_ring(16);
        let (mut prod_b, cons_b) = make_ring(16);
        handle
            .cmd_tx
            .send(MixerCmd::SetSource {
                cons: cons_a,
                channels: 1,
                rate: 44100,
            })
            .unwrap();
        gen.store(2, Ordering::SeqCst);
        // fondu de 4 trames qui commence à la trame 8 : A d'abord forwardé pur
        handle
            .cmd_tx
            .send(MixerCmd::StartCrossfade {
                cons_b,
                xf_frames: 4,
                channels: 1,
                gen_a: 1,
                gen_b: 2,
                start_at: 8,
            })
            .unwrap();
        prod_a.try_push(f32_chunk(1, vec![0.5; 8])).unwrap();
        prod_b.try_push(f32_chunk(2, vec![-0.5; 8])).unwrap();

        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 2); // re-taggé génération courante
        assert_eq!(v, vec![0.5; 8]); // A tel quel avant start_at
        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 2);
        assert_eq!(v.len(), 8); // puis le mix
        handle.stop();
    }

    #[test]
    fn start_crossfade_sans_source_branche_b_direct() {
        let gen = Arc::new(AtomicU64::new(5));
        let (handle, mut cons_out) = spawn_mixer(gen.clone(), 16, Arc::new(AtomicU64::new(0)));
        let (mut prod_b, cons_b) = make_ring(16);
        handle
            .cmd_tx
            .send(MixerCmd::StartCrossfade {
                cons_b,
                xf_frames: 10,
                channels: 1,
                gen_a: 4,
                gen_b: 5,
                start_at: 0,
            })
            .unwrap();
        prod_b.try_push(f32_chunk(5, vec![0.3; 2])).unwrap();
        let (g, v) = pop_f32(&mut cons_out);
        assert_eq!(g, 5);
        assert_eq!(v, vec![0.3; 2]);
        handle.stop();
    }
}
