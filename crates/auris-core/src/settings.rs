//! Réglages de l'application (équivalent du menu « Réglages » de Volumio Nova).
//!
//! Persistés dans `<data-dir>/config.toml` (section `[settings]`), exposés via
//! `GET/PUT /api/settings`, appliqués au moteur de lecture à chaud.

use serde::{Deserialize, Serialize};

/// Thèmes intégrés de l'interface (ids réservés ; tout autre id = pack de thème
/// CSS servi par `/api/packs/theme/{id}.css`, embarqué ou déposé dans
/// `<data-dir>/themes/`).
pub const BUILTIN_THEMES: &[&str] = &["dark", "light", "system"];

/// Langues intégrées (packs embarqués dans l'UI) ; toute autre langue ISO
/// (`xx` / `xx-YY`) peut être fournie par un pack communautaire
/// `<data-dir>/locales/{lang}.json`.
pub const BUILTIN_LOCALES: &[&str] = &["fr", "en"];

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

/// Fréquences des bandes de l'égaliseur paramétrique (octaves ISO, 10 bandes).
pub const EQ_BAND_FREQS: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/// Borne du gain d'une bande d'EQ (±20 dB).
pub const EQ_MAX_GAIN_DB: f32 = 20.0;

/// Une bande de l'égaliseur paramétrique.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub freq_hz: u32,
    /// Gain en dB, entre -20 et +20.
    pub gain_db: f32,
}

/// Réglages de l'égaliseur paramétrique (10 bandes par défaut, gain 0 = plat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    pub bands: Vec<EqBand>,
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings {
            enabled: false,
            bands: EQ_BAND_FREQS
                .iter()
                .map(|&freq_hz| EqBand {
                    freq_hz,
                    gain_db: 0.0,
                })
                .collect(),
        }
    }
}

/// Analyse une heure « HH:MM » (24 h). Retourne (heure, minute) ou None si invalide.
pub fn parse_hhmm(s: &str) -> Option<(u8, u8)> {
    let (hs, ms) = s.trim().split_once(':')?;
    if hs.is_empty() || hs.len() > 2 || ms.is_empty() || ms.len() > 2 {
        return None;
    }
    let h: u8 = hs.parse().ok()?;
    let m: u8 = ms.parse().ok()?;
    if h < 24 && m < 60 {
        Some((h, m))
    } else {
        None
    }
}

/// Réglages de lecture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub volume_curve: VolumeCurve,
    /// Volume maximal autorisé (0-100). Le volume courant est borné par cette valeur.
    pub max_volume: u8,
    pub volume_mode: VolumeMode,
    /// Lecture sans trou entre pistes de même format (gapless).
    pub gapless: bool,
    /// Fondu enchaîné (crossfade) entre deux pistes, en secondes (0 = direct,
    /// max 12). Uniquement entre pistes de même format (cadence/canaux).
    pub crossfade_secs: f64,
    /// Rééchantillonnage autorisé (false = bit-perfect, aucun resample).
    pub resampling: bool,
    pub dsd_mode: DsdMode,
    /// Minuterie de sommeil : après N minutes de lecture, pause automatique.
    /// `None` = désactivée (one-shot : à refaire à chaque utilisation).
    pub sleep_timer: Option<u32>,
    /// Heure de réveil « HH:MM » (heure locale) : relance la lecture. `None` = désactivé.
    pub wake_time: Option<String>,
    /// Égaliseur paramétrique appliqué au flux PCM (DAC local et « Jouer ici »).
    pub eq: EqSettings,
    /// Normalisation du volume par loudness intégrée R128 (gain statique par
    /// piste, mesuré à l'analyse de la bibliothèque). false = aucun gain (#37).
    pub loudness: bool,
    /// Cible de loudness en LUFS (streaming -14, broadcast EBU R128 -23).
    pub loudness_target_lufs: f64,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        PlaybackSettings {
            volume_curve: VolumeCurve::Logarithmic,
            max_volume: 100,
            volume_mode: VolumeMode::Software,
            gapless: true,
            crossfade_secs: 0.0,
            resampling: false,
            dsd_mode: DsdMode::Dop,
            sleep_timer: None,
            wake_time: None,
            eq: EqSettings::default(),
            loudness: false,
            loudness_target_lufs: -14.0,
        }
    }
}

