//! Compatibilité socket.io 2.x (Engine.IO v3, EIO=3) — #45.
//!
//! Permet aux clients mobiles Volumio existants de piloter Auris :
//! handshake polling `GET /socket.io/?EIO=3&transport=polling`, long-poll,
//! POST des paquets, upgrade websocket, puis couche socket.io (paquets `4x`)
//! avec les événements Volumio usuels (`play`, `pause`, `setVolume`, …) et les
//! push `pushState` / `pushQueue`.
//!
//! En EIO=3 c'est le **client** qui ping ("2") et le serveur qui répond
//! pong ("3"). Les paquets polling sont préfixés `len:` et séparés par \x1e.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    body::Bytes,
    extract::{FromRequestParts, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

type Request = axum::extract::Request;
use tokio::sync::mpsc;

use auris_core::RepeatMode;

use crate::AppState;

struct Session {
    tx: mpsc::Sender<String>,
    /// Récepteur des paquets sortants, « emprunté » le temps d'un long-poll.
    rx: Mutex<Option<mpsc::Receiver<String>>>,
}

/// Hub de sessions Engine.IO (#45).
#[derive(Default)]
pub struct SocketIoHub {
    sessions: Mutex<HashMap<String, Session>>,
}

impl SocketIoHub {
    pub fn new() -> Arc<Self> {
        Arc::new(SocketIoHub::default())
    }

    /// Ouvre une session ; renvoie le sid.
    pub fn open(&self) -> String {
        let sid = format!("{:016x}{:016x}", rand_u64(), rand_u64());
        let (tx, rx) = mpsc::channel::<String>(256);
        self.sessions.lock().unwrap().insert(
            sid.clone(),
            Session {
                tx,
                rx: Mutex::new(Some(rx)),
            },
        );
        sid
    }

    pub fn close(&self, sid: &str) {
        self.sessions.lock().unwrap().remove(sid);
    }

    /// Envoie un paquet EIO brut à une session (noop si session fermée).
    pub fn send(&self, sid: &str, packet: &str) {
        if let Some(sess) = self.sessions.lock().unwrap().get(sid) {
            let _ = sess.tx.try_send(packet.to_string());
        }
    }

    /// Diffuse un paquet à toutes les sessions (pushState, pushQueue…).
    pub fn broadcast(&self, packet: &str) {
        for sess in self.sessions.lock().unwrap().values() {
            let _ = sess.tx.try_send(packet.to_string());
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// Emprunte le récepteur d'une session pour un long-poll, le restaure après.
    async fn poll(&self, sid: &str, wait: std::time::Duration) -> Vec<String> {
        let rx = self
            .sessions
            .lock()
            .unwrap()
            .get(sid)
            .and_then(|s| s.rx.lock().unwrap().take());
        let Some(mut rx) = rx else {
            return vec!["6".to_string()]; // noop
        };
        let mut packets = Vec::new();
        if let Ok(Some(p)) = tokio::time::timeout(wait, rx.recv()).await {
            packets.push(p);
        }
        while let Ok(p) = rx.try_recv() {
            packets.push(p);
        }
        if let Some(sess) = self.sessions.lock().unwrap().get(sid) {
            *sess.rx.lock().unwrap() = Some(rx);
        }
        if packets.is_empty() {
            packets.push("6".to_string()); // noop
        }
        packets
    }
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    let mut x = n | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// Encode un payload de polling EIO3 : `len:packet` séparés par \x1e.
/// EIO=3 : les paquets sont concaténés avec un préfixe `len:` (pas de séparateur).
fn encode_payload(packets: &[String]) -> String {
    packets
        .iter()
        .map(|p| format!("{}:{}", p.chars().count(), p))
        .collect::<Vec<_>>()
        .join("")
}

/// EIO=3 : payload = concaténation de `len:paquet` (tolère aussi le séparateur \x1e).
fn decode_payload(payload: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = payload.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1e' {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] != ':' && chars[i] != '\x1e' {
            i += 1;
        }
        if i >= chars.len() || chars[i] == '\x1e' {
            // pas de préfixe : paquet brut (ex. "2" sur websocket)
            if start < i {
                out.push(chars[start..i].iter().collect());
            }
            continue;
        }
        let len: usize = chars[start..i]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        i += 1; // saute ':'
        let end = (i + len).min(chars.len());
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

const PING_INTERVAL_MS: u64 = 25000;
const PING_TIMEOUT_MS: u64 = 60000;

/// Entrée unique GET /socket.io/ : websocket si upgrade demandé, sinon polling.
pub async fn socketio_get(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _body) = req.into_parts();
    let q = match Query::<HashMap<String, String>>::from_request_parts(&mut parts, &s).await {
        Ok(q) => q.0,
        Err(e) => return e.into_response(),
    };
    let wants_ws = parts
        .headers
        .get(header::UPGRADE)
        .map(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
        .unwrap_or(false);
    if wants_ws && q.get("transport").map(|t| t.as_str()) == Some("websocket") {
        match axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &s).await {
            Ok(ws) => {
                let state = s.clone();
                return ws
                    .on_upgrade(move |socket| ws_session(socket, state, q))
                    .into_response();
            }
            Err(e) => return e.into_response(),
        }
    }
    // polling
    match q.get("sid") {
        None => {
            // handshake
            let sid = s.socketio.open();
            let open = serde_json::json!({
                "sid": sid,
                "upgrades": ["websocket"],
                "pingInterval": PING_INTERVAL_MS,
                "pingTimeout": PING_TIMEOUT_MS,
            });
            // socket.io 2.x : le serveur connecte proactivement le namespace '/'
            s.socketio
                .send(&sid, &format!("40{}", serde_json::json!({ "sid": sid })));
            push_state(&s, &sid);
            push_queue(&s, &sid);
            payload_response(vec![format!("0{}", open)])
        }
        Some(sid) => {
            let packets = s
                .socketio
                .poll(sid, std::time::Duration::from_millis(PING_INTERVAL_MS))
                .await;
            payload_response(packets)
        }
    }
}

fn payload_response(packets: Vec<String>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
        encode_payload(&packets),
    )
        .into_response()
}

/// POST polling : paquets entrants du client.
pub async fn socketio_post(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let sid = q.get("sid").cloned().unwrap_or_default();
    let text = String::from_utf8_lossy(&body);
    for packet in decode_payload(&text) {
        handle_eio_packet(&s, &sid, &packet);
    }
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain")], "ok").into_response()
}

/// Traite un paquet EIO brut entrant (polling ou ws).
fn handle_eio_packet(s: &AppState, sid: &str, packet: &str) {
    if packet.is_empty() {
        return;
    }
    let (kind, rest) = packet.split_at(1);
    match kind {
        "2" => s.socketio.send(sid, "3"), // ping client -> pong
        "4" => handle_sio_packet(s, sid, rest),
        "1" => s.socketio.close(sid),
        _ => {}
    }
}

/// Paquet socket.io (après le "4" EIO) : 0 connect, 1 disconnect, 2 event.
fn handle_sio_packet(s: &AppState, sid: &str, packet: &str) {
    if packet.is_empty() {
        return;
    }
    let (t, rest) = packet.split_at(1);
    match t {
        "0" => {
            s.socketio
                .send(sid, &format!("40{}", serde_json::json!({ "sid": sid })));
            push_state(s, sid);
            push_queue(s, sid);
        }
        "1" => s.socketio.close(sid),
        "2" => {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(rest) {
                if let Some(name) = arr.first().and_then(|v| v.as_str()) {
                    dispatch(s, sid, name, &arr[1..]);
                }
            }
        }
        _ => {}
    }
}

fn push_state(s: &AppState, sid: &str) {
    if let Ok(json) = serde_json::to_value(s.engine.state()) {
        s.socketio.send(
            sid,
            &format!("42{}", serde_json::json!(["pushState", json])),
        );
    }
}

fn push_queue(s: &AppState, sid: &str) {
    let q = s.engine.state().queue;
    if let Ok(json) = serde_json::to_value(&q) {
        s.socketio.send(
            sid,
            &format!("42{}", serde_json::json!(["pushQueue", json])),
        );
    }
}

/// Diffuse pushState à toutes les sessions (appelé sur chaque événement moteur).
pub fn broadcast_state(s: &AppState) {
    if let Ok(json) = serde_json::to_value(s.engine.state()) {
        s.socketio
            .broadcast(&format!("42{}", serde_json::json!(["pushState", json])));
    }
}

/// Commandes socket.io style Volumio (v1.7) -> moteur.
fn dispatch(s: &AppState, sid: &str, name: &str, args: &[serde_json::Value]) {
    let e = &s.engine;
    match name {
        "getState" => push_state(s, sid),
        "getQueue" => push_queue(s, sid),
        "play" => {
            if let Some(uri) = args
                .first()
                .and_then(|a| a.get("uri").and_then(|u| u.as_str()))
            {
                e.play_uri(uri.to_string());
            } else if let Some(idx) = args.first().and_then(|a| a.as_u64()) {
                e.play_queue_index(idx as usize);
            } else {
                e.resume();
            }
        }
        "pause" => e.pause(),
        "resume" => e.resume(),
        "stop" => e.stop(),
        "next" | "nexttrack" => e.next(),
        "previous" | "previoustrack" => e.prev(),
        "seek" => {
            if let Some(v) = args.first().and_then(|a| a.as_f64()) {
                let secs = if v > 10000.0 { v / 1000.0 } else { v }; // tolère les ms
                e.seek(secs);
            }
        }
        "setVolume" | "volume" => {
            if let Some(v) = args.first().and_then(|a| a.as_f64()) {
                e.set_volume(v.clamp(0.0, 100.0) as u8);
            }
        }
        "mute" => e.set_muted(args.first().and_then(|a| a.as_bool()).unwrap_or(true)),
        "unmute" => e.set_muted(false),
        "repeat" => {
            let mode = match args.first().and_then(|a| a.as_str()).unwrap_or("") {
                "all" | "true" | "1" => RepeatMode::All,
                "one" => RepeatMode::One,
                _ => RepeatMode::Off,
            };
            e.set_repeat(mode);
        }
        "shuffle" => e.set_shuffle(args.first().and_then(|a| a.as_bool()).unwrap_or(false)),
        "clearQueue" => e.clear_queue(),
        "playQueueIndex" => {
            if let Some(i) = args.first().and_then(|a| a.as_u64()) {
                e.play_queue_index(i as usize);
            }
        }
        "removeFromQueue" => {
            if let Some(i) = args.first().and_then(|a| a.as_u64()) {
                e.remove_queue_item(i as usize);
            }
        }
        "addToQueue" => {
            if let Some(uri) = args
                .first()
                .and_then(|a| a.get("uri").and_then(|u| u.as_str()))
            {
                if let Some(t) = s.library.get_by_uri(uri) {
                    e.enqueue(t);
                }
            }
        }
        _ => tracing::debug!("socket.io: événement ignoré: {name}"),
    }
}

/// Session websocket Engine.IO : paquets bruts par frame.
async fn ws_session(socket: axum::extract::ws::WebSocket, s: AppState, q: HashMap<String, String>) {
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, mut rx) = socket.split();
    // upgrade polling->ws : le client fournit son sid existant
    let sid = match q.get("sid") {
        Some(existing) if !existing.is_empty() => existing.clone(),
        _ => s.socketio.open(),
    };
    let open = serde_json::json!({
        "sid": sid,
        "upgrades": [],
        "pingInterval": PING_INTERVAL_MS,
        "pingTimeout": PING_TIMEOUT_MS,
    });
    if tx
        .send(axum::extract::ws::Message::Text(
            format!("0{}", open).into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    // socket.io connect + état courant
    s.socketio
        .send(&sid, &format!("40{}", serde_json::json!({ "sid": sid })));
    push_state(&s, &sid);
    push_queue(&s, &sid);

    // pompe les paquets sortants de la session vers le ws
    let out = {
        let mut sessions = s.socketio.sessions.lock().unwrap();
        match sessions.get_mut(&sid) {
            Some(sess) => sess.rx.lock().unwrap().take(),
            None => None,
        }
    };
    let Some(mut out_rx) = out else {
        return;
    };
    let send_task = tokio::spawn(async move {
        while let Some(p) = out_rx.recv().await {
            if tx
                .send(axum::extract::ws::Message::Text(p.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    while let Some(msg) = rx.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(t)) => {
                for packet in t.split('\x1e') {
                    if !packet.is_empty() {
                        handle_eio_packet(&s, &sid, packet);
                    }
                }
            }
            Ok(axum::extract::ws::Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    s.socketio.close(&sid);
    send_task.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_roundtrip() {
        let packets = vec![
            "0{\"sid\":\"abc\"}".to_string(),
            "40".to_string(),
            "42[\"pushState\",{}]".to_string(),
        ];
        let enc = encode_payload(&packets);
        let dec = decode_payload(&enc);
        assert_eq!(dec, packets);
        // concaténation EIO=3 sans séparateur
        assert!(!enc.contains('\x1e'));
    }

    #[test]
    fn payload_sans_prefix() {
        assert_eq!(decode_payload("2"), vec!["2".to_string()]);
        assert_eq!(
            decode_payload("1:21:4"),
            vec!["2".to_string(), "4".to_string()]
        );
        assert_eq!(
            decode_payload("2\x1e4"),
            vec!["2".to_string(), "4".to_string()]
        );
    }

    #[test]
    fn hub_open_send_close() {
        let hub = SocketIoHub::default();
        let sid = hub.open();
        assert_eq!(hub.session_count(), 1);
        hub.send(&sid, "3");
        hub.broadcast("42[]");
        hub.close(&sid);
        assert_eq!(hub.session_count(), 0);
    }
}
