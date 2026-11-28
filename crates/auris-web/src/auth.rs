//! Token d'accès optionnel (#97) — style Navidrome : si `system.access_token`
//! est renseigné dans config.toml, toute l'API (et les websockets) exigent une
//! authentification. Les pages publiques (liens de partage `/share/{token}`,
//! DLNA) restent accessibles : leur token est dans l'URL.
//!
//! Un client s'authentifie soit par cookie de session (UI web, posé par
//! `POST /api/auth/login`), soit par `Authorization: Bearer <token>`, soit par
//! `?access_token=<token>` (websocket navigateur / clients socket.io).

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::AppState;

/// Valeur renvoyée par GET /api/settings à la place du vrai token (#97).
pub const TOKEN_MASK: &str = "********";

/// Durée de vie d'une session de login (30 jours).
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;

#[derive(Serialize)]
pub struct AuthStatus {
    /// Un token d'accès est configuré : l'API est protégée.
    pub required: bool,
    /// La requête courante est authentifiée.
    pub authenticated: bool,
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

/// Routes publiques d'authentification.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/status", get(get_status))
        .route("/api/auth/login", post(post_login))
        .route("/api/auth/logout", post(post_logout))
}

/// Token configuré (None = accès libre, comportement par défaut).
fn configured_token(s: &AppState) -> Option<String> {
    s.settings
        .read()
        .unwrap()
        .system
        .access_token
        .clone()
        .filter(|t| !t.trim().is_empty())
}

/// Comparaison en temps constant.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn session_valid(s: &AppState, sid: &str) -> bool {
    let mut w = s.sessions.write().unwrap();
    let now = chrono::Utc::now().timestamp();
    // purge des sessions expirées au passage
    w.retain(|_, exp| *exp > now);
    matches!(w.get(sid), Some(exp) if *exp > now)
}

fn create_session(s: &AppState) -> String {
    let sid = uuid::Uuid::new_v4().to_string().replace('-', "");
    let exp = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
    s.sessions.write().unwrap().insert(sid.clone(), exp);
    sid
}

/// Extrait le token présenté par la requête : header Bearer, cookie de session,
/// ou query `access_token` (websocket navigateur).
fn presented_token(s: &AppState, req: &Request) -> bool {
    let Some(want) = configured_token(s) else {
        return true; // pas de token configuré : tout est ouvert
    };
    // 1. Authorization: Bearer <token>
    if let Some(v) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return constant_time_eq(t.trim(), &want);
        }
    }
    // 2. cookie auris_session=<sid>
    if let Some(c) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
    {
        for part in c.split(';') {
            let part = part.trim();
            if let Some(sid) = part.strip_prefix("auris_session=") {
                if session_valid(s, sid) {
                    return true;
                }
            }
        }
    }
    // 3. query ?access_token=<token> (WebSocket navigateur, clients socket.io)
    if let Some(q) = req.uri().query() {
        for kv in q.split('&') {
            if let Some(v) = kv.strip_prefix("access_token=") {
                let v = v.trim();
                if !v.is_empty() && constant_time_eq(v, &want) {
                    return true;
                }
            }
        }
    }
    false
}

/// Middleware : 401 JSON si un token est configuré et la requête n'est pas authentifiée.
pub async fn require_auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    if presented_token(&s, &req) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "auth required" })),
        )
            .into_response()
    }
}

async fn get_status(State(s): State<AppState>, req: Request) -> Json<AuthStatus> {
    let required = configured_token(&s).is_some();
    let authenticated = !required || presented_token(&s, &req);
    Json(AuthStatus {
        required,
        authenticated,
    })
}

async fn post_login(State(s): State<AppState>, Json(b): Json<LoginBody>) -> Response {
    match configured_token(&s) {
        None => Json(serde_json::json!({ "ok": true, "required": false })).into_response(),
        Some(want) => {
            if constant_time_eq(b.token.trim(), &want) {
                let sid = create_session(&s);
                (
                    StatusCode::OK,
                    [(
                        header::SET_COOKIE,
                        format!(
                            "auris_session={sid}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_TTL_SECS}"
                        ),
                    )],
                    Json(serde_json::json!({ "ok": true, "required": true })),
                )
                    .into_response()
            } else {
                // ralentit les essais par force brute
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "token invalide" })),
                )
                    .into_response()
            }
        }
    }
}

async fn post_logout(State(s): State<AppState>, req: Request) -> Response {
    if let Some(c) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
    {
        for part in c.split(';') {
            if let Some(sid) = part.trim().strip_prefix("auris_session=") {
                s.sessions.write().unwrap().remove(sid);
            }
        }
    }
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            "auris_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".to_string(),
        )],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

/// Applique le masque/« inchangé » sur `access_token` avant persistance (#97) :
/// `"********"` = garder l'ancien, `""`/None = supprimer, sinon nouvelle valeur.
pub fn normalize_token_in(
    mut new: auris_core::Settings,
    old: Option<String>,
) -> auris_core::Settings {
    match new.system.access_token.as_deref().map(str::trim) {
        Some(TOKEN_MASK) => new.system.access_token = old,
        Some("") => new.system.access_token = None,
        Some(t) => new.system.access_token = Some(t.to_string()),
        None => new.system.access_token = None,
    }
    new
}

/// Masque le token dans les réglages exposés à l'UI.
pub fn mask_token_out(mut s: auris_core::Settings) -> auris_core::Settings {
    if s.system
        .access_token
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        s.system.access_token = Some(TOKEN_MASK.to_string());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc123", "abc1234"));
    }
}
