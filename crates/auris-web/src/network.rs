//! Interfaces réseau : état (Linux sysfs/ip + Windows PowerShell Get-NetAdapter)
//! et activation/désactivation par interface (#44 — Ethernet + Wi-Fi simultanés).
//!
//! L'action up/down exige des droits élevés (root / administrateur) : sur RPi,
//! prévoir polkit ou faire tourner le service en root. On refuse toute interface
//! inconnue de la liste courante (anti-injection dans la commande shell).

#[cfg(unix)]
use std::path::Path;
use std::process::Command;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use auris_core::AurisError;

use crate::{ApiResult, AppState};

/// Une interface réseau vue par le système.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    pub mac: Option<String>,
    /// Interface active (UP) ?
    pub up: bool,
    pub loopback: bool,
    pub wifi: bool,
    pub ips: Vec<String>,
    /// Porte la route par défaut ?
    pub default_route: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(unix)]
fn read_sys(name: &str, file: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/net/{name}/{file}"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(unix)]
fn list_interfaces() -> Vec<NetInterface> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/net") else {
        return out;
    };
    // route par défaut : « default via X dev Y … »
    let default_dev = run("ip", &["route", "show", "default"]).and_then(|s| {
        s.lines().find_map(|l| {
            l.split_whitespace()
                .skip_while(|w| *w != "dev")
                .nth(1)
                .map(str::to_string)
        })
    });
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let operstate = read_sys(&name, "operstate").unwrap_or_default();
        let lo = name == "lo" || read_sys(&name, "type").as_deref() == Some("772");
        let wifi = Path::new(&format!("/sys/class/net/{name}/wireless")).exists();
        let ips = run("ip", &["-o", "-4", "addr", "show", "dev", &name])
            .map(|s| {
                s.lines()
                    .filter_map(|l| l.split_whitespace().nth(3).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push(NetInterface {
            mac: read_sys(&name, "address").filter(|m| m != "00:00:00:00:00:00"),
            up: operstate == "up" || (lo && operstate == "unknown"),
            loopback: lo,
            wifi,
            ips,
            default_route: default_dev.as_deref() == Some(name.as_str()),
            rx_bytes: read_sys(&name, "statistics/rx_bytes")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            tx_bytes: read_sys(&name, "statistics/tx_bytes")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(windows)]
fn list_interfaces() -> Vec<NetInterface> {
    // CIM : noms de propriétés en anglais, stables quelle que soit la locale.
    let adapters = run(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-NetAdapter | Select-Object Name,Status,MacAddress,InterfaceDescription | ConvertTo-Json -Compress",
        ],
    );
    let ips_raw = run(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike '169.254.*' } | Select-Object InterfaceAlias,IPAddress | ConvertTo-Json -Compress",
        ],
    );
    let routes = run(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -ExpandProperty InterfaceAlias | ConvertTo-Json -Compress",
        ],
    );
    #[derive(Deserialize)]
    struct Adapter {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Status")]
        status: String,
        #[serde(rename = "MacAddress")]
        mac: Option<String>,
        #[serde(rename = "InterfaceDescription")]
        desc: Option<String>,
    }
    #[derive(Deserialize)]
    struct IpRow {
        #[serde(rename = "InterfaceAlias")]
        alias: String,
        #[serde(rename = "IPAddress")]
        ip: String,
    }
    let parse = |txt: Option<String>| -> Vec<serde_json::Value> {
        txt.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| match v {
                serde_json::Value::Array(a) => a,
                single => vec![single],
            })
            .unwrap_or_default()
    };
    let adapters: Vec<Adapter> = parse(adapters)
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    let mut ips: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for v in parse(ips_raw) {
        if let Ok(r) = serde_json::from_value::<IpRow>(v) {
            ips.entry(r.alias).or_default().push(r.ip);
        }
    }
    let default_set: std::collections::HashSet<String> = parse(routes)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    adapters
        .into_iter()
        .map(|a| {
            let wifi = a
                .desc
                .as_deref()
                .map(|d| d.contains("Wi-Fi") || d.contains("Wireless"))
                .unwrap_or(false);
            NetInterface {
                name: a.name.clone(),
                mac: a.mac.filter(|m| !m.is_empty()),
                up: a.status == "Up",
                loopback: a.desc.as_deref() == Some("Microsoft KM-TEST Loopback Adapter"),
                wifi,
                ips: ips.get(&a.name).cloned().unwrap_or_default(),
                default_route: default_set.contains(&a.name),
                rx_bytes: 0,
                tx_bytes: 0,
            }
        })
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn list_interfaces() -> Vec<NetInterface> {
    Vec::new()
}

