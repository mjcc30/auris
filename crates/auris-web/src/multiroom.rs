//! Multiroom / « Regroupement » — #23.
//!
//! - Découverte des instances Auris sur le LAN : balise JSON en multicast UDP
//!   (239.255.255.251:3011) toutes les 20 s, cache des pairs (TTL 60 s).
//! - Groupes nommés persistés dans `<data-dir>/multiroom.json`.
//! - Lecture synchronisée façon Snapcast allégé : l'initiateur (horloge
//!   maître) envoie à chaque pair un `start_at_ms` exprimé dans l'horloge du
//!   pair (offset estimé via `/api/multiroom/clock`) ; chaque pair démarre à
//!   l'heure dite puis se cale en position (seek).
//! - Volume par zone (pair) et par groupe.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::AppState;

const MCAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 251);
const MCAST_PORT: u16 = 3011;
const BEACON_PERIOD: Duration = Duration::from_secs(20);
const PEER_TTL: Duration = Duration::from_secs(75);
/// Marge de synchro ajoutée au start_at (buffer réseau + traitement).
const SYNC_DELAY_MS: u64 = 1500;

// ---------- modèles ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    /// URL de base de l'API du pair (`http://<ip>:<port>`).
    pub base: String,
    pub version: String,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// ids de pairs membres (l'id local = « cet appareil »).
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiroomState {
    pub self_id: String,
    pub self_name: String,
    pub peers: Vec<PeerInfo>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Default)]
pub struct MultiroomHub {
    pub self_id: String,
    pub peers: Mutex<HashMap<String, (PeerInfo, Instant)>>,
    groups_path: PathBuf,
    groups: Mutex<Vec<Group>>,
}

impl MultiroomHub {
    pub fn new(data_dir: PathBuf, self_id: String) -> Self {
        let groups_path = data_dir.join("multiroom.json");
        let groups = load_groups(&groups_path).unwrap_or_default();
        MultiroomHub {
            self_id,
            peers: Mutex::new(HashMap::new()),
            groups_path,
            groups: Mutex::new(groups),
        }
    }

