//! Mesure de loudness intégrée ITU-R BS.1770 / EBU R128 (K-weighting) et
//! normalisation statique par gain (style ReplayGain) — DSP #37.
//!
//! Le filtre K est la cascade d'un high-shelf (f0 ≈ 1681,97 Hz, G ≈ 4 dB,
//! Q ≈ 0,7072) et d'un high-pass (f0 ≈ 38,135 Hz, Q ≈ 0,5003), calculés par
//! bilinéaire (RBJ) pour chaque fréquence d'échantillonnage — à 48 kHz on
//! retrouve les coefficients de référence de la norme.

/// Filtre biquad (Direct Form II Transposed).
#[derive(Debug, Clone)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// High-shelf du filtre K (formulation ITU-R BS.1770 / libebur128 :
    /// à 48 kHz on retrouve exactement les coefficients de référence de la norme).
    fn high_shelf(f0: f64, g_db: f64, q: f64, fs: f64) -> Self {
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let vh = 10f64.powf(g_db / 20.0);
        let vb = vh.powf(0.499666774155);
        let a0 = 1.0 + k / q + k * k;
        let b0 = (vh + vb * k / q + k * k) / a0;
        let b1 = 2.0 * (k * k - vh) / a0;
        let b2 = (vh - vb * k / q + k * k) / a0;
        let a1 = 2.0 * (k * k - 1.0) / a0;
        let a2 = (1.0 - k / q + k * k) / a0;
        Self::new(b0, b1, b2, a1, a2)
    }

    /// High-pass RLB du filtre K (formulation bilinéaire K = tan(pi f0 / fs),
    /// normalisée b0 = 1 : à 48 kHz on retrouve b = [1, -2, 1]).
    fn high_pass(f0: f64, q: f64, fs: f64) -> Self {
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let a0 = 1.0 + k / q + k * k;
        Self::new(
            1.0,
            -2.0,
            1.0,
            2.0 * (k * k - 1.0) / a0,
            (1.0 - k / q + k * k) / a0,
        )
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Poids par canal BS.1770 (ordre 5.1 : L R C LFE Ls Rs) ; LFE = 0.
fn channel_weight(channels: usize, idx: usize) -> f64 {
    if channels >= 6 && idx == 3 {
        0.0 // LFE
    } else if channels >= 6 && (idx == 4 || idx == 5) {
        1.41 // Ls / Rs
    } else {
        1.0
    }
}

/// Mesure de loudness intégrée en streaming (blocs 400 ms, recouvrement 75 %,
/// double seuil -70 LUFS absolu puis relatif -10 LU).
pub struct LoudnessMeter {
    channels: usize,
    /// Cascade K par canal : [high-shelf, high-pass].
    filters: Vec<[Biquad; 2]>,
    block_len: usize,
    step: usize,
    /// Accumulateur de somme des carrés filtrés par canal dans le bloc courant.
    block_sum: Vec<f64>,
    /// Puissances des blocs écoulés (étape de 100 ms).
    block_powers: Vec<f64>,
    /// Tampon interleaved partiel en attente d'un bloc complet.
    pending: Vec<f32>,
}

const ABS_GATE_LUFS: f64 = -70.0;
const REL_GATE_LU: f64 = -10.0;
const OFFSET_LUFS: f64 = -0.691;

impl LoudnessMeter {
    pub fn new(rate: u32, channels: u8) -> Self {
        let fs = rate as f64;
        let ch = channels.max(1) as usize;
        let filters = (0..ch)
            .map(|_| {
                [
                    Biquad::high_shelf(1681.974450955532, 3.99984385397335, 0.707175236955419, fs),
                    Biquad::high_pass(38.1354708760244, 0.500327037323877, fs),
                ]
            })
            .collect();
        let block_len = (0.4 * fs) as usize;
        let step = (0.1 * fs) as usize;
        LoudnessMeter {
            channels: ch,
            filters,
            block_len,
            step,
            block_sum: vec![0.0; ch],
            block_powers: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Pousse des échantillons interleavés f32 (tous canaux).
    pub fn push(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        let frame = self.channels;
        while self.pending.len() >= self.block_len * frame {
            // extrait un bloc complet
            let block: Vec<f32> = self.pending.drain(..self.block_len * frame).collect();
            for (c, f) in self.filters.iter_mut().enumerate() {
                let mut sum = 0.0;
                for frame_i in 0..self.block_len {
                    let x = block[frame_i * frame + c] as f64;
                    let y0 = f[0].tick(x);
                    let y = f[1].tick(y0);
                    sum += y * y;
                }
                self.block_sum[c] = sum;
            }
            self.emit_block();
            // recouvrement 75 % : on remet 3/4 du bloc (re-filtré depuis le tampon
            // n'est pas possible ; on pousse les échantillons bruts restants et on
            // laisse le bloc glissant se faire par pas de `step`).
            // -> on conserve les 3 derniers quarts dans pending :
            //    (fait ci-dessous via re-insertion)
            let keep = self.block_len - self.step;
            let kept: Vec<f32> = block[keep * frame..].to_vec();
            let mut new_pending = kept;
            new_pending.extend_from_slice(&self.pending);
            self.pending = new_pending;
        }
    }

    fn emit_block(&mut self) {
        let mut power = 0.0;
        for (c, sum) in self.block_sum.iter().enumerate() {
            let w = channel_weight(self.channels, c);
            power += w * sum / self.block_len as f64;
        }
        self.block_powers.push(power);
    }

    /// Loudness intégrée (LUFS) ; `f64::NEG_INFINITY` si aucun bloc valide.
    pub fn integrated(&self) -> f64 {
        let gate = 10f64.powf((ABS_GATE_LUFS - OFFSET_LUFS) / 10.0);
        let above: Vec<f64> = self
            .block_powers
            .iter()
            .copied()
            .filter(|&p| p > gate)
            .collect();
        if above.is_empty() {
            return f64::NEG_INFINITY;
        }
        let mean = above.iter().sum::<f64>() / above.len() as f64;
        let rel_threshold =
            10f64.powf((OFFSET_LUFS + 10.0 * mean.log10() + REL_GATE_LU - OFFSET_LUFS) / 10.0);
        let gated: Vec<f64> = above.into_iter().filter(|&p| p > rel_threshold).collect();
        if gated.is_empty() {
            return f64::NEG_INFINITY;
        }
        OFFSET_LUFS + 10.0 * (gated.iter().sum::<f64>() / gated.len() as f64).log10()
    }
}

/// Gain linéaire (RepleyGain-style) pour amener `lufs` à `target_lufs`,
/// plafonné à ±15 dB.
pub fn gain_for_target(lufs: f64, target_lufs: f64) -> f32 {
    if !lufs.is_finite() {
        return 1.0;
    }
    let db = (target_lufs - lufs).clamp(-15.0, 15.0);
    10f32.powf(db as f32 / 20.0)
}

/// Analyse la loudness intégrée d'un fichier local en le décodant entièrement
/// (tâche de fond — prévu pour un scan de bibliothèque, pas du temps réel).
pub fn analyze_file(path: &std::path::Path) -> auris_core::Result<f64> {
    use crate::decoder::{open_decoder_with_path, ChunkData, TrackMeta};
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = std::fs::File::open(path)
        .map_err(|e| auris_core::AurisError::other(format!("open {}: {e}", path.display())))?;
    let mut dec = open_decoder_with_path(
        Box::new(file),
        &ext,
        TrackMeta::default(),
        Some(path.to_path_buf()),
    )?;
    let spec = dec.spec();
    if spec.is_dsd_dop {
        return Err(auris_core::AurisError::other(
            "loudness: DSD DoP non supporté",
        ));
    }
    let mut meter = LoudnessMeter::new(spec.rate, spec.channels);
    while let Some(chunk) = dec.next_chunk(16384)? {
        match chunk {
            ChunkData::F32(v) => meter.push(&v),
            ChunkData::I24Dop(_) => {}
        }
    }
    Ok(meter.integrated())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f64, dbfs: f64, channels: u8, both: bool) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        let amp = 10f64.powf(dbfs / 20.0) as f32;
        let mut out = Vec::with_capacity(n * channels as usize);
        for i in 0..n {
            let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin();
            for c in 0..channels as usize {
                out.push(if c == 0 || both { s } else { 0.0 });
            }
        }
        out
    }

    #[test]
    fn coefficients_48k_match_reference() {
        // Coefficients de référence ITU-R BS.1770 à 48 kHz.
        let shelf = Biquad::high_shelf(
            1681.974450955532,
            3.99984385397335,
            0.707175236955419,
            48000.0,
        );
        let b0 = 1.53512485958697;
        assert!((shelf.b0 - b0).abs() < 1e-9, "b0 shelf: {}", shelf.b0);
        assert!((shelf.b1 - -2.69169618940638).abs() < 1e-9);
        assert!((shelf.b2 - 1.19839281085285).abs() < 1e-9);
        assert!((shelf.a1 - -1.69065929318241).abs() < 1e-9);
        assert!((shelf.a2 - 0.73248077421585).abs() < 1e-9);
        let hp = Biquad::high_pass(38.1354708760244, 0.500327037323877, 48000.0);
        assert!((hp.b0 - 1.0).abs() < 1e-9);
        assert!((hp.b1 - -2.0).abs() < 1e-9);
        assert!((hp.a1 - -1.99004745483398).abs() < 1e-9);
        assert!((hp.a2 - 0.99007225036621).abs() < 1e-9);
    }

    #[test]
    fn sine_1k_minus23dbfs_mesure_moins23_lufs() {
        let mut m = LoudnessMeter::new(48000, 2);
        m.push(&sine(48000, 5.0, -23.0, 2, true));
        let l = m.integrated();
        assert!((l - -23.0).abs() < 0.2, "lufs={l}");
    }

    #[test]
    fn mono_vs_stereo_3db() {
        let mut stereo = LoudnessMeter::new(48000, 2);
        stereo.push(&sine(48000, 5.0, -23.0, 2, true));
        let mut mono = LoudnessMeter::new(48000, 2);
        mono.push(&sine(48000, 5.0, -23.0, 2, false)); // un seul canal
        let diff = stereo.integrated() - mono.integrated();
        assert!((diff - 3.01).abs() < 0.2, "diff={diff}");
    }

    #[test]
    fn silence_returns_neg_inf() {
        let mut m = LoudnessMeter::new(44100, 2);
        m.push(&vec![0.0f32; 44100 * 2]);
        assert_eq!(m.integrated(), f64::NEG_INFINITY);
    }

    #[test]
    fn gain_clamp() {
        let g = gain_for_target(-20.0, -14.0);
        assert!((g - 10f32.powf(6.0 / 20.0)).abs() < 1e-4);
        // plafond +15 dB
        assert!((gain_for_target(-40.0, -14.0) - 10f32.powf(15.0 / 20.0)).abs() < 1e-4);
        // pas de gain si pas de mesure
        assert_eq!(gain_for_target(f64::NEG_INFINITY, -14.0), 1.0);
    }

    #[test]
    fn streaming_par_petits_bouts() {
        let data = sine(44100, 3.0, -18.0, 2, true);
        let mut m1 = LoudnessMeter::new(44100, 2);
        m1.push(&data);
        let mut m2 = LoudnessMeter::new(44100, 2);
        for chunk in data.chunks(1000) {
            m2.push(chunk);
        }
        assert!((m1.integrated() - m2.integrated()).abs() < 1e-6);
    }
}
