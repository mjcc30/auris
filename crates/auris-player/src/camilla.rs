//! Pont CamillaDSP (#82) : sonde et pilote une instance CamillaDSP via son
//! websocket (protocole JSON, port 1234 par défaut — le même que pycamilladsp) :
//! `GetVersion`, `GetStatus`, `GetConfigName`, `GetConfigNames`, `SetConfigName`.
//!
//! La délégation du traitement audio elle-même se fait côté sortie (loopback /
//! FIFO, voir `engine.rs`) : ici on ne fait que contrôler l'instance (sonde +
//! sélection de profil), avec des fonctions de parsing pures et testables.

use std::future::Future;
use std::time::Duration;

use auris_core::{CamillaMode, CamillaSettings, CamillaStatus};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

/// Délai d'attente d'une requête websocket (sonde / sélection de profil).
pub const RPC_TIMEOUT: Duration = Duration::from_secs(3);

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(ws_url: &str) -> Result<Ws, String> {
    tokio::time::timeout(RPC_TIMEOUT, connect_async(ws_url))
        .await
        .map_err(|_| format!("délai dépassé ({}s) pour {ws_url}", RPC_TIMEOUT.as_secs()))?
        .map(|(ws, _)| ws)
        .map_err(|e| format!("connexion {ws_url}: {e}"))
}

/// Envoie `{"<key>": payload}` et attend la réponse `{"<key>": …}` (les autres
/// messages — notifications — sont ignorés). `{"Error": …}` -> Err.
async fn rpc(ws: &mut Ws, key: &str, payload: Value) -> Result<Value, String> {
    ws.send(Message::Text(json!({ key: payload }).to_string().into()))
        .await
        .map_err(|e| format!("envoi {key}: {e}"))?;
    loop {
        let msg = tokio::time::timeout(RPC_TIMEOUT, ws.next())
            .await
            .map_err(|_| format!("délai dépassé en attente de la réponse à {key}"))?
            .ok_or_else(|| "connexion fermée par CamillaDSP".to_string())?
            .map_err(|e| format!("réception {key}: {e}"))?;
        match msg {
            Message::Text(t) => {
                let v: Value =
                    serde_json::from_str(&t).map_err(|e| format!("JSON invalide: {e}"))?;
                if let Some(err) = v.get("Error") {
                    return Err(err.as_str().unwrap_or("erreur CamillaDSP").to_string());
                }
                if let Some(r) = v.get(key) {
                    return Ok(r.clone());
                }
                // autres messages (notifications) : on continue d'attendre
            }
            Message::Close(_) => return Err("connexion fermée par CamillaDSP".into()),
            _ => {}
        }
    }
}

// ---------- Parsing pur (testable sans réseau) ----------