    pub fn upsert_peer(&self, p: PeerInfo) {
        self.peers
            .lock()
            .unwrap()
            .insert(p.id.clone(), (p, Instant::now()));
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        let cutoff = Instant::now() - PEER_TTL;
        let mut m = self.peers.lock().unwrap();
        m.retain(|_, (_, seen)| *seen > cutoff);
        let mut v: Vec<PeerInfo> = m.values().map(|(p, _)| p.clone()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn groups(&self) -> Vec<Group> {
        self.groups.lock().unwrap().clone()
    }

    fn save_groups(&self) {
        let g = self.groups.lock().unwrap().clone();
        let _ = save_groups(&self.groups_path, &g);
    }

    pub fn create_group(&self, name: &str) -> Group {
        let g = Group {
            id: new_id("grp", name),
            name: name.trim().to_string(),
            members: vec![self.self_id.clone()],
        };
        self.groups.lock().unwrap().push(g.clone());
        self.save_groups();
        g
    }

    pub fn delete_group(&self, id: &str) -> bool {
        let mut g = self.groups.lock().unwrap();
        let n = g.len();
        g.retain(|x| x.id != id);
        let removed = g.len() != n;
        drop(g);
        if removed {
            self.save_groups();
        }
        removed
    }

    pub fn set_member(&self, group_id: &str, member: &str, add: bool) -> Option<Group> {
        let mut g = self.groups.lock().unwrap();
        let grp = g.iter_mut().find(|x| x.id == group_id)?;
        if add {
            if !grp.members.contains(&member.to_string()) {
                grp.members.push(member.to_string());
            }
        } else {
            grp.members.retain(|m| m != member);
        }
        let out = grp.clone();
        drop(g);
        self.save_groups();
        Some(out)
    }
}

// ---------- persistance ----------

fn load_groups(path: &PathBuf) -> Option<Vec<Group>> {
    let txt = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&txt).ok()
}

fn save_groups(path: &PathBuf, groups: &[Group]) -> std::io::Result<()> {
    let txt = serde_json::to_string_pretty(groups).unwrap_or_else(|_| "[]".into());
    std::fs::write(path, txt)
}

/// id stable : md5 du nom + timestamp, formaté en UUID-like.
pub fn new_id(prefix: &str, seed: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut m = Md5::new();
    m.update(format!("{prefix}|{seed}|{nanos}"));
    let h = format!("{:x}", m.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// id de pair persistant (`<data-dir>/peer_id`), créé à la première utilisation.
pub fn peer_id(data_dir: &std::path::Path) -> String {
    let f = data_dir.join("peer_id");
    if let Ok(id) = std::fs::read_to_string(&f) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return id;
        }
    }
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "auris".into());
    let id = new_id("peer", &format!("{host}|{}", data_dir.display()));
    let _ = std::fs::write(&f, &id);
    id
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------- balise multicast ----------

/// Lance la boucle de balise (envoi toutes les 20 s + réception des balises
/// des autres instances). Thread dédié, ne panique jamais.
pub fn spawn_beacon(state: AppState) {
    let _ = std::thread::Builder::new()
        .name("auris-multiroom".into())
        .spawn(move || beacon_loop(state));
}

fn self_base(state: &AppState) -> String {
    let port = state
        .bound_addr
        .read()
        .unwrap()
        .map(|a| a.port())
        .unwrap_or(3000);
    let ip = state
        .bound_addr
        .read()
        .unwrap()
        .map(|a| a.ip())
        .filter(|ip| !ip.is_unspecified())
        .map(|ip| ip.to_string())
        .unwrap_or_else(crate::dlna_renderer::local_ip);
    format!("http://{ip}:{port}")
}

fn beacon_loop(state: AppState) {
    let mcast_addr = SocketAddr::new(MCAST.into(), MCAST_PORT);
    let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, MCAST_PORT)) {
        Ok(s) => {
            let _ = s.join_multicast_v4(&MCAST, &Ipv4Addr::UNSPECIFIED);
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            s
        }
        Err(_) => match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            Ok(s) => {
                let _ = s.join_multicast_v4(&MCAST, &Ipv4Addr::UNSPECIFIED);
                s
            }
            Err(e) => {
                tracing::warn!("multiroom: socket multicast indisponible: {e}");
                return;
            }
        },
    };
    let receiving = sock
        .local_addr()
        .map(|a| a.port() == MCAST_PORT)
        .unwrap_or(false);
    let mut buf = [0u8; 2048];
    let mut last_beacon = Instant::now() - BEACON_PERIOD;
    loop {
        if last_beacon.elapsed() >= BEACON_PERIOD {
            let beacon = PeerInfo {
                id: state.multiroom.self_id.clone(),
                name: crate::dlna_renderer::friendly_name(&state),
                base: self_base(&state),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ts_ms: now_ms(),
            };
            if let Ok(txt) = serde_json::to_string(&beacon) {
                let _ = sock.send_to(txt.as_bytes(), mcast_addr);
            }
            last_beacon = Instant::now();
        }
        if !receiving {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Ok(p) = serde_json::from_slice::<PeerInfo>(&buf[..n]) {
                    if p.id != state.multiroom.self_id {
                        state.multiroom.upsert_peer(p);
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

// ---------- synchro ----------

/// Offset estimé (ms) : horloge du pair - horloge locale, mesuré via
/// `/api/multiroom/clock` (RTT/2 retranché).
async fn clock_offset_ms(client: &reqwest::Client, base: &str) -> Option<i64> {
    let t0 = now_ms() as i64;
    let resp = client
        .get(format!("{base}/api/multiroom/clock"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    let t1 = now_ms() as i64;
    let v: serde_json::Value = resp.json().await.ok()?;
    let peer_now = v.get("now_ms")?.as_i64()?;
    let rtt = t1 - t0;
    Some(peer_now - (t0 + rtt / 2))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receive {
    pub action: String,
    #[serde(default)]
    pub uri: Option<String>,
    /// heure de démarrage exprimée dans l'horloge du récepteur (ms unix).
    #[serde(default)]
    pub start_at_ms: Option<u64>,
    #[serde(default)]
    pub position_secs: Option<f64>,
    #[serde(default)]
    pub volume: Option<u8>,
}

/// Applique une consigne reçue d'un autre membre du groupe.
fn apply_receive(state: &AppState, r: Receive) {
    match r.action.as_str() {
        "play" => {
            if let Some(uri) = r.uri.clone() {
                match r.start_at_ms {
                    Some(at) => {
                        let engine = state.engine.clone();
                        let pos = r.position_secs.unwrap_or(0.0);
                        tokio::spawn(async move {
                            let now = now_ms();
                            if at > now {
                                tokio::time::sleep(Duration::from_millis(at - now)).await;
                            }
                            engine.play_uri(uri);
                            if pos > 0.0 {
                                engine.seek(pos);
                            }
                        });
                    }
                    None => {
                        state.engine.play_uri(uri);
                        if let Some(pos) = r.position_secs {
                            if pos > 0.0 {
                                state.engine.seek(pos);
                            }
                        }
                    }
                }
            }
        }
        "pause" => state.engine.pause(),
        "stop" => state.engine.stop(),
        "seek" => {
            if let Some(pos) = r.position_secs {
                state.engine.seek(pos);
            }
        }
        "volume" => {
            if let Some(v) = r.volume {
                state.engine.set_volume(v);
            }
        }
        other => tracing::debug!("multiroom: action reçue inconnue {other}"),
    }
}

/// Envoie une consigne à un pair ; renvoie true si accepté (HTTP 2xx).
async fn send_to_peer(client: &reqwest::Client, base: &str, r: &Receive) -> bool {
    client
        .post(format!("{base}/api/multiroom/receive"))
        .timeout(Duration::from_secs(5))
        .json(r)
        .send()
        .await
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

#[derive(Debug, Serialize)]
pub struct GroupActionReport {
    pub group: String,
    pub action: String,
    pub ok: Vec<String>,
    pub failed: Vec<String>,
}

/// Diffuse une action à tous les membres du groupe (local compris).
async fn broadcast(
    state: &AppState,
    group_id: &str,
    action: &str,
    uri: Option<String>,
    position: Option<f64>,
    volume: Option<u8>,
    only_member: Option<&str>,
) -> Result<GroupActionReport, String> {
    let groups = state.multiroom.groups();
    let grp = groups
        .iter()
        .find(|g| g.id == group_id)
        .cloned()
        .ok_or_else(|| format!("groupe inconnu: {group_id}"))?;
    let client = reqwest::Client::new();
    let mut ok: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for member in &grp.members {
        if let Some(only) = only_member {
            if member != only {
                continue;
            }
        }
        if member == &state.multiroom.self_id {
            let r = Receive {
                action: action.to_string(),
                uri: uri.clone(),
                start_at_ms: None,
                position_secs: position,
                volume,
            };
            apply_receive(state, r);
            ok.push("local".to_string());
        } else {
            let peer = state
                .multiroom
                .peers()
                .into_iter()
                .find(|p| &p.id == member);
            let Some(peer) = peer else {
                failed.push(format!("{member} (hors ligne)"));
                continue;
            };
            // horloge maître : start_at exprimé dans l'horloge du pair
            let start_at_ms = clock_offset_ms(&client, &peer.base)
                .await
                .map(|offset| (now_ms() as i64 + offset + SYNC_DELAY_MS as i64) as u64);
            let r = Receive {
                action: action.to_string(),
                uri: uri.clone(),
                start_at_ms,
                position_secs: position,
                volume,
            };
            if send_to_peer(&client, &peer.base, &r).await {
                ok.push(peer.name);
            } else {
                failed.push(peer.name);
            }
        }
    }
    Ok(GroupActionReport {
        group: grp.name,
        action: action.to_string(),
        ok,
        failed,
    })
}

// ---------- handlers ----------

pub async fn get_state(State(s): State<AppState>) -> Json<MultiroomState> {
    Json(MultiroomState {
        self_id: s.multiroom.self_id.clone(),
        self_name: crate::dlna_renderer::friendly_name(&s),
        peers: s.multiroom.peers(),
        groups: s.multiroom.groups(),
    })
}

pub async fn get_clock(State(_s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "now_ms": now_ms() }))
}

#[derive(Deserialize)]
pub struct GroupBody {
    name: String,
}

pub async fn post_group(State(s): State<AppState>, Json(b): Json<GroupBody>) -> Json<Group> {
    Json(s.multiroom.create_group(&b.name))
}

pub async fn delete_group(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "removed": s.multiroom.delete_group(&id) }))
}

#[derive(Deserialize)]
pub struct MemberBody {
    member: String,
    #[serde(default = "default_true")]
    add: bool,
}

fn default_true() -> bool {
    true
}

pub async fn post_member(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<MemberBody>,
) -> axum::response::Response {
    match s.multiroom.set_member(&id, &b.member, b.add) {
        Some(g) => Json(g).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "groupe inconnu" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct PlayBody {
    group: String,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    position: Option<f64>,
}

pub async fn post_play(
    State(s): State<AppState>,
    Json(b): Json<PlayBody>,
) -> axum::response::Response {
    // uri par défaut = piste en cours
    let uri = b.uri.or_else(|| s.engine.state().current.map(|t| t.uri));
    let position = b.position.unwrap_or_else(|| s.engine.state().position_secs);
    match broadcast(&s, &b.group, "play", uri, Some(position), None, None).await {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct GroupBody2 {
    group: String,
}

pub async fn post_pause(
    State(s): State<AppState>,
    Json(b): Json<GroupBody2>,
) -> axum::response::Response {
    match broadcast(&s, &b.group, "pause", None, None, None, None).await {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

pub async fn post_stop(
    State(s): State<AppState>,
    Json(b): Json<GroupBody2>,
) -> axum::response::Response {
    match broadcast(&s, &b.group, "stop", None, None, None, None).await {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct SeekBody {
    group: String,
    secs: f64,
}

pub async fn post_seek(
    State(s): State<AppState>,
    Json(b): Json<SeekBody>,
) -> axum::response::Response {
    match broadcast(&s, &b.group, "seek", None, Some(b.secs), None, None).await {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct VolumeBody {
    group: String,
    /// membre ciblé (volume par zone) ; absent = tout le groupe.
    #[serde(default)]
    member: Option<String>,
    volume: u8,
}

pub async fn post_volume(
    State(s): State<AppState>,
    Json(b): Json<VolumeBody>,
) -> axum::response::Response {
    match broadcast(
        &s,
        &b.group,
        "volume",
        None,
        None,
        Some(b.volume),
        b.member.as_deref(),
    )
    .await
    {
        Ok(rep) => Json(rep).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

pub async fn post_receive(
    State(s): State<AppState>,
    Json(r): Json<Receive>,
) -> axum::response::Response {
    apply_receive(&s, r);
    Json(serde_json::json!({ "ok": true })).into_response()
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("auris-mr-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn peer_id_stable() {
        let d = tmp_dir("pid");
        let a = peer_id(&d);
        let b = peer_id(&d);
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn groups_persist_roundtrip() {
        let d = tmp_dir("grp");
        let hub = MultiroomHub::new(d.clone(), "self".into());
        let g = hub.create_group("Salon + Cuisine");
        assert_eq!(g.members, vec!["self".to_string()]);
        hub.set_member(&g.id, "peer-1", true);
        hub.set_member(&g.id, "peer-2", true);
        hub.set_member(&g.id, "peer-1", false);
        // rechargement depuis le disque
        let hub2 = MultiroomHub::new(d.clone(), "self".into());
        let gs = hub2.groups();
        assert_eq!(gs.len(), 1);
        assert_eq!(gs[0].name, "Salon + Cuisine");
        assert_eq!(
            gs[0].members,
            vec!["self".to_string(), "peer-2".to_string()]
        );
        assert!(hub2.delete_group(&g.id));
        assert!(!hub2.delete_group(&g.id));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn beacon_json_roundtrip() {
        let p = PeerInfo {
            id: "abc".into(),
            name: "Salon".into(),
            base: "http://192.168.1.10:3000".into(),
            version: "0.1.0".into(),
            ts_ms: 123,
        };
        let txt = serde_json::to_string(&p).unwrap();
        let q: PeerInfo = serde_json::from_str(&txt).unwrap();
        assert_eq!(q.id, "abc");
        assert_eq!(q.base, "http://192.168.1.10:3000");
    }

    #[test]
    fn peer_ttl_expiry() {
        let d = tmp_dir("ttl");
        let hub = MultiroomHub::new(d.clone(), "self".into());
        hub.upsert_peer(PeerInfo {
            id: "p1".into(),
            name: "n".into(),
            base: "b".into(),
            version: "v".into(),
            ts_ms: 0,
        });
        assert_eq!(hub.peers().len(), 1);
        // expire manuellement
        {
            let mut m = hub.peers.lock().unwrap();
            if let Some((_, seen)) = m.get_mut("p1") {
                *seen = Instant::now() - PEER_TTL - Duration::from_secs(1);
            }
        }
        assert_eq!(hub.peers().len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn receive_deserialize_defaults() {
        let r: Receive = serde_json::from_str(r#"{"action":"pause"}"#).unwrap();
        assert_eq!(r.action, "pause");
        assert!(r.uri.is_none() && r.start_at_ms.is_none() && r.volume.is_none());
    }
}
