//! Égaliseur paramétrique : biquads « peaking EQ » (recettes RBJ) en cascade,
//! appliqués au PCM f32 avant le gain de volume (donc sur DAC local et « Jouer ici »).
//!
//! La configuration est poussée à chaud via [`EqConfig`] (champ `version` incrémenté
//! à chaque changement) ; [`EqProcessor`] reconstruit la chaîne quand la version ou
//! la cadence change, ce qui permet de modifier l'EQ pendant la lecture sans coupure.

/// Q des bandes « peaking » (≈ 1 octave de largeur).
pub const EQ_BAND_Q: f32 = 1.0;

/// Un biquad (forme directe I) : y = b0*x + b1*x1 + b2*x2 - a1*y1 - a2*y2.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Biquad {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// Filtre « peaking EQ » RBJ : gain `gain_db` à `freq_hz`, Q donné, cadence `sample_rate`.
    pub fn peaking(freq_hz: f32, gain_db: f32, q: f32, sample_rate: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q.max(1e-3));
        let cos_w0 = w0.cos();
        let a0 = 1.0 + alpha / a;
        let b0 = (1.0 + alpha * a) / a0;
        let b1 = (-2.0 * cos_w0) / a0;
        let b2 = (1.0 - alpha * a) / a0;
        let a1 = (-2.0 * cos_w0) / a0;
        let a2 = (1.0 - alpha / a) / a0;
        Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            ..Default::default()
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Paramètres d'une bande, poussés par les réglages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBandParams {
    pub freq_hz: u32,
    pub gain_db: f32,
}

/// Configuration partagée de l'EQ (mise à jour à chaud par le moteur).
#[derive(Debug, Clone, Default)]
pub struct EqConfig {
    pub enabled: bool,
    pub bands: Vec<EqBandParams>,
    /// Incrémenté à chaque changement de `enabled`/`bands`.
    pub version: u64,
}

/// Chaîne de biquads : une instance par bande et par canal audio.
#[derive(Debug, Clone)]
pub struct EqChain {
    channels: usize,
    /// bands[bande][canal]
    bands: Vec<Vec<Biquad>>,
}

impl EqChain {
    /// Construit la chaîne pour `sample_rate` et `channels`.
    /// Les bandes de gain nul (< 1e-6 dB) ou hors Nyquist sont ignorées.
    pub fn new(bands: &[EqBandParams], sample_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let nyquist = sample_rate as f32 / 2.0;
        let mut chain_bands = Vec::new();
        for b in bands {
            if b.gain_db.abs() < 1e-6 || b.freq_hz as f32 >= nyquist {
                continue;
            }
            let f0 = (b.freq_hz as f32).min(nyquist * 0.98);
            let mut per_ch = Vec::with_capacity(channels);
            for _ in 0..channels {
                per_ch.push(Biquad::peaking(
                    f0,
                    b.gain_db,
                    EQ_BAND_Q,
                    sample_rate as f32,
                ));
            }
            chain_bands.push(per_ch);
        }
        EqChain {
            channels,
            bands: chain_bands,
        }
    }

