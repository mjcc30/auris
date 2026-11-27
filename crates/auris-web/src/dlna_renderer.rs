//! Enceinte DLNA (MediaRenderer UPnP) — #35.
//!
//! Auris annonce un device `MediaRenderer:1` en SSDP (NOTIFY alive +
//! réponses M-SEARCH) et expose deux services SOAP :
//! - AVTransport (`/dlna/avtransport`) : SetAVTransportURI / Play / Pause /
//!   Stop / Seek / Next / Previous + GetTransportInfo / GetPositionInfo /
//!   GetMediaInfo, adossés au moteur de lecture ;
//! - RenderingControl (`/dlna/rendering`) : Get/SetVolume, Get/SetMute.
//!
//! Résultat : Auris apparait comme « enceinte DLNA » dans BubbleUPnP,
//! Hi-Fi Cast, etc. qui peuvent pousser de la musique dessus.
//! (La direction inverse — pousser vers une enceinte tierce — reste ouverte.)

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use auris_core::model::PlayStatus;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::AppState;

const SSDP_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const DEVICE_TYPE: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
const ROOT_DEVICE: &str = "upnp:rootdevice";
const AVT_SERVICE: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RCS_SERVICE: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
const MAX_AGE: u32 = 1800;
/// Période d'annonce SSDP (NOTIFY alive) — bien inférieure à max-age.
const ALIVE_PERIOD: Duration = Duration::from_secs(60);

// ---------- identité du device ----------

fn renderer_enabled(s: &AppState) -> bool {
    s.settings.read().unwrap().network.dlna_renderer
}

/// Nom « friendly » affiché dans les applis DLNA.
pub fn friendly_name(s: &AppState) -> String {
    let r = s.settings.read().unwrap();
    let name = r.system.device_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    drop(r);
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Auris".to_string())
}