/// `{"GetVersion": "2.0.5"}` -> `Some("2.0.5")`.
pub fn parse_version(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// `{"GetStatus": {"state": "Running", …}}` -> `Some("Running")`.
pub fn parse_state(v: &Value) -> Option<String> {
    v.get("state")?.as_str().map(str::to_string)
}

/// `{"GetConfigName": "salon.yml"}` -> `Some("salon.yml")` ; `null` -> None.
pub fn parse_config_name(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// `{"GetConfigNames": ["a.yml", "b.yml"]}` -> liste des profils.
pub fn parse_config_names(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------- API async ----------

async fn probe_inner(ws_url: &str) -> Result<CamillaStatus, String> {
    let mut ws = connect(ws_url).await?;
    let version = rpc(&mut ws, "GetVersion", Value::Null)
        .await
        .ok()
        .and_then(|v| parse_version(&v));
    let state = rpc(&mut ws, "GetStatus", Value::Null)
        .await
        .ok()
        .and_then(|v| parse_state(&v));
    let active_profile = rpc(&mut ws, "GetConfigName", Value::Null)
        .await
        .ok()
        .and_then(|v| parse_config_name(&v));
    let profiles = rpc(&mut ws, "GetConfigNames", Value::Null)
        .await
        .map(|v| parse_config_names(&v))
        .unwrap_or_default();
    Ok(CamillaStatus {
        reachable: true,
        version,
        state,
        active_profile,
        profiles,
        error: None,
    })
}

/// Sonde une instance CamillaDSP. Ne renvoie jamais d'erreur : `reachable =
/// false` + `error` si injoignable (utile pour le fallback DSP interne).
pub async fn probe(ws_url: &str) -> CamillaStatus {
    match probe_inner(ws_url).await {
        Ok(st) => st,
        Err(e) => CamillaStatus {
            reachable: false,
            error: Some(e),
            ..Default::default()
        },
    }
}

/// Active un profil (`SetConfigName`) puis vérifie avec `GetConfigName`.
pub async fn select_profile(ws_url: &str, name: &str) -> Result<(), String> {
    let mut ws = connect(ws_url).await?;
    ws.send(Message::Text(
        json!({ "SetConfigName": name }).to_string().into(),
    ))
    .await
    .map_err(|e| format!("envoi SetConfigName: {e}"))?;
    // la réponse attendue est {"SetConfigNameDone": name} (ou {"Error": …})
    loop {
        let msg = tokio::time::timeout(RPC_TIMEOUT, ws.next())
            .await
            .map_err(|_| "délai dépassé en attente de SetConfigNameDone".to_string())?
            .ok_or_else(|| "connexion fermée par CamillaDSP".to_string())?
            .map_err(|e| format!("réception SetConfigName: {e}"))?;
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).map_err(|e| format!("JSON invalide: {e}"))?;
            if let Some(err) = v.get("Error") {
                return Err(err.as_str().unwrap_or("erreur CamillaDSP").to_string());
            }
            if v.get("SetConfigNameDone").is_some() {
                break;
            }
        }
    }
    let cur = rpc(&mut ws, "GetConfigName", Value::Null).await?;
    match parse_config_name(&cur) {
        Some(c) if c == name => Ok(()),
        other => Err(format!(
            "profil non appliqué: actif = {other:?}, demandé = {name:?}"
        )),
    }
}

/// Choix de sortie résultant de l'application des réglages CamillaDSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputChoice {
    /// Délégation : router le flux vers `target` (périphérique de capture en
    /// mode loopback, FIFO en mode pipe) — le DSP interne est court-circuité.
    Delegate { target: String },
    /// DSP interne + sortie courante (désactivé, mode websocket, ou repli).
    Internal,
}

/// Décision pure de routage : délègue seulement si activé, mode audio
/// (loopback/pipe) et instance joignable.
pub fn resolve_output(c: &CamillaSettings, reachable: bool) -> OutputChoice {
    if !c.enabled || c.mode == CamillaMode::Websocket || !reachable {
        OutputChoice::Internal
    } else {
        OutputChoice::Delegate {
            target: c.capture_device.trim().to_string(),
        }
    }
}

// ---------- Wrappers bloquants (le moteur tourne dans un thread dédié) ----------

fn run_blocking<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime tokio current_thread")
        .block_on(f)
}

/// Sonde bloquante (à appeler hors runtime tokio, ex. thread moteur).
pub fn probe_blocking(ws_url: &str) -> CamillaStatus {
    run_blocking(probe(ws_url))
}