/// Réglages d'apparence de l'UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Id du thème : `dark` / `light` / `system` (intégrés) ou id d'un pack de
    /// thème CSS communautaire (ex. `aurora`).
    pub theme: String,
    /// Langue de l'interface (code ISO `xx` ou `xx-YY`, ex. `fr`, `en`).
    pub language: String,
    /// Couleur d'accent au format `#RRGGBB` (défaut `#1ED760`, vert charter).
    pub accent: String,
    /// Image de fond optionnelle (URL ou chemin servi par l'UI).
    pub background: Option<String>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        AppearanceSettings {
            theme: "dark".into(),
            language: "fr".into(),
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

/// Réglages réseau (informations + nom d'hôte + enceinte DLNA).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// Nom d'hôte souhaité (appliqué au mieux selon l'OS, Linux uniquement).
    pub hostname: Option<String>,
    /// Enceinte DLNA (MediaRenderer UPnP) : Auris annonce un AVTransport +
    /// RenderingControl en SSDP pour recevoir du contenu poussé (#35).
    pub dlna_renderer: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        NetworkSettings {
            hostname: None,
            dlna_renderer: true,
        }
    }
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

/// Réglages Last.fm (scrobble + « en cours de lecture ») — #26.
/// La clé API + le secret partagé viennent de https://www.last.fm/api/account/create ;
/// la session (`sk`) s'obtient par le flux auth.getToken → autorisation web → auth.getSession.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LastfmSettings {
    pub enabled: bool,
    /// API key (32 caractères hex) du compte Last.fm.
    pub api_key: String,
    /// Shared secret du compte API (affiché sur la page du compte).
    pub shared_secret: String,
    /// Session key (`sk`) obtenue après autorisation — requise pour scrobbler.
    pub session_key: String,
    /// Endpoint API (défaut `https://ws.audioscrobbler.com/2.0/`) — surchargeable
    /// pour les tests.
    pub api_url: Option<String>,
}

/// Réglages Spotify Connect (librespot en processus enfant) — #24.
/// Auris apparaît comme enceinte dans l'app Spotify ; le flux est fourni
/// par Spotify au client authentifié (aucun DRM contourné).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotifySettings {
    pub enabled: bool,
    /// Nom de l'enceinte affiché dans l'app Spotify (vide = nom du périphérique).
    pub device_name: String,
    /// Nom d'utilisateur Spotify (auth legacy, avec `password`).
    pub username: String,
    /// Mot de passe Spotify — laisser vide si `credentials_path` est renseigné.
    pub password: String,
    /// Chemin vers un `credentials.json` librespot (auth recommandée, OAuth).
    pub credentials_path: String,
    /// Débit du flux : 96, 160 ou 320 kbps.
    pub bitrate: u16,
    /// Backend librespot (vide = auto : `rodio` sur Windows, `alsa` sinon).
    pub backend: String,
    /// Périphérique ALSA (backend `alsa` ; « default » = sortie système).
    pub alsa_device: String,
    /// Dossier de cache librespot (vide = `<data-dir>/spotify-cache`).
    pub cache_dir: String,
}

impl Default for SpotifySettings {
    fn default() -> Self {
        SpotifySettings {
            enabled: false,
            device_name: String::new(),
            username: String::new(),
            password: String::new(),
            credentials_path: String::new(),
            bitrate: 320,
            backend: String::new(),
            alsa_device: "default".to_string(),
            cache_dir: String::new(),
        }
    }
}

/// Réglages MusicBrainz (fiche recording : sortie, label, tags, ISRC, note) — #26.
/// L'API WS2 est publique, sans clé ; 1 requête/s max (rate-limit côté client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicBrainzSettings {
    pub enabled: bool,
    /// Base WS2 (défaut `https://musicbrainz.org/ws/2`) — surchargeable pour les tests.
    pub api_url: Option<String>,
}