/// UUID stable dérivé du nom de l'appareil (FNV-1a 64 bits, formaté en UUID).
pub fn device_uuid(host: &str) -> String {
    fn fnv(s: &str, seed: u64) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed;
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    let a = fnv(host, 0);
    let b = fnv(host, 0x9e37_79b9_7f4a_7c15);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16 & 0x0fff | 0x4000,
        ((b as u16) & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

/// IP locale "de sortie" (sans envoyer de paquet : connexion UDP vers le
/// groupe multicast, on lit juste l'adresse locale choisie par la pile).
pub fn local_ip() -> String {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect((SSDP_MULTICAST, SSDP_PORT))?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// Base URL du serveur (`http://<ip>:<port>`), déduite du header Host de la
/// requête quand il est présent (proxy/port externe), sinon IP locale.
fn base_url(s: &AppState, headers: &HeaderMap) -> String {
    if let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) {
        if !host.is_empty() {
            return format!("http://{host}");
        }
    }
    let port = s
        .bound_addr
        .read()
        .unwrap()
        .map(|a| a.port())
        .unwrap_or(3000);
    format!("http://{}:{}", local_ip(), port)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

// ---------- conversions temps ----------

/// "H:MM:SS" (ou "HH:MM:SS.sss") -> secondes.
pub fn parse_hms(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("NOT_IMPLEMENTED") {
        return None;
    }
    let (hms, frac) = match s.split_once('.') {
        Some((a, b)) => (
            a,
            b.parse::<f64>()
                .ok()
                .map(|f| f / 10f64.powi(b.len() as i32)),
        ),
        None => (s, None),
    };
    let mut parts = hms.split(':');
    let h: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    let sec: f64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec + frac.unwrap_or(0.0))
}

/// secondes -> "H:MM:SS" (format REL_TIME UPnP).
pub fn secs_to_hms(secs: f64) -> String {
    let secs = if secs.is_finite() && secs > 0.0 {
        secs
    } else {
        0.0
    };
    let total = secs.round() as u64;
    format!(
        "{}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

// ---------- parsing SOAP ----------

/// Extrait l'appel SOAP : (nom de l'action, arguments {nom -> valeur}).
/// Tolérant aux préfixes d'espaces de noms (`<u:SetAVTransportURI …>`) et aux
/// sections CDATA dans les valeurs.
pub fn soap_call(xml: &str) -> Option<(String, HashMap<String, String>)> {
    let body = tag_inner(xml, "Body")?;
    // premier tag du Body = l'action
    let (action, rest) = first_tag(body)?;
    let local = action.rsplit(':').next().unwrap_or(&action).to_string();
    let close = format!("</{action}>");
    let args_seg = match rest.find(&close) {
        Some(i) => &rest[..i],
        None => rest,
    };
    let mut args = HashMap::new();
    let mut cur = args_seg;
    while let Some((name, after)) = first_tag(cur) {
        if name.starts_with('/') || name.starts_with('?') || name.starts_with('!') {
            break;
        }
        let local_name = name.rsplit(':').next().unwrap_or(&name).to_string();
        let close_name = format!("</{name}>");
        let (val, next) = match after.find(&close_name) {
            Some(i) => (&after[..i], &after[i + close_name.len()..]),
            None => (after, ""),
        };
        let val = strip_cdata(val);
        args.insert(local_name, xml_unescape(val.trim()));
        cur = next;
    }
    Some((local, args))
}

/// Contenu intérieur du premier tag ouvrant dont le nom local est `name`
/// (ex. `<s:Body>…</s:Body>` -> `…`), à n'importe quelle profondeur.
fn tag_inner<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' && !matches!(bytes.get(i + 1), Some(b'/') | Some(b'?') | Some(b'!')) {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len()
                && !bytes[j].is_ascii_whitespace()
                && bytes[j] != b'>'
                && bytes[j] != b'/'
            {
                j += 1;
            }
            if j == start {
                i += 1;
                continue;
            }
            let tag = &xml[start..j];
            let local = tag.rsplit(':').next().unwrap_or(tag);
            let gt = xml[j..].find('>')? + j;
            if local == name {
                if xml[j..gt].trim_end().ends_with('/') {
                    return Some(""); // tag auto-fermant
                }
                let close = format!("</{tag}>");
                let inner_start = gt + 1;
                let end = xml[inner_start..].find(&close)? + inner_start;
                return Some(&xml[inner_start..end]);
            }
            i = gt + 1;
        } else {
            i += 1;
        }
    }
    None
}

/// Premier tag de `s` : renvoie (nom complet du tag, contenu après `>`).
/// Le nom peut inclure le préfixe (`u:Play`). Ignore `<?…?>` et `<!--…-->`.
fn first_tag(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            match bytes.get(i + 1) {
                Some(b'?') => {
                    let j = s[i..].find("?>")?;
                    i += j + 2;
                    continue;
                }
                Some(b'!') => {
                    let j = s[i..].find('>')?;
                    i += j + 1;
                    continue;
                }
                Some(_) => {}
                None => return None,
            }
            let start = i + 1;
            let mut j = start;
            while j < bytes.len()
                && !bytes[j].is_ascii_whitespace()
                && bytes[j] != b'>'
                && bytes[j] != b'/'
            {
                j += 1;
            }
            if j == start {
                return None;
            }
            let name = s[start..j].to_string();
            let gt = s[j..].find('>')? + j;
            return Some((name, &s[gt + 1..]));
        }
        i += 1;
    }
    None
}

fn strip_cdata(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("<![CDATA[") {
        if let Some(end) = rest.find("]]>") {
            return &rest[..end];
        }
    }
    s
}

// ---------- réponses SOAP ----------

fn soap_response(service: &str, action: &str, out: &[(&str, String)]) -> String {
    let args: String = out
        .iter()
        .map(|(k, v)| format!("<{k}>{}</{k}>", escape_xml(v)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><u:{action}Response xmlns:u="{service}">{args}</u:{action}Response></s:Body></s:Envelope>"#
    )
}

fn soap_fault(code: u16, desc: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>
<detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{}</errorDescription></UPnPError></detail>
</s:Fault></s:Body></s:Envelope>"#,
        escape_xml(desc)
    )
}

fn xml_response(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")],
        body,
    )
        .into_response()
}

fn fault_response(code: u16, desc: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")],
        soap_fault(code, desc),
    )
        .into_response()
}

// ---------- descriptions UPnP ----------