/// Sélection de profil bloquante.
pub fn select_profile_blocking(ws_url: &str, name: &str) -> Result<(), String> {
    run_blocking(select_profile(ws_url, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_helpers() {
        assert_eq!(parse_version(&json!("2.0.5")), Some("2.0.5".into()));
        assert_eq!(parse_version(&json!(null)), None);
        assert_eq!(
            parse_state(&json!({"state": "Running", "capturerate": 48000})),
            Some("Running".into())
        );
        assert_eq!(parse_state(&json!({})), None);
        assert_eq!(
            parse_config_name(&json!("salon.yml")),
            Some("salon.yml".into())
        );
        assert_eq!(parse_config_name(&json!(null)), None);
        assert_eq!(
            parse_config_names(&json!(["a.yml", "b.yml"])),
            vec!["a.yml".to_string(), "b.yml".to_string()]
        );
        assert!(parse_config_names(&json!({})).is_empty());
        assert!(parse_config_names(&json!(["a", 3, null])).len() == 1);
    }

    #[test]
    fn resolve_output_decisions() {
        let mut c = CamillaSettings::default();
        // désactivé -> interne, même joignable
        assert_eq!(resolve_output(&c, true), OutputChoice::Internal);
        c.enabled = true;
        // mode websocket -> jamais de reroutage audio
        c.mode = CamillaMode::Websocket;
        assert_eq!(resolve_output(&c, true), OutputChoice::Internal);
        // loopback + joignable -> délégation vers le périphérique de capture
        c.mode = CamillaMode::Loopback;
        c.capture_device = "camilladsp".into();
        assert_eq!(
            resolve_output(&c, true),
            OutputChoice::Delegate {
                target: "camilladsp".into()
            }
        );
        // pipe + joignable -> délégation vers le FIFO
        c.mode = CamillaMode::Pipe;
        c.capture_device = "/tmp/camilla.fifo".into();
        assert_eq!(
            resolve_output(&c, true),
            OutputChoice::Delegate {
                target: "/tmp/camilla.fifo".into()
            }
        );
        // injoignable -> repli interne
        assert_eq!(resolve_output(&c, false), OutputChoice::Internal);
    }

    /// Petit serveur websocket mock façon CamillaDSP : répond selon la clé de
    /// requête, comme le vrai (clé de réponse = clé de requête).
    async fn mock_camilla(listener: tokio::net::TcpListener) {
        use tokio_tungstenite::accept_async;
        // accepte plusieurs connexions (probe + select_profile = 2 clients)
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut ws = accept_async(stream).await.unwrap();
                while let Some(Ok(Message::Text(t))) = ws.next().await {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    let resp = if v.get("GetVersion").is_some() {
                        json!({ "GetVersion": "2.0.5" })
                    } else if v.get("GetStatus").is_some() {
                        json!({ "GetStatus": { "state": "Running", "capturerate": 48000 } })
                    } else if v.get("GetConfigNames").is_some() {
                        json!({ "GetConfigNames": ["default.yml", "salon.yml"] })
                    } else if let Some(name) = v.get("SetConfigName").and_then(|x| x.as_str()) {
                        if name == "inconnu.yml" {
                            json!({ "Error": "config not found" })
                        } else {
                            json!({ "SetConfigNameDone": name })
                        }
                    } else if v.get("GetConfigName").is_some() {
                        // le mock retient le dernier profil sélectionné
                        json!({ "GetConfigName": "salon.yml" })
                    } else {
                        json!({ "Error": "unknown" })
                    };
                    if ws
                        .send(Message::Text(resp.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    }

    #[tokio::test]
    async fn probe_et_select_profile_contre_mock() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(mock_camilla(listener));
        let url = format!("ws://{addr}");

        let st = probe(&url).await;
        assert!(st.reachable, "sonde: {:?}", st.error);
        assert_eq!(st.version.as_deref(), Some("2.0.5"));
        assert_eq!(st.state.as_deref(), Some("Running"));
        assert_eq!(st.active_profile.as_deref(), Some("salon.yml"));
        assert_eq!(
            st.profiles,
            vec!["default.yml".to_string(), "salon.yml".to_string()]
        );

        // SetConfigName + vérification GetConfigName (le mock renvoie salon.yml)
        select_profile(&url, "salon.yml").await.unwrap();
    }

    #[tokio::test]
    async fn select_profile_erreur_camilla() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(mock_camilla(listener));
        let url = format!("ws://{addr}");
        let err = select_profile(&url, "inconnu.yml").await.unwrap_err();
        assert!(err.contains("config not found"), "{err}");
    }

    #[tokio::test]
    async fn probe_injoignable() {
        // port fermé : reachable = false, pas de panique
        let st = probe("ws://127.0.0.1:1").await;
        assert!(!st.reachable);
        assert!(st.error.is_some());
    }
}
