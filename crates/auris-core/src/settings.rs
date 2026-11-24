//! Réglages de l'application (équivalent du menu « Réglages » de Volumio Nova).
//!
//! Persistés dans `<data-dir>/config.toml` (section `[settings]`), exposés via
//! `GET/PUT /api/settings`, appliqués au moteur de lecture à chaud.

use serde::{Deserialize, Serialize};

/// Thème de l'interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    /// Suit `prefers-color-scheme` du navigateur.
    System,
}

/// Courbe de volume (comme Volumio : « Natural (Logarithmic) » vs linéaire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VolumeCurve {
    /// Logarithmique : gain = (v/100)^2 — perception naturelle.
    #[default]
    Logarithmic,
    /// Linéaire : gain = v/100.
    Linear,
}

/// Mode de volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VolumeMode {
    /// Volume logiciel (f32) appliqué dans le callback — défaut actuel.
    #[default]
    Software,
    /// Bit-perfect : aucun traitement, gain = 1.0, le volume passe par le DAC/ampli.
    Disabled,
}

/// Mode de lecture des fichiers DSD (DSF/DFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DsdMode {
    /// DSD over PCM 24 bits vers un DAC compatible DoP.
    #[default]
    Dop,
    /// DSD converti en PCM (haute fréquence) — utile sans DAC DoP.
    Pcm,
}

/// Réglages de lecture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub volume_curve: VolumeCurve,
    /// Volume maximal autorisé (0-100). Le volume courant est borné par cette valeur.
    pub max_volume: u8,
    pub volume_mode: VolumeMode,
    /// Lecture sans trou entre pistes de même format (gapless).
    pub gapless: bool,
    /// Rééchantillonnage autorisé (false = bit-perfect, aucun resample).
    pub resampling: bool,
    pub dsd_mode: DsdMode,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        PlaybackSettings {
            volume_curve: VolumeCurve::Logarithmic,
            max_volume: 100,
            volume_mode: VolumeMode::Software,
            gapless: true,
            resampling: false,
            dsd_mode: DsdMode::Dop,
        }
    }
}

/// Réglages d'apparence de l'UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: Theme,
    /// Couleur d'accent au format `#RRGGBB` (défaut `#1ED760`, vert charter).
    pub accent: String,
    /// Image de fond optionnelle (URL ou chemin servi par l'UI).
    pub background: Option<String>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        AppearanceSettings {
            theme: Theme::Dark,
            accent: "#1ED760".into(),
            background: None,
        }
    }
}

/// Réglages système.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemSettings {
    /// Nom affiché de l'appareil (découverte LAN, UI).
    pub device_name: String,
    /// Fuseau horaire IANA (ex. `Europe/Paris`) — cosmétique pour l'UI.
    pub timezone: Option<String>,
}

impl Default for SystemSettings {
    fn default() -> Self {
        SystemSettings {
            device_name: "auris".into(),
            timezone: None,
        }
    }
}

/// Réglages réseau (informations + nom d'hôte).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// Nom d'hôte souhaité (appliqué au mieux selon l'OS, Linux uniquement).
    pub hostname: Option<String>,
}

/// Réglages des sources (toggles de la page Sources de Nova).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesSettings {
    /// Scan des métadonnées à l'indexation (tags + pochettes).
    pub metadata_discovery: bool,
}

impl Default for SourcesSettings {
    fn default() -> Self {
        SourcesSettings {
            metadata_discovery: true,
        }
    }
}

/// Tous les réglages, racine de la section `[settings]` de config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub playback: PlaybackSettings,
    pub appearance: AppearanceSettings,
    pub system: SystemSettings,
    pub network: NetworkSettings,
    pub sources: SourcesSettings,
}

impl Settings {
    /// Validation avant persistance. Retourne un message d'erreur clair (HTTP 400).
    pub fn validate(&self) -> Result<(), String> {
        if self.playback.max_volume == 0 || self.playback.max_volume > 100 {
            return Err("max_volume doit être entre 1 et 100".into());
        }
        let a = self.appearance.accent.trim();
        let hex_ok =
            a.len() == 7 && a.starts_with('#') && a[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !hex_ok {
            return Err(format!(
                "accent doit être une couleur hex `#RRGGBB` (reçu: `{a}`)"
            ));
        }
        let name = self.system.device_name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("device_name doit faire entre 1 et 64 caractères".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defauts_sains() {
        let s = Settings::default();
        assert_eq!(s.playback.volume_curve, VolumeCurve::Logarithmic);
        assert_eq!(s.playback.max_volume, 100);
        assert_eq!(s.playback.volume_mode, VolumeMode::Software);
        assert!(s.playback.gapless);
        assert!(!s.playback.resampling);
        assert_eq!(s.appearance.theme, Theme::Dark);
        assert_eq!(s.appearance.accent, "#1ED760");
        assert_eq!(s.system.device_name, "auris");
        assert!(s.sources.metadata_discovery);
        s.validate().unwrap();
    }

    #[test]
    fn validation_max_volume() {
        let mut s = Settings::default();
        s.playback.max_volume = 0;
        assert!(s.validate().is_err());
        s.playback.max_volume = 101;
        assert!(s.validate().is_err());
        s.playback.max_volume = 50;
        s.validate().unwrap();
    }

    #[test]
    fn validation_accent() {
        let mut s = Settings::default();
        for bad in ["1ED760", "#1ED76", "#GGGGGG", "red", "#1ed7600"] {
            s.appearance.accent = bad.into();
            assert!(s.validate().is_err(), "{bad} devrait être refusé");
        }
        s.appearance.accent = "#ff8800".into();
        s.validate().unwrap();
    }

    #[test]
    fn validation_device_name() {
        let mut s = Settings::default();
        s.system.device_name = "  ".into();
        assert!(s.validate().is_err());
        s.system.device_name = "x".repeat(65);
        assert!(s.validate().is_err());
        s.system.device_name = " salon ".into();
        s.validate().unwrap();
    }

    #[test]
    fn serde_roundtrip_et_defaut_partiel() {
        let s = Settings::default();
        let txt = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&txt).unwrap();
        assert_eq!(s, back);
        // un TOML partiel (ex. juste max_volume) complète avec les défauts
        let partial: Settings = toml::from_str("[playback]\nmax_volume = 80\n").unwrap();
        assert_eq!(partial.playback.max_volume, 80);
        assert_eq!(partial.appearance.theme, Theme::Dark);
        assert_eq!(partial.playback.volume_mode, VolumeMode::Software);
    }
}