/// Document de description du device (servi sur `/dlna/desc.xml`).
/// Les URL de service sont relatives (résolues contre l'URL de description).
pub fn device_description(_base: &str, friendly: &str, uuid: &str) -> String {
    let ver = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<device>
<deviceType>{DEVICE_TYPE}</deviceType>
<friendlyName>{}</friendlyName>
<manufacturer>mjcc30</manufacturer>
<manufacturerURL>https://github.com/mjcc30/auris</manufacturerURL>
<modelName>Auris</modelName>
<modelNumber>{ver}</modelNumber>
<modelURL>https://auris.audio</modelURL>
<UDN>uuid:{uuid}</UDN>
<serviceList>
<service>
<serviceType>{AVT_SERVICE}</serviceType>
<serviceId>urn:upnp-org:serviceId:AVTransport</serviceId>
<SCPDURL>/dlna/scpd/AVTransport.xml</SCPDURL>
<controlURL>/dlna/avtransport</controlURL>
<eventSubURL>/dlna/avtransport/event</eventSubURL>
</service>
<service>
<serviceType>{RCS_SERVICE}</serviceType>
<serviceId>urn:upnp-org:serviceId:RenderingControl</serviceId>
<SCPDURL>/dlna/scpd/RenderingControl.xml</SCPDURL>
<controlURL>/dlna/rendering</controlURL>
<eventSubURL>/dlna/rendering/event</eventSubURL>
</service>
</serviceList>
</device>
</root>"#,
        escape_xml(friendly)
    )
}

/// SCPD minimal mais valide d'un service (liste des actions + état).
pub fn scpd(service: &str) -> Option<String> {
    let (actions, state_vars) = match service {
        AVT_SERVICE => (avt_actions(), avt_state()),
        RCS_SERVICE => (rcs_actions(), rcs_state()),
        _ => return None,
    };
    Some(format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<actionList>{actions}</actionList>
<serviceStateTable>{state_vars}</serviceStateTable>
</scpd>"#
    ))
}

fn action_xml(name: &str, args: &[(&str, bool, &str)]) -> String {
    let args_xml: String = args
        .iter()
        .map(|(n, out, var)| {
            let dir = if *out { "out" } else { "in" };
            format!("<argument><name>{n}</name><direction>{dir}</direction><relatedStateVariable>{var}</relatedStateVariable></argument>")
        })
        .collect();
    format!("<action><name>{name}</name><argumentList>{args_xml}</argumentList></action>")
}

fn state_var(name: &str, ty: &str, sends: bool) -> String {
    let ev = if sends { " yes" } else { "" };
    format!(
        "<stateVariable sendEvents=\"{ev}\"><name>{name}</name><dataType>{ty}</dataType></stateVariable>"
    )
    .replace("sendEvents=\" yes\"", "sendEvents=\"yes\"")
}