impl Default for MusicBrainzSettings {
    fn default() -> Self {
        MusicBrainzSettings {
            enabled: true,
            api_url: None,
        }
    }
}

/// Mode de pont vers CamillaDSP (convolution / correction de pièce) — #82.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CamillaMode {
    /// Boucle (loopback ALSA ou équivalent) : le flux PCM est routé vers le
    /// périphérique de capture de CamillaDSP (`capture_device`, ex.
    /// `camilladsp`), qui le traite (FIR, convolution) puis le joue sur le DAC.
    #[default]
    Loopback,
    /// Pipe nommé (FIFO) : le flux PCM f32le brut est écrit dans le FIFO
    /// `capture_device` (capture `File`/`RawFile` côté CamillaDSP).
    Pipe,
    /// Websocket seul : aucun reroutage audio, on pilote une instance
    /// CamillaDSP (sélection de profil) qui gère la sortie autrement.
    Websocket,
}

/// Réglages du pont CamillaDSP — #82. Quand `enabled`, Auris délègue le
/// traitement (EQ/convolution) à une instance CamillaDSP joignable, avec
/// repli sur le DSP interne si elle est indisponible (`fallback_internal`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CamillaSettings {
    /// Délégation du traitement vers CamillaDSP (false = DSP interne #37).
    pub enabled: bool,
    pub mode: CamillaMode,
    /// URL du websocket CamillaDSP (défaut `ws://127.0.0.1:1234`).
    pub ws_url: String,
    /// Mode loopback : nom du périphérique de capture (ex. `camilladsp`) ;
    /// mode pipe : chemin du FIFO. Ignoré en mode websocket.
    pub capture_device: String,
    /// Profil (fichier de config) à activer à la connexion via
    /// `SetConfigName`. `None` = laisser le profil courant de CamillaDSP.
    pub profile: Option<String>,
    /// Si CamillaDSP est injoignable ou le périphérique de capture indisponible :
    /// repli sur le DSP interne + sortie normale (true, défaut) plutôt que de
    /// rester muet.
    pub fallback_internal: bool,
}

impl Default for CamillaSettings {
    fn default() -> Self {
        CamillaSettings {
            enabled: false,
            mode: CamillaMode::Loopback,
            ws_url: "ws://127.0.0.1:1234".into(),
            capture_device: "camilladsp".into(),
            profile: None,
            fallback_internal: true,
        }
    }
}

/// Statut d'une instance CamillaDSP sondée (GET /api/camilla/status) — #82.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CamillaStatus {
    pub reachable: bool,
    /// Version de CamillaDSP (réponse `GetVersion`).
    pub version: Option<String>,
    /// État interne (« Running », « Paused », … — réponse `GetStatus`).
    pub state: Option<String>,
    /// Profil actif côté CamillaDSP (`GetConfigName`).
    pub active_profile: Option<String>,
    /// Profils (configs) disponibles (`GetConfigNames`).
    pub profiles: Vec<String>,
    /// Message d'erreur de la sonde, si injoignable.
    pub error: Option<String>,
}

/// Tous les réglages, racine de la section `[settings]` de config.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub playback: PlaybackSettings,
    pub appearance: AppearanceSettings,
    pub system: SystemSettings,
    pub network: NetworkSettings,
    pub sources: SourcesSettings,
    pub lastfm: LastfmSettings,
    pub musicbrainz: MusicBrainzSettings,
    pub spotify: SpotifySettings,
    pub camilla: CamillaSettings,
}