    /// Vrai si aucune bande active (rendu = identité).
    pub fn is_flat(&self) -> bool {
        self.bands.is_empty()
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Traite un échantillon du canal `ch`.
    #[inline]
    pub fn process_sample(&mut self, ch: usize, x: f32) -> f32 {
        let mut y = x;
        let ch = ch.min(self.channels - 1);
        for band in &mut self.bands {
            y = band[ch].process(y);
        }
        y
    }

    /// Traite un buffer entrelacé (tous canaux).
    pub fn process(&mut self, samples: &mut [f32]) {
        let ch = self.channels;
        for (i, s) in samples.iter_mut().enumerate() {
            *s = self.process_sample(i % ch, *s);
        }
    }

    pub fn reset(&mut self) {
        for band in &mut self.bands {
            for b in band {
                b.reset();
            }
        }
    }
}

/// Processeur tenu dans le callback audio : reconstruit la chaîne à chaud quand
/// la config (version) ou la cadence du flux change.
#[derive(Debug, Default)]
pub struct EqProcessor {
    chain: Option<EqChain>,
    version: u64,
    rate: u32,
}

impl EqProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applique l'EQ à un échantillon du canal `ch` ; identité si EQ désactivé/plat.
    #[inline]
    pub fn process(
        &mut self,
        cfg: &EqConfig,
        sample_rate: u32,
        channels: usize,
        ch: usize,
        x: f32,
    ) -> f32 {
        if !cfg.enabled || cfg.bands.iter().all(|b| b.gain_db.abs() < 1e-6) {
            return x;
        }
        if self.chain.is_none() || self.version != cfg.version || self.rate != sample_rate {
            self.chain = Some(EqChain::new(&cfg.bands, sample_rate, channels));
            self.version = cfg.version;
            self.rate = sample_rate;
        }
        self.chain.as_mut().unwrap().process_sample(ch, x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin() * 0.5)
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn biquad_gain_nul_est_identite() {
        let mut b = Biquad::peaking(1000.0, 0.0, 1.0, 44100.0);
        for x in [0.1, -0.7, 0.3, 0.9, -0.2] {
            let y = b.process(x);
            assert!(
                (y - x).abs() < 1e-5,
                "gain 0 doit être transparent: {x} -> {y}"
            );
        }
    }

    #[test]
    fn chaine_plate_est_identite() {
        let bands = vec![
            EqBandParams {
                freq_hz: 100,
                gain_db: 0.0,
            },
            EqBandParams {
                freq_hz: 1000,
                gain_db: 0.0,
            },
        ];
        let mut chain = EqChain::new(&bands, 44100, 2);
        assert!(chain.is_flat());
        let mut buf = sine(1000.0, 44100, 512);
        let orig = buf.clone();
        chain.process(&mut buf);
        for (a, b) in orig.iter().zip(buf.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn peaking_applique_le_gain_a_la_frequence() {
        let sr = 44100;
        let bands = vec![EqBandParams {
            freq_hz: 1000,
            gain_db: 6.0,
        }];
        let mut chain = EqChain::new(&bands, sr, 1);
        assert!(!chain.is_flat());
        let input = sine(1000.0, sr, sr as usize); // 1 s
        let mut out = input.clone();
        chain.process(&mut out);
        // on ignore la transitoire (1/4 s) : rapport des RMS = 10^(6/20) ≈ 1.995
        let ratio = rms(&out[sr as usize / 4..]) / rms(&input[sr as usize / 4..]);
        let expected = 10f32.powf(6.0 / 20.0);
        assert!(
            (ratio - expected).abs() / expected < 0.05,
            "ratio {ratio} attendu {expected}"
        );
    }

    #[test]
    fn peaking_laisse_les_autres_frequences() {
        let sr = 44100;
        let bands = vec![EqBandParams {
            freq_hz: 1000,
            gain_db: 12.0,
        }];
        let mut chain = EqChain::new(&bands, sr, 1);
        let input = sine(100.0, sr, sr as usize);
        let mut out = input.clone();
        chain.process(&mut out);
        let ratio = rms(&out[sr as usize / 4..]) / rms(&input[sr as usize / 4..]);
        assert!(
            (0.95..1.05).contains(&ratio),
            "100 Hz ne devrait presque pas bouger avec +12 dB à 1 kHz: ratio {ratio}"
        );
    }

    #[test]
    fn stereo_entreleave_les_deux_canaux_identicos() {
        let sr = 48000;
        let bands = vec![EqBandParams {
            freq_hz: 500,
            gain_db: -8.0,
        }];
        let mut chain = EqChain::new(&bands, sr, 2);
        assert_eq!(chain.channels(), 2);
        let mono = sine(500.0, sr, 2048);
        let mut interleaved: Vec<f32> = Vec::with_capacity(mono.len() * 2);
        for s in &mono {
            interleaved.push(*s);
            interleaved.push(*s);
        }
        chain.process(&mut interleaved);
        for i in 0..mono.len() {
            assert_eq!(interleaved[2 * i], interleaved[2 * i + 1]);
        }
        // -8 dB à 500 Hz sur un sinus 500 Hz : ratio ≈ 10^(-8/20) ≈ 0.398
        let ratio = rms(&interleaved[mono.len() / 2..]) / rms(&mono[mono.len() / 4..]);
        let expected = 10f32.powf(-8.0 / 20.0);
        assert!((ratio - expected).abs() / expected < 0.1);
    }

    #[test]
    fn bande_au_dela_de_nyquist_ignoree() {
        let bands = vec![EqBandParams {
            freq_hz: 30000,
            gain_db: 10.0,
        }];
        let chain = EqChain::new(&bands, 44100, 2);
        assert!(chain.is_flat());
    }

    #[test]
    fn processeur_reconstruit_a_chaud() {
        let sr = 44100;
        let mut cfg = EqConfig {
            enabled: true,
            bands: vec![EqBandParams {
                freq_hz: 1000,
                gain_db: 6.0,
            }],
            version: 1,
        };
        let mut p = EqProcessor::new();
        // désactivé -> identité
        cfg.enabled = false;
        assert_eq!(p.process(&cfg, sr, 2, 0, 0.5), 0.5);
        // activé -> traite
        cfg.enabled = true;
        let y = p.process(&cfg, sr, 2, 0, 0.5);
        // au premier échantillon la sortie vaut b0*x (état nul)
        assert!(y != 0.5);
        // changement de version -> chaîne reconstruite (état remis à zéro)
        cfg.bands[0].gain_db = -6.0;
        cfg.version = 2;
        let y2 = p.process(&cfg, sr, 2, 0, 0.5);
        let b = Biquad::peaking(1000.0, -6.0, EQ_BAND_Q, sr as f32);
        assert!(
            (y2 - b.b0 * 0.5).abs() < 1e-6,
            "rebuild attendu après bump de version"
        );
        // changement de cadence -> rebuild aussi
        cfg.version = 3;
        let y3 = p.process(&cfg, 48000, 2, 0, 0.5);
        let b48 = Biquad::peaking(1000.0, -6.0, EQ_BAND_Q, 48000.0);
        assert!((y3 - b48.b0 * 0.5).abs() < 1e-6);
    }
}