fn avt_actions() -> String {
    [
        action_xml(
            "SetAVTransportURI",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("CurrentURI", false, "AVTransportURI"),
                ("CurrentURIMetaData", false, "AVTransportURIMetaData"),
            ],
        ),
        action_xml(
            "Play",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Speed", false, "TransportPlaySpeed"),
            ],
        ),
        action_xml("Pause", &[("InstanceID", false, "A_ARG_TYPE_InstanceID")]),
        action_xml("Stop", &[("InstanceID", false, "A_ARG_TYPE_InstanceID")]),
        action_xml(
            "Seek",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Unit", false, "A_ARG_TYPE_SeekMode"),
                ("Target", false, "A_ARG_TYPE_SeekTarget"),
            ],
        ),
        action_xml("Next", &[("InstanceID", false, "A_ARG_TYPE_InstanceID")]),
        action_xml(
            "Previous",
            &[("InstanceID", false, "A_ARG_TYPE_InstanceID")],
        ),
        action_xml(
            "GetTransportInfo",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("CurrentTransportState", true, "TransportState"),
                ("CurrentTransportStatus", true, "TransportStatus"),
                ("CurrentSpeed", true, "TransportPlaySpeed"),
            ],
        ),
        action_xml(
            "GetPositionInfo",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Track", true, "CurrentTrack"),
                ("TrackDuration", true, "CurrentTrackDuration"),
                ("TrackMetaData", true, "AVTransportURIMetaData"),
                ("TrackURI", true, "AVTransportURI"),
                ("RelTime", true, "RelativeTimePosition"),
                ("AbsTime", true, "AbsoluteTimePosition"),
                ("RelCount", true, "RelativeCountPosition"),
                ("AbsCount", true, "AbsoluteCountPosition"),
            ],
        ),
        action_xml(
            "GetMediaInfo",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("NrTracks", true, "NumberOfTracks"),
                ("MediaDuration", true, "CurrentMediaDuration"),
                ("CurrentURI", true, "AVTransportURI"),
                ("CurrentURIMetaData", true, "AVTransportURIMetaData"),
                ("NextURI", true, "NextAVTransportURI"),
                ("NextURIMetaData", true, "NextAVTransportURIMetaData"),
                ("PlayMedium", true, "PlaybackStorageMedium"),
                ("RecordMedium", true, "RecordStorageMedium"),
                ("WriteStatus", true, "RecordMediumWriteStatus"),
            ],
        ),
        action_xml(
            "GetDeviceCapabilities",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("PlayMedia", true, "SupportedPlayMedia"),
                ("RecMedia", true, "SupportedRecordMedia"),
                ("RecQualityModes", true, "SupportedRecordQualityModes"),
            ],
        ),
        action_xml(
            "GetTransportSettings",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("PlayMode", true, "CurrentPlayMode"),
                ("RecQualityMode", true, "CurrentRecordQualityMode"),
            ],
        ),
    ]
    .concat()
}

fn avt_state() -> String {
    [
        state_var("A_ARG_TYPE_InstanceID", "ui4", false),
        state_var("AVTransportURI", "string", false),
        state_var("AVTransportURIMetaData", "string", false),
        state_var("NextAVTransportURI", "string", false),
        state_var("NextAVTransportURIMetaData", "string", false),
        state_var("TransportState", "string", true),
        state_var("TransportStatus", "string", true),
        state_var("TransportPlaySpeed", "string", false),
        state_var("CurrentTrack", "ui4", false),
        state_var("CurrentTrackDuration", "string", false),
        state_var("RelativeTimePosition", "string", false),
        state_var("AbsoluteTimePosition", "string", false),
        state_var("RelativeCountPosition", "i4", false),
        state_var("AbsoluteCountPosition", "i4", false),
        state_var("NumberOfTracks", "ui4", false),
        state_var("CurrentMediaDuration", "string", false),
        state_var("PlaybackStorageMedium", "string", false),
        state_var("RecordStorageMedium", "string", false),
        state_var("RecordMediumWriteStatus", "string", false),
        state_var("SupportedPlayMedia", "string", false),
        state_var("SupportedRecordMedia", "string", false),
        state_var("SupportedRecordQualityModes", "string", false),
        state_var("CurrentPlayMode", "string", false),
        state_var("CurrentRecordQualityMode", "string", false),
        state_var("A_ARG_TYPE_SeekMode", "string", false),
        state_var("A_ARG_TYPE_SeekTarget", "string", false),
    ]
    .concat()
}

fn rcs_actions() -> String {
    [
        action_xml(
            "GetVolume",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Channel", false, "A_ARG_TYPE_Channel"),
                ("CurrentVolume", true, "Volume"),
            ],
        ),
        action_xml(
            "SetVolume",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Channel", false, "A_ARG_TYPE_Channel"),
                ("DesiredVolume", false, "Volume"),
            ],
        ),
        action_xml(
            "GetMute",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Channel", false, "A_ARG_TYPE_Channel"),
                ("CurrentMute", true, "Mute"),
            ],
        ),
        action_xml(
            "SetMute",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("Channel", false, "A_ARG_TYPE_Channel"),
                ("DesiredMute", false, "Mute"),
            ],
        ),
        action_xml(
            "ListPresets",
            &[
                ("InstanceID", false, "A_ARG_TYPE_InstanceID"),
                ("PresetNameList", true, "PresetNameList"),
            ],
        ),
    ]
    .concat()
}

fn rcs_state() -> String {
    [
        state_var("A_ARG_TYPE_InstanceID", "ui4", false),
        state_var("A_ARG_TYPE_Channel", "string", false),
        state_var("Volume", "ui2", true),
        state_var("Mute", "boolean", true),
        state_var("PresetNameList", "string", false),
    ]
    .concat()
}

