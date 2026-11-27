//! `GET /api/signal-path` — chaîne de traitement du flux en cours, façon
//! « Signal Path » de Roon (#77) : source → format natif → bit-perfect /
//! resampling → DSD → loudness → EQ → crossfade/gapless → volume → sortie.
//! L'UI affiche ces étages dans un badge de la barre de lecture.

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use auris_core::{AudioFormat, DsdMode, EqBand, OutputMode, VolumeCurve, VolumeMode};

use crate::AppState;

#[derive(Serialize)]
pub struct SignalPath {
    /// « radio », id de source `src://…`, ou « local ».
    pub service: String,
    pub uri: String,
    pub title: String,
    pub live: bool,
    pub stream_title: Option<String>,
    /// Format natif du flux en cours.
    pub format_kind: String,
    pub sample_rate: Option<u32>,
    pub bits: Option<u8>,
    pub channels: Option<u8>,
    /// true = rééchantillonnage autorisé (false = bit-perfect).
    pub resampling: bool,
    pub dsd_mode: DsdMode,
    /// WASAPI exclusif actif (Windows, bit-perfect vers le DAC).
    pub exclusive: bool,
    pub output_backend: String,
    pub output_mode: OutputMode,
    pub device: Option<String>,
    pub volume_mode: VolumeMode,
    pub volume_curve: VolumeCurve,
    pub volume: u8,
    pub max_volume: u8,
    pub muted: bool,
    pub loudness: bool,
    pub loudness_target_lufs: f64,
    /// Gain appliqué au flux courant (dB), si mesuré (#37).
    pub loudness_gain_db: Option<f32>,
    pub eq_enabled: bool,
    /// Bandes non plates uniquement.
    pub eq_bands: Vec<EqBand>,
    /// Étape DSP : « CamillaDSP (<profil>) » si le traitement est délégué (#82),
    /// sinon « interne » (EQ + loudness maison).
    pub dsp: String,
    pub crossfade_secs: f64,
    pub gapless: bool,
}

async fn get_signal_path(State(s): State<AppState>) -> Json<SignalPath> {
    let st = s.engine.state();
    let pb = s.settings.read().unwrap().playback.clone();
    let cur = st.current;
    let uri = cur.as_ref().map(|t| t.uri.clone()).unwrap_or_default();
    let service = match auris_sources::resolve_uri(&uri) {
        Ok(auris_sources::UriRef::Src { source_id, .. }) => source_id,
        _ if auris_sources::is_http_uri(&uri) => "radio".to_string(),
        _ => "local".to_string(),
    };
    let (format_kind, sample_rate, bits, channels) = match st.current_format {
        Some(AudioFormat::Pcm {
            sample_rate,
            bits,
            channels,
        }) => (
            "pcm".to_string(),
            Some(sample_rate),
            Some(bits),
            Some(channels),
        ),
        Some(AudioFormat::Dsd {
            sample_rate,
            channels,
        }) => ("dsd".to_string(), Some(sample_rate), None, Some(channels)),
        None => (String::new(), None, None, None),
    };
    let gain_db = if pb.loudness {
        s.engine.loudness_gain(&uri).map(|g| 20.0 * g.log10())
    } else {
        None
    };
    let eq_bands = pb
        .eq
        .bands
        .iter()
        .copied()
        .filter(|b| b.gain_db != 0.0)
        .collect();
    Json(SignalPath {
        service,
        title: cur.as_ref().map(|t| t.title.clone()).unwrap_or_default(),
        uri,
        live: st.is_live,
        stream_title: st.stream_title.clone(),
        format_kind,
        sample_rate,
        bits,
        channels,
        resampling: pb.resampling,
        dsd_mode: pb.dsd_mode,
        exclusive: s.engine.exclusive_enabled(),
        output_backend: st.output_backend.clone(),
        output_mode: st.output_mode,
        device: st.audio_device.clone(),
        volume_mode: pb.volume_mode,
        volume_curve: pb.volume_curve,
        volume: st.volume,
        max_volume: st.max_volume,
        muted: st.muted,
        loudness: pb.loudness,
        loudness_target_lufs: pb.loudness_target_lufs,
        loudness_gain_db: gain_db,
        eq_enabled: pb.eq.enabled && !s.engine.dsp_external(),
        eq_bands,
        dsp: {
            let cst = s.engine.camilla_status();
            if s.engine.dsp_external() {
                match cst.active_profile {
                    Some(p) => format!("CamillaDSP ({p})"),
                    None => "CamillaDSP".to_string(),
                }
            } else {
                "interne".to_string()
            }
        },
        crossfade_secs: pb.crossfade_secs,
        gapless: pb.gapless,
    })
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/signal-path", get(get_signal_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_path_serialise() {
        let sp = SignalPath {
            service: "qobuz".into(),
            uri: "src://qobuz/track/101".into(),
            title: "So What".into(),
            live: false,
            stream_title: None,
            format_kind: "pcm".into(),
            sample_rate: Some(44100),
            bits: Some(16),
            channels: Some(2),
            resampling: false,
            dsd_mode: DsdMode::Dop,
            exclusive: true,
            output_backend: "wasapi".into(),
            output_mode: OutputMode::Local,
            device: Some("DAC".into()),
            volume_mode: VolumeMode::Software,
            volume_curve: VolumeCurve::Logarithmic,
            volume: 80,
            max_volume: 100,
            muted: false,
            loudness: true,
            loudness_target_lufs: -23.0,
            loudness_gain_db: Some(-3.5),
            eq_enabled: true,
            eq_bands: vec![EqBand {
                freq_hz: 62,
                gain_db: 2.0,
            }],
            dsp: "interne".into(),
            crossfade_secs: 0.0,
            gapless: true,
        };
        let v = serde_json::to_value(&sp).unwrap();
        assert_eq!(v["service"], "qobuz");
        assert_eq!(v["format_kind"], "pcm");
        assert_eq!(v["bits"], 16);
        assert_eq!(v["eq_bands"][0]["freq_hz"], 62);
        assert_eq!(v["resampling"], false);
    }
}
