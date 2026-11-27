//! Pont CamillaDSP (#82) — API de contrôle : statut live de l'instance,
//! sélection de profil, re-sonde. Les réglages eux-mêmes passent par
//! `PUT /api/settings` (section `camilla`), qui déclenche `engine.apply_camilla`.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use auris_core::{CamillaSettings, CamillaStatus};
use auris_player::camilla_probe;

use crate::{ApiError, ApiResult, AppState};

/// Réponse de `GET /api/camilla/status`.
#[derive(Serialize)]
pub struct CamillaStatusResponse {
    /// Réglages courants (section `camilla` de `/api/settings`).
    pub settings: CamillaSettings,
    /// Sonde live de l'instance (`None` si le pont est désactivé).
    pub live: Option<CamillaStatus>,
    /// Dernier statut appliqué par le moteur (délégation effective).
    pub engine: CamillaStatus,
    /// true = le moteur route actuellement le flux vers CamillaDSP.
    pub delegated: bool,
    /// Périphérique de sortie courant du moteur.
    pub device: Option<String>,
}

fn current_response(s: &AppState, live: Option<CamillaStatus>) -> CamillaStatusResponse {
    let settings = s.settings.read().unwrap().camilla.clone();
    CamillaStatusResponse {
        engine: s.engine.camilla_status(),
        delegated: s.engine.dsp_external(),
        device: s.engine.state().audio_device,
        live,
        settings,
    }
}

/// `GET /api/camilla/status` — sonde live + état appliqué par le moteur.
async fn get_status(State(s): State<AppState>) -> Json<CamillaStatusResponse> {
    let url = s.settings.read().unwrap().camilla.ws_url.clone();
    let enabled = s.settings.read().unwrap().camilla.enabled;
    let live = if enabled {
        Some(camilla_probe(url.trim()).await)
    } else {
        None
    };
    Json(current_response(&s, live))
}

#[derive(Deserialize)]
pub struct ProfileBody {
    pub name: String,
}

/// `POST /api/camilla/profile { "name": "salon.yml" }` — sélectionne le profil
/// actif côté CamillaDSP, le persiste dans les réglages et réapplique le pont.
async fn post_profile(
    State(s): State<AppState>,
    Json(b): Json<ProfileBody>,
) -> ApiResult<Json<CamillaStatusResponse>> {
    let name = b.name.trim().to_string();
    if name.is_empty() || name.len() > 128 {
        return Err(ApiError(auris_core::AurisError::other(
            "nom de profil invalide (1-128 caractères)",
        )));
    }
    {
        let mut w = s.settings.write().unwrap();
        w.camilla.profile = Some(name);
        if let Err(e) = w.validate() {
            return Err(ApiError(auris_core::AurisError::other(format!(
                "réglages invalides: {e}"
            ))));
        }
    }
    s.persist_settings();
    // réapplique le pont (sonde + SetConfigName + routage)
    let c = s.settings.read().unwrap().camilla.clone();
    s.engine.apply_camilla(&c);
    // attend que le moteur ait traité la commande (sonde + SetConfigName), ~3 s max
    let want = c.profile.clone();
    for _ in 0..30 {
        let eng = s.engine.camilla_status();
        if !eng.reachable || eng.active_profile == want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let live = if c.enabled {
        Some(camilla_probe(c.ws_url.trim()).await)
    } else {
        None
    };
    Ok(Json(current_response(&s, live)))
}

/// `POST /api/camilla/refresh` — re-sonde l'instance et réapplique le pont
/// (utile après un redémarrage de CamillaDSP).
async fn post_refresh(State(s): State<AppState>) -> Json<CamillaStatusResponse> {
    let c = s.settings.read().unwrap().camilla.clone();
    s.engine.apply_camilla(&c);
    let live = if c.enabled {
        Some(camilla_probe(c.ws_url.trim()).await)
    } else {
        None
    };
    Json(current_response(&s, live))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/camilla/status", get(get_status))
        .route("/api/camilla/profile", post(post_profile))
        .route("/api/camilla/refresh", post(post_refresh))
}