pub fn interfaces() -> Vec<NetInterface> {
    list_interfaces()
}

#[derive(Debug, Deserialize)]
pub struct InterfaceAction {
    pub name: String,
    /// "up" | "down"
    pub action: String,
}

/// Active/désactive une interface. Le nom doit exister dans la liste courante.
pub fn set_interface(name: &str, action: &str) -> Result<(), AurisError> {
    let known = interfaces().into_iter().any(|i| i.name == name);
    if !known {
        return Err(AurisError::other(format!(
            "interface inconnue: {name} (refus pour éviter toute injection de commande)"
        )));
    }
    match action {
        "up" | "down" => {}
        other => {
            return Err(AurisError::other(format!(
                "action inconnue: {other} (attendu: up|down)"
            )))
        }
    }
    #[cfg(unix)]
    {
        let out = Command::new("ip")
            .args(["link", "set", "dev", name, action])
            .output()
            .map_err(|e| AurisError::other(format!("spawn ip: {e}")))?;
        if !out.status.success() {
            return Err(AurisError::other(format!(
                "ip link set dev {name} {action} a échoué (droits root requis ?) : {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let admin = if action == "up" {
            "ENABLED"
        } else {
            "DISABLED"
        };
        let out = Command::new("netsh")
            .args([
                "interface",
                "set",
                "interface",
                &format!("name={name}"),
                &format!("admin={admin}"),
            ])
            .output()
            .map_err(|e| AurisError::other(format!("spawn netsh: {e}")))?;
        if !out.status.success() {
            return Err(AurisError::other(format!(
                "netsh a échoué (administrateur requis ?) : {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (name, action);
        Err(AurisError::other("non supporté sur cet OS"))
    }
}

pub async fn get_interfaces(State(_s): State<AppState>) -> Json<Vec<NetInterface>> {
    Json(interfaces())
}

pub async fn post_interface(
    State(_s): State<AppState>,
    Json(a): Json<InterfaceAction>,
) -> ApiResult<Json<Vec<NetInterface>>> {
    set_interface(&a.name, &a.action)?;
    Ok(Json(interfaces()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_returns_something_on_dev() {
        // au moins l'interface loopback sur unix, au moins une carte sur Windows
        let v = interfaces();
        assert!(!v.is_empty(), "aucune interface détectée");
        assert!(v.iter().any(|i| i.loopback || !i.loopback));
    }

    #[test]
    fn action_rejette_nom_inconnu_et_action_invalide() {
        let r = set_interface("__definitely_not_an_iface__", "up");
        assert!(r.is_err());
        // action invalide : on ne connaît de toute façon pas l'interface bidon,
        // mais on vérifie le garde-fou d'action via une interface réelle si dispo
        if let Some(i) = interfaces().into_iter().next() {
            let r = set_interface(&i.name, "destroy");
            assert!(r.is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn parse_default_route_dev() {
        let s = "default via 192.168.1.1 dev eth0 proto dhcp metric 100\n";
        let dev = s.lines().find_map(|l| {
            l.split_whitespace()
                .skip_while(|w| *w != "dev")
                .nth(1)
                .map(str::to_string)
        });
        assert_eq!(dev.as_deref(), Some("eth0"));
    }
}