impl Settings {
    /// Validation avant persistance. Retourne un message d'erreur clair (HTTP 400).
    pub fn validate(&self) -> Result<(), String> {
        if self.playback.max_volume == 0 || self.playback.max_volume > 100 {
            return Err("max_volume doit être entre 1 et 100".into());
        }
        let xf = self.playback.crossfade_secs;
        if !xf.is_finite() || xf < 0.0 || xf > 12.0 {
            return Err("crossfade_secs doit être entre 0 et 12".into());
        }
        let a = self.appearance.accent.trim();
        let hex_ok =
            a.len() == 7 && a.starts_with('#') && a[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !hex_ok {
            return Err(format!(
                "accent doit être une couleur hex `#RRGGBB` (reçu: `{a}`)"
            ));
        }
        // thème : id intégré ou id de pack (alphanum + tiret, ex. `aurora`)
        let th = self.appearance.theme.trim();
        let theme_ok = BUILTIN_THEMES.contains(&th)
            || (!th.is_empty()
                && th.len() <= 64
                && th
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        if !theme_ok {
            return Err(format!(
                "theme doit être dark/light/system ou un id de pack [a-z0-9-] (reçu: `{th}`)"
            ));
        }
        // langue : code ISO simplifié `xx` ou `xx-YY` (pack de langue embarqué ou communautaire)
        let lang = self.appearance.language.trim();
        let lang_ok = (lang.len() == 2 && lang.chars().all(|c| c.is_ascii_lowercase()))
            || (lang.len() == 5
                && lang.chars().enumerate().all(|(i, c)| {
                    if i == 2 {
                        c == '-'
                    } else if i < 2 {
                        c.is_ascii_lowercase()
                    } else {
                        c.is_ascii_uppercase()
                    }
                }));
        if !lang_ok {
            return Err(format!(
                "language doit être un code ISO `xx` ou `xx-YY` (reçu: `{lang}`)"
            ));
        }
        let name = self.system.device_name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("device_name doit faire entre 1 et 64 caractères".into());
        }
        if let Some(m) = self.playback.sleep_timer {
            if m == 0 || m > 480 {
                return Err("sleep_timer doit être entre 1 et 480 minutes".into());
            }
        }
        if let Some(w) = &self.playback.wake_time {
            if parse_hhmm(w).is_none() {
                return Err(format!(
                    "wake_time doit être au format HH:MM 24 h (reçu: `{w}`)"
                ));
            }
        }
        let eq = &self.playback.eq;
        if eq.bands.len() > 32 {
            return Err("l'égaliseur accepte au plus 32 bandes".into());
        }
        for b in &eq.bands {
            if b.freq_hz < 20 || b.freq_hz > 20000 {
                return Err(format!(
                    "fréquence de bande EQ hors plage 20-20000 Hz: {}",
                    b.freq_hz
                ));
            }
            if !b.gain_db.is_finite() || b.gain_db.abs() > EQ_MAX_GAIN_DB {
                return Err(format!(
                    "gain de bande EQ hors plage ±{EQ_MAX_GAIN_DB} dB: {}",
                    b.gain_db
                ));
            }
        }
        // Last.fm (#26) : si activé, les 3 identifiants sont requis et la clé
        // API doit être hexadécimale (32 caractères côté Last.fm).
        let lf = &self.lastfm;
        if lf.enabled {
            if lf.api_key.trim().is_empty()
                || lf.shared_secret.trim().is_empty()
                || lf.session_key.trim().is_empty()
            {
                return Err(
                    "Last.fm activé : api_key, shared_secret et session_key sont requis".into(),
                );
            }
            let k = lf.api_key.trim();
            if !k.chars().all(|c| c.is_ascii_hexdigit()) || k.len() != 32 {
                return Err(format!(
                    "lastfm.api_key doit être une clé API hex de 32 caractères (reçu: `{k}`)"
                ));
            }
        }
        // Spotify Connect (#24) : débit autorisé, nom d'enceinte borné, et
        // si activé il faut un compte (identifiants ou credentials.json).
        let sp = &self.spotify;
        if ![96, 160, 320].contains(&sp.bitrate) {
            return Err(format!(
                "spotify.bitrate doit être 96, 160 ou 320 (reçu: {})",
                sp.bitrate
            ));
        }
        let sdn = sp.device_name.trim();
        if sdn.len() > 64 {
            return Err("spotify.device_name doit faire au plus 64 caractères".into());
        }
        if sp.enabled && sp.username.trim().is_empty() && sp.credentials_path.trim().is_empty() {
            return Err(
                "Spotify Connect activé : renseignez username+password ou credentials_path".into(),
            );
        }
        // CamillaDSP (#82) : URL websocket ws:// ou wss:// avec un hôte, et
        // périphérique de capture renseigné sauf en mode « websocket ».
        let ca = &self.camilla;
        let url = ca.ws_url.trim();
        let scheme_ok = url.starts_with("ws://") || url.starts_with("wss://");
        let authority = url
            .strip_prefix("wss://")
            .or_else(|| url.strip_prefix("ws://"))
            .unwrap_or("");
        let url_ok = scheme_ok
            && url.len() <= 256
            && !authority.is_empty()
            && !authority.chars().any(char::is_whitespace);
        if !url_ok {
            return Err(format!(
                "camilla.ws_url doit être une URL ws:// ou wss:// avec hôte (reçu: `{url}`)"
            ));
        }
        if ca.mode != CamillaMode::Websocket {
            let cd = ca.capture_device.trim();
            if cd.is_empty() || cd.len() > 256 {
                return Err(
                    "camilla.capture_device est requis (hors mode websocket), 1-256 caractères"
                        .into(),
                );
            }
        }
        if let Some(p) = &ca.profile {
            let p = p.trim();
            if p.is_empty() || p.len() > 128 {
                return Err("camilla.profile doit faire entre 1 et 128 caractères".into());
            }
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
        assert_eq!(s.playback.dsd_mode, DsdMode::Dop);
        assert_eq!(s.playback.sleep_timer, None);
        assert_eq!(s.playback.wake_time, None);
        assert!(!s.playback.eq.enabled);
        assert_eq!(s.playback.eq.bands.len(), EQ_BAND_FREQS.len());
        assert!(s.playback.eq.bands.iter().all(|b| b.gain_db == 0.0));
        assert_eq!(s.appearance.theme, "dark");
        assert_eq!(s.appearance.accent, "#1ED760");
        assert_eq!(s.system.device_name, "auris");
        assert!(s.sources.metadata_discovery);
        assert!(!s.spotify.enabled);
        assert_eq!(s.spotify.bitrate, 320);
        assert!(!s.camilla.enabled);
        assert_eq!(s.camilla.mode, CamillaMode::Loopback);
        assert_eq!(s.camilla.ws_url, "ws://127.0.0.1:1234");
        assert!(s.camilla.fallback_internal);
        s.validate().unwrap();
    }

    #[test]
    fn validation_camilla() {
        let mut s = Settings::default();
        // défauts valides
        s.validate().unwrap();
        // URL websocket invalide
        for bad in [
            "",
            "http://127.0.0.1:1234",
            "ws://",
            "ws://ho st:1234",
            "ws",
        ] {
            s.camilla.ws_url = bad.into();
            assert!(s.validate().is_err(), "{bad:?} devrait être refusé");
        }
        s.camilla.ws_url = "ws://127.0.0.1:1234".into();
        s.camilla.ws_url = "wss://dsp.local:1234".into();
        s.validate().unwrap();
        s.camilla.ws_url = "ws://127.0.0.1:1234".into();
        // capture_device requis hors mode websocket
        s.camilla.capture_device = "  ".into();
        assert!(s.validate().is_err());
        s.camilla.mode = CamillaMode::Websocket;
        s.validate().unwrap(); // vide toléré en mode websocket
        s.camilla.mode = CamillaMode::Pipe;
        assert!(s.validate().is_err());
        s.camilla.capture_device = "/tmp/camilla.fifo".into();
        s.validate().unwrap();
        // profil borné
        s.camilla.profile = Some("x".repeat(129));
        assert!(s.validate().is_err());
        s.camilla.profile = Some("correction-salon.yml".into());
        s.validate().unwrap();
        // sérialisation : la section camilla absente d'un vieux TOML -> défaut
        let old: Settings = toml::from_str("[playback]\nmax_volume = 80\n").unwrap();
        assert!(!old.camilla.enabled);
        assert_eq!(old.camilla.ws_url, "ws://127.0.0.1:1234");
    }

    #[test]
    fn spotify_validation() {
        let mut s = Settings::default();
        s.spotify.enabled = true;
        // activé mais aucun compte renseigné
        assert!(s.validate().is_err());
        s.spotify.username = "user".into();
        s.spotify.password = "secret".into();
        s.validate().unwrap();
        // débit non autorisé
        s.spotify.bitrate = 128;
        assert!(s.validate().is_err());
        s.spotify.bitrate = 96;
        // credentials_path seul suffit
        s.spotify.username.clear();
        s.spotify.credentials_path = "/tmp/credentials.json".into();
        s.validate().unwrap();
    }

    #[test]
    fn parse_hhmm_valide_et_invalide() {
        assert_eq!(parse_hhmm("07:30"), Some((7, 30)));
        assert_eq!(parse_hhmm("23:59"), Some((23, 59)));
        assert_eq!(parse_hhmm("00:00"), Some((0, 0)));
        assert_eq!(parse_hhmm("7:05"), Some((7, 5)));
        for bad in ["", "7", "24:00", "12:60", "ab:cd", "1:2:3", "-1:00"] {
            assert!(parse_hhmm(bad).is_none(), "{bad:?} devrait être refusé");
        }
    }

    #[test]
    fn validation_sleep_timer() {
        let mut s = Settings::default();
        s.playback.sleep_timer = Some(0);
        assert!(s.validate().is_err());
        s.playback.sleep_timer = Some(481);
        assert!(s.validate().is_err());
        s.playback.sleep_timer = Some(60);
        s.validate().unwrap();
        s.playback.sleep_timer = None;
        s.validate().unwrap();
    }

    #[test]
    fn validation_wake_time() {
        let mut s = Settings::default();
        s.playback.wake_time = Some("25:00".into());
        assert!(s.validate().is_err());
        s.playback.wake_time = Some("matin".into());
        assert!(s.validate().is_err());
        s.playback.wake_time = Some("06:45".into());
        s.validate().unwrap();
    }

    #[test]
    fn validation_eq() {
        let mut s = Settings::default();
        s.playback.eq.enabled = true;
        s.playback.eq.bands[5].gain_db = 6.0;
        s.validate().unwrap();
        s.playback.eq.bands[5].gain_db = 21.0;
        assert!(s.validate().is_err());
        s.playback.eq.bands[5].gain_db = f32::NAN;
        assert!(s.validate().is_err());
        s.playback.eq.bands[5].gain_db = 0.0;
        s.playback.eq.bands[0].freq_hz = 10;
        assert!(s.validate().is_err());
        s.playback.eq.bands[0].freq_hz = 31;
        s.playback.eq.bands = (0..33)
            .map(|i| EqBand {
                freq_hz: 1000 + i as u32,
                gain_db: 0.0,
            })
            .collect();
        assert!(s.validate().is_err());
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
    fn validation_theme_et_language() {
        let mut s = Settings::default();
        assert_eq!(s.appearance.theme, "dark");
        assert_eq!(s.appearance.language, "fr");
        for ok in ["dark", "light", "system", "aurora", "mon-theme-2"] {
            s.appearance.theme = ok.into();
            s.validate().unwrap();
        }
        for bad in ["", "Aurora", "../etc", "thème", "a b", &"x".repeat(65)] {
            s.appearance.theme = bad.into();
            assert!(s.validate().is_err(), "{bad} devrait être refusé");
        }
        s.appearance.theme = "dark".into();
        for ok in ["fr", "en", "de", "pt-BR", "zh-CN"] {
            s.appearance.language = ok.into();
            s.validate().unwrap();
        }
        for bad in ["", "fr!", "french", "fr-FR-extra", "FR"] {
            s.appearance.language = bad.into();
            assert!(s.validate().is_err(), "{bad} devrait être refusé");
        }
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
        assert_eq!(partial.appearance.theme, "dark");
        assert_eq!(partial.playback.volume_mode, VolumeMode::Software);
    }
}