// ---------- handlers axum ----------

pub async fn get_desc(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if !renderer_enabled(&s) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let base = base_url(&s, &headers);
    let friendly = friendly_name(&s);
    let uuid = device_uuid(&friendly);
    xml_response(device_description(&base, &friendly, &uuid))
}

pub async fn get_scpd(State(s): State<AppState>, Path(name): Path<String>) -> Response {
    if !renderer_enabled(&s) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // `AVTransport.xml` / `RenderingControl.xml` -> URN du service
    let svc = match name.as_str() {
        "AVTransport.xml" => AVT_SERVICE,
        "RenderingControl.xml" => RCS_SERVICE,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match scpd(svc) {
        Some(x) => xml_response(x),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn post_avtransport(
    State(s): State<AppState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !renderer_enabled(&s) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let text = String::from_utf8_lossy(&body);
    let Some((action, args)) = soap_call(&text) else {
        return fault_response(400, "Requête SOAP mal formée");
    };
    let resp = match action.as_str() {
        "SetAVTransportURI" => {
            let uri = args.get("CurrentURI").cloned().unwrap_or_default();
            if uri.is_empty() {
                s.engine.stop();
            } else {
                s.engine.play_uri(uri);
            }
            soap_response(AVT_SERVICE, "SetAVTransportURI", &[])
        }
        "Play" => {
            s.engine.resume();
            soap_response(AVT_SERVICE, "Play", &[])
        }
        "Pause" => {
            s.engine.pause();
            soap_response(AVT_SERVICE, "Pause", &[])
        }
        "Stop" => {
            s.engine.stop();
            soap_response(AVT_SERVICE, "Stop", &[])
        }
        "Seek" => {
            let unit = args.get("Unit").map(String::as_str).unwrap_or("");
            let target = args.get("Target").cloned().unwrap_or_default();
            if unit == "REL_TIME" {
                if let Some(sec) = parse_hms(&target) {
                    s.engine.seek(sec);
                }
            }
            soap_response(AVT_SERVICE, "Seek", &[])
        }
        "Next" => {
            s.engine.next();
            soap_response(AVT_SERVICE, "Next", &[])
        }
        "Previous" => {
            s.engine.prev();
            soap_response(AVT_SERVICE, "Previous", &[])
        }
        "GetTransportInfo" => {
            let st = s.engine.state();
            let state = match st.status {
                PlayStatus::Playing => "PLAYING",
                PlayStatus::Paused => "PAUSED_PLAYBACK",
                PlayStatus::Stopped => {
                    if st.current.is_some() {
                        "STOPPED"
                    } else {
                        "NO_MEDIA_PRESENT"
                    }
                }
            };
            soap_response(
                AVT_SERVICE,
                "GetTransportInfo",
                &[
                    ("CurrentTransportState", state.to_string()),
                    ("CurrentTransportStatus", "OK".to_string()),
                    ("CurrentSpeed", "1".to_string()),
                ],
            )
        }
        "GetPositionInfo" => {
            let st = s.engine.state();
            let cur = st.current.as_ref();
            let dur = cur
                .and_then(|t| t.duration_secs)
                .unwrap_or(st.duration_secs);
            soap_response(
                AVT_SERVICE,
                "GetPositionInfo",
                &[
                    ("Track", "1".to_string()),
                    ("TrackDuration", secs_to_hms(dur)),
                    ("TrackMetaData", String::new()),
                    ("TrackURI", cur.map(|t| t.uri.clone()).unwrap_or_default()),
                    ("RelTime", secs_to_hms(st.position_secs)),
                    ("AbsTime", secs_to_hms(st.position_secs)),
                    ("RelCount", "2147483647".to_string()),
                    ("AbsCount", "2147483647".to_string()),
                ],
            )
        }
        "GetMediaInfo" => {
            let st = s.engine.state();
            let cur = st.current.as_ref();
            soap_response(
                AVT_SERVICE,
                "GetMediaInfo",
                &[
                    ("NrTracks", "1".to_string()),
                    ("MediaDuration", secs_to_hms(st.duration_secs)),
                    ("CurrentURI", cur.map(|t| t.uri.clone()).unwrap_or_default()),
                    ("CurrentURIMetaData", String::new()),
                    ("NextURI", String::new()),
                    ("NextURIMetaData", String::new()),
                    ("PlayMedium", "NONE".to_string()),
                    ("RecordMedium", "NOT_IMPLEMENTED".to_string()),
                    ("WriteStatus", "NOT_IMPLEMENTED".to_string()),
                ],
            )
        }
        "GetDeviceCapabilities" => soap_response(
            AVT_SERVICE,
            "GetDeviceCapabilities",
            &[
                ("PlayMedia", "NETWORK,INTERNAL,HDD".to_string()),
                ("RecMedia", "NOT_IMPLEMENTED".to_string()),
                ("RecQualityModes", "NOT_IMPLEMENTED".to_string()),
            ],
        ),
        "GetTransportSettings" => soap_response(
            AVT_SERVICE,
            "GetTransportSettings",
            &[
                ("PlayMode", "NORMAL".to_string()),
                ("RecQualityMode", "NOT_IMPLEMENTED".to_string()),
            ],
        ),
        other => {
            tracing::debug!("DLNA AVTransport: action non supportée {other}");
            return fault_response(600, &format!("Action non supportée: {other}"));
        }
    };
    xml_response(resp)
}

pub async fn post_rendering(
    State(s): State<AppState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !renderer_enabled(&s) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let text = String::from_utf8_lossy(&body);
    let Some((action, args)) = soap_call(&text) else {
        return fault_response(400, "Requête SOAP mal formée");
    };
    let resp = match action.as_str() {
        "GetVolume" => {
            let st = s.engine.state();
            soap_response(
                RCS_SERVICE,
                "GetVolume",
                &[("CurrentVolume", st.volume.to_string())],
            )
        }
        "SetVolume" => {
            if let Some(v) = args.get("DesiredVolume").and_then(|v| v.parse::<u8>().ok()) {
                s.engine.set_volume(v);
            }
            soap_response(RCS_SERVICE, "SetVolume", &[])
        }
        "GetMute" => {
            let st = s.engine.state();
            soap_response(
                RCS_SERVICE,
                "GetMute",
                &[("CurrentMute", if st.muted { "1" } else { "0" }.to_string())],
            )
        }
        "SetMute" => {
            let m = args
                .get("DesiredMute")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            s.engine.set_muted(m);
            soap_response(RCS_SERVICE, "SetMute", &[])
        }
        "ListPresets" => soap_response(
            RCS_SERVICE,
            "ListPresets",
            &[("PresetNameList", "Factory".to_string())],
        ),
        other => {
            tracing::debug!("DLNA RenderingControl: action non supportée {other}");
            return fault_response(600, &format!("Action non supportée: {other}"));
        }
    };
    xml_response(resp)
}

// ---------- annonce SSDP ----------

/// Lance le thread d'annonce SSDP (NOTIFY alive + réponses M-SEARCH).
/// Ne panique jamais : en cas d'échec réseau, on réessaie périodiquement.
pub fn spawn_announcer(state: AppState, base: String) {
    let _ = std::thread::Builder::new()
        .name("dlna-ssdp".into())
        .spawn(move || announce_loop(state, base));
}

fn server_token() -> String {
    format!(
        "{} UPnP/1.0 Auris/{}",
        std::env::consts::OS.to_uppercase(),
        env!("CARGO_PKG_VERSION")
    )
}

/// NT annoncés (rootdevice, device, uuid + les deux services).
fn notify_targets(uuid: &str) -> Vec<(String, String)> {
    let u = format!("uuid:{uuid}");
    vec![
        (ROOT_DEVICE.to_string(), u.clone()),
        (u.clone(), u.clone()),
        (DEVICE_TYPE.to_string(), format!("{u}::{DEVICE_TYPE}")),
        (AVT_SERVICE.to_string(), format!("{u}::{AVT_SERVICE}")),
        (RCS_SERVICE.to_string(), format!("{u}::{RCS_SERVICE}")),
    ]
}

fn notify_msg(nt: &str, usn: &str, location: &str, nts: &str) -> String {
    format!(
        "NOTIFY * HTTP/1.1\r\nHOST: {SSDP_MULTICAST}:{SSDP_PORT}\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nLOCATION: {location}\r\nNT: {nt}\r\nNTS: {nts}\r\nSERVER: {}\r\nUSN: {usn}\r\n\r\n",
        server_token()
    )
}

fn search_response(st: &str, usn: &str, location: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nEXT:\r\nLOCATION: {location}\r\nSERVER: {}\r\nST: {st}\r\nUSN: {usn}\r\n\r\n",
        server_token()
    )
}

/// Header `ST:` d'un M-SEARCH (insensible à la casse).
fn msearch_st(msg: &str) -> Option<String> {
    for line in msg.lines().skip(1) {
        let (k, v) = line.split_once(':')?;
        if k.trim().eq_ignore_ascii_case("st") {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn announce_loop(state: AppState, base: String) {
    let location = format!("{base}/dlna/desc.xml");
    let ssdp_addr = SocketAddr::new(SSDP_MULTICAST.into(), SSDP_PORT);
    // Bind du port 1900 pour recevoir le multicast ; repli sur un port
    // éphémère (annonce seule, sans réponse aux M-SEARCH) si déjà pris.
    let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, SSDP_PORT)) {
        Ok(s) => {
            let _ = s.join_multicast_v4(&SSDP_MULTICAST, &Ipv4Addr::UNSPECIFIED);
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            s
        }
        Err(_) => match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            Ok(s) => {
                let _ = s.join_multicast_v4(&SSDP_MULTICAST, &Ipv4Addr::UNSPECIFIED);
                s
            }
            Err(e) => {
                tracing::warn!("DLNA: socket SSDP indisponible: {e}");
                return;
            }
        },
    };
    let receiving = sock
        .local_addr()
        .map(|a| a.port() == SSDP_PORT)
        .unwrap_or(false);
    let mut buf = [0u8; 2048];
    let mut last_alive = Instant::now() - ALIVE_PERIOD; // annonce immédiate
    let mut was_enabled = true;
    loop {
        let enabled = renderer_enabled(&state);
        let friendly = friendly_name(&state);
        let uuid = device_uuid(&friendly);
        let targets = notify_targets(&uuid);
        if enabled != was_enabled {
            if !enabled {
                // byebye : on se retire proprement du réseau
                for (nt, usn) in &targets {
                    let _ = sock.send_to(
                        notify_msg(nt, usn, &location, "ssdp:byebye").as_bytes(),
                        ssdp_addr,
                    );
                }
            } else {
                last_alive = Instant::now() - ALIVE_PERIOD; // ré-annonce immédiate
            }
            was_enabled = enabled;
        }
        if enabled && last_alive.elapsed() >= ALIVE_PERIOD {
            for (nt, usn) in &targets {
                if let Err(e) = sock.send_to(
                    notify_msg(nt, usn, &location, "ssdp:alive").as_bytes(),
                    ssdp_addr,
                ) {
                    tracing::debug!("DLNA: NOTIFY alive: {e}");
                }
            }
            last_alive = Instant::now();
        }
        if !receiving {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let msg = String::from_utf8_lossy(&buf[..n]);
                if msg.starts_with("M-SEARCH") {
                    if let Some(st) = msearch_st(&msg) {
                        let hit = targets.iter().find(|(nt, _)| {
                            st == "ssdp:all" || st == *nt || st == format!("uuid:{uuid}")
                        });
                        if let Some((nt, usn)) = hit {
                            let _ =
                                sock.send_to(search_response(nt, usn, &location).as_bytes(), from);
                        }
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                tracing::debug!("DLNA: recv SSDP: {e}");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hms_ok() {
        assert_eq!(parse_hms("0:00:00"), Some(0.0));
        assert_eq!(parse_hms("1:02:03"), Some(3723.0));
        assert_eq!(parse_hms("0:00:30.5"), Some(30.5));
        assert_eq!(parse_hms("NOT_IMPLEMENTED"), None);
        assert_eq!(parse_hms("abc"), None);
    }

    #[test]
    fn hms_roundtrip() {
        assert_eq!(secs_to_hms(0.0), "0:00:00");
        assert_eq!(secs_to_hms(3723.4), "1:02:03");
        assert_eq!(secs_to_hms(-5.0), "0:00:00");
        assert_eq!(secs_to_hms(f64::NAN), "0:00:00");
    }

    #[test]
    fn soap_call_extracts_action_and_args() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body><u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
<InstanceID>0</InstanceID>
<CurrentURI>http://example.org/a%20b.flac</CurrentURI>
<CurrentURIMetaData><![CDATA[<DIDL-Lite>x &amp; y</DIDL-Lite>]]></CurrentURIMetaData>
</u:SetAVTransportURI></s:Body></s:Envelope>"#;
        let (action, args) = soap_call(xml).unwrap();
        assert_eq!(action, "SetAVTransportURI");
        assert_eq!(args.get("InstanceID").unwrap(), "0");
        assert_eq!(
            args.get("CurrentURI").unwrap(),
            "http://example.org/a%20b.flac"
        );
        assert_eq!(
            args.get("CurrentURIMetaData").unwrap(),
            "<DIDL-Lite>x & y</DIDL-Lite>"
        );
    }

    #[test]
    fn soap_call_seek() {
        let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>
<u:Seek xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>0:01:40</Target></u:Seek>
</s:Body></s:Envelope>"#;
        let (action, args) = soap_call(xml).unwrap();
        assert_eq!(action, "Seek");
        assert_eq!(parse_hms(args.get("Target").unwrap()), Some(100.0));
    }

    #[test]
    fn soap_response_envelope() {
        let r = soap_response(AVT_SERVICE, "GetVolume", &[("CurrentVolume", "42".into())]);
        assert!(r.contains(
            "<u:GetVolumeResponse xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">"
        ));
        assert!(r.contains("<CurrentVolume>42</CurrentVolume>"));
    }

    #[test]
    fn soap_fault_has_error_code() {
        let f = soap_fault(600, "Action non supportée: Foo");
        assert!(f.contains("<errorCode>600</errorCode>"));
        assert!(f.contains("Action non supportée: Foo"));
    }

    #[test]
    fn desc_contains_services_and_udn() {
        let d = device_description("http://192.168.1.10:3000", "Salon & TV", "1234-uuid");
        assert!(d.contains("<friendlyName>Salon &amp; TV</friendlyName>"));
        assert!(d.contains("<UDN>uuid:1234-uuid</UDN>"));
        assert!(d.contains("<controlURL>/dlna/avtransport</controlURL>"));
        assert!(d.contains("<controlURL>/dlna/rendering</controlURL>"));
        assert!(d.contains(DEVICE_TYPE));
    }

    #[test]
    fn scpd_known_services() {
        let a = scpd(AVT_SERVICE).unwrap();
        assert!(a.contains("<name>SetAVTransportURI</name>"));
        assert!(a.contains("<name>GetPositionInfo</name>"));
        let r = scpd(RCS_SERVICE).unwrap();
        assert!(r.contains("<name>SetVolume</name>"));
        assert!(scpd("urn:foo").is_none());
    }

    #[test]
    fn uuid_stable_and_formatted() {
        let a = device_uuid("Salon");
        let b = device_uuid("Salon");
        let c = device_uuid("Cuisine");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 36);
        assert!(a.chars().filter(|c| *c == '-').count() == 4);
    }

    #[test]
    fn msearch_st_case_insensitive() {
        let msg = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nst: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        assert_eq!(
            msearch_st(msg).as_deref(),
            Some("urn:schemas-upnp-org:device:MediaRenderer:1")
        );
    }

    #[test]
    fn notify_and_search_shapes() {
        let n = notify_msg(DEVICE_TYPE, "uuid:x::nt", "http://h/d.xml", "ssdp:alive");
        assert!(n.starts_with("NOTIFY * HTTP/1.1\r\n"));
        assert!(n.contains("NTS: ssdp:alive"));
        assert!(n.contains("LOCATION: http://h/d.xml"));
        let r = search_response(DEVICE_TYPE, "uuid:x", "http://h/d.xml");
        assert!(r.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(r.contains("ST: urn:schemas-upnp-org:device:MediaRenderer:1"));
    }
}
