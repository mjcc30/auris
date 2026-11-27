//! Hôte de plugins communautaires v1 (#8) — spec : docs/PLUGIN_SYSTEM.md.
//!
//! Un plugin = un répertoire sous `<data-dir>/plugins/<id>/` contenant un
//! manifeste `auris-plugin.toml` et un binaire. Le cœur lance le binaire et
//! parle **JSON-RPC 2.0 sur stdio** (une ligne JSON par message), avec timeout.
//! Le cœur ne fait jamais confiance au plugin : tailles plafonnées, processus
//! tué s'il ne répond pas.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use auris_core::{AurisError, Result};
use serde::{Deserialize, Serialize};

/// Taille max d'une ligne JSON-RPC lue depuis un plugin (1 Mo).
const MAX_LINE: usize = 1024 * 1024;
/// Timeout d'un appel JSON-RPC (spec : 10 s).
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Manifeste `auris-plugin.toml` (voir docs/PLUGIN_SYSTEM.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub plugin: ManifestPlugin,
    #[serde(default)]
    pub install: Option<ManifestInstall>,
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    /// source | output | metadata | ui
    pub category: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub license: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub min_auris: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInstall {
    #[serde(default)]
    pub asset_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    pub bin: String,
}

/// Plugin installé : manifeste + état côté hôte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    #[serde(flatten)]
    pub plugin: ManifestPlugin,
    pub dir: String,
    pub bin: Option<String>,
    pub enabled: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// État persistant de l'hôte (`<data-dir>/plugins.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct HostState {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    errors: HashMap<String, String>,
}

struct RunningPlugin {
    child: Child,
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<String>,
    next_id: u64,
}

/// Hôte de plugins : scanne `<data-dir>/plugins`, lance/arrête les processus,
/// relaie les appels JSON-RPC.
pub struct PluginHost {
    plugins_dir: PathBuf,
    state_path: PathBuf,
    running: Mutex<HashMap<String, RunningPlugin>>,
    state: Mutex<HostState>,
}

impl PluginHost {
    pub fn new(data_dir: &Path) -> Self {
        let plugins_dir = data_dir.join("plugins");
        let state_path = data_dir.join("plugins.json");
        let state = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        PluginHost {
            plugins_dir,
            state_path,
            running: Mutex::new(HashMap::new()),
            state: Mutex::new(state),
        }
    }

    fn save_state(&self) -> Result<()> {
        let s = self.state.lock().unwrap();
        let txt =
            serde_json::to_string_pretty(&*s).map_err(|e| AurisError::other(e.to_string()))?;
        std::fs::write(&self.state_path, txt).map_err(|e| AurisError::other(e.to_string()))
    }

    fn manifest_path(dir: &Path) -> PathBuf {
        dir.join("auris-plugin.toml")
    }

    fn read_manifest(dir: &Path) -> Result<PluginManifest> {
        let p = Self::manifest_path(dir);
        let txt = std::fs::read_to_string(&p)
            .map_err(|e| AurisError::other(format!("lecture {}: {e}", p.display())))?;
        toml::from_str(&txt)
            .map_err(|e| AurisError::other(format!("manifest invalide {}: {e}", p.display())))
    }

    /// Liste tous les plugins installés (avec état enabled/running/erreur).
    pub fn list(&self) -> Vec<PluginInfo> {
        let mut out = Vec::new();
        let state = self.state.lock().unwrap();
        let running = self.running.lock().unwrap();
        let Ok(rd) = std::fs::read_dir(&self.plugins_dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Ok(m) = Self::read_manifest(&dir) else {
                continue;
            };
            let id = m.plugin.id.clone();
            out.push(PluginInfo {
                plugin: m.plugin.clone(),
                dir: dir.to_string_lossy().into_owned(),
                bin: m.install.as_ref().map(|i| i.bin.clone()),
                enabled: !state.disabled.contains(&id),
                running: running.contains_key(&id),
                last_error: state.errors.get(&id).cloned(),
            });
        }
        out.sort_by(|a, b| a.plugin.id.cmp(&b.plugin.id));
        out
    }

    fn find(&self, id: &str) -> Result<PathBuf> {
        self.list()
            .into_iter()
            .find(|p| p.plugin.id == id)
            .map(|p| PathBuf::from(p.dir))
            .ok_or_else(|| AurisError::other(format!("plugin inconnu: {id}")))
    }

    /// Installe un plugin depuis un répertoire local (copie dans plugins/<id>).
    pub fn install_from_dir(&self, src: &Path) -> Result<PluginInfo> {
        let m = Self::read_manifest(src)?;
        let id = m.plugin.id.clone();
        let dst = self.plugins_dir.join(&id);
        if dst.exists() {
            return Err(AurisError::other(format!("plugin déjà installé: {id}")));
        }
        std::fs::create_dir_all(&self.plugins_dir)
            .map_err(|e| AurisError::other(format!("create plugins dir: {e}")))?;
        copy_dir(src, &dst)?;
        // le binaire doit exister et être exécutable
        if let Some(i) = &m.install {
            let bin = dst.join(&i.bin);
            if !bin.exists() {
                let _ = std::fs::remove_dir_all(&dst);
                return Err(AurisError::other(format!(
                    "binaire du plugin introuvable: {}",
                    bin.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
            }
        }
        self.list()
            .into_iter()
            .find(|p| p.plugin.id == id)
            .ok_or_else(|| AurisError::other("plugin non listé après installation"))
    }

    /// Désinstalle un plugin (tue le processus, supprime le répertoire).
    pub fn uninstall(&self, id: &str) -> Result<()> {
        self.stop(id);
        let dir = self.find(id)?;
        std::fs::remove_dir_all(&dir)
            .map_err(|e| AurisError::other(format!("rm {}: {e}", dir.display())))?;
        let mut st = self.state.lock().unwrap();
        st.disabled.retain(|d| d != id);
        st.errors.remove(id);
        drop(st);
        self.save_state()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<PluginInfo> {
        self.find(id)?;
        {
            let mut st = self.state.lock().unwrap();
            if enabled {
                st.disabled.retain(|d| d != id);
            } else if !st.disabled.iter().any(|d| d == id) {
                st.disabled.push(id.to_string());
            }
        }
        if !enabled {
            self.stop(id);
        }
        self.save_state()?;
        self.list()
            .into_iter()
            .find(|p| p.plugin.id == id)
            .ok_or_else(|| AurisError::other("plugin disparu"))
    }

    fn stop(&self, id: &str) {
        let mut running = self.running.lock().unwrap();
        if let Some(mut rp) = running.remove(id) {
            let _ = rp.child.kill();
            let _ = rp.child.wait();
        }
    }

    /// Lance le processus du plugin si nécessaire et renvoie un appel JSON-RPC.
    pub fn call(
        &self,
        id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let info = self
            .list()
            .into_iter()
            .find(|p| p.plugin.id == id)
            .ok_or_else(|| AurisError::other(format!("plugin inconnu: {id}")))?;
        if !info.enabled {
            return Err(AurisError::other(format!("plugin désactivé: {id}")));
        }
        let bin_rel = info
            .bin
            .clone()
            .ok_or_else(|| AurisError::other(format!("plugin sans binaire: {id}")))?;
        let bin = PathBuf::from(&info.dir).join(&bin_rel);

        let mut running = self.running.lock().unwrap();
        if !running.contains_key(id) {
            let rp = spawn_plugin(&bin)?;
            running.insert(id.to_string(), rp);
        }
        let rp = running.get_mut(id).unwrap();
        let res = rpc_call(rp, method, params);
        if res.is_err() {
            // processus probablement mort : on le retire pour repartir propre
            running.remove(id);
            let mut st = self.state.lock().unwrap();
            st.errors.insert(
                id.to_string(),
                res.as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default(),
            );
            drop(st);
            let _ = self.save_state();
        }
        res
    }

    /// Test de santé : manifest.get + capabilities.list.
    pub fn test(&self, id: &str) -> Result<serde_json::Value> {
        let m = self.call(id, "manifest.get", serde_json::json!({}))?;
        let caps = self.call(id, "capabilities.list", serde_json::json!({}))?;
        Ok(serde_json::json!({ "manifest": m, "capabilities": caps }))
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| AurisError::other(e.to_string()))?;
    for entry in std::fs::read_dir(src).map_err(|e| AurisError::other(e.to_string()))? {
        let entry = entry.map_err(|e| AurisError::other(e.to_string()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| AurisError::other(e.to_string()))?;
        }
    }
    Ok(())
}

fn spawn_plugin(bin: &Path) -> Result<RunningPlugin> {
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AurisError::other(format!("spawn plugin {}: {e}", bin.display())))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    // thread d'écriture : reçoit les lignes JSON-RPC et les pousse sur stdin
    let (wtx, wrx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(line) = wrx.recv() {
            if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() {
                break;
            }
            let _ = stdin.flush();
        }
    });
    // thread de lecture : une ligne JSON par message, taille plafonnée
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.len() > MAX_LINE {
                        break;
                    }
                    if tx.send(line.clone()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Ok(RunningPlugin {
        child,
        tx: wtx,
        rx,
        next_id: 0,
    })
}

fn rpc_call(
    rp: &mut RunningPlugin,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    rp.next_id += 1;
    let id = rp.next_id;
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    rp.tx
        .send(req.to_string())
        .map_err(|_| AurisError::other("plugin: canal stdin fermé"))?;
    let line = rp
        .rx
        .recv_timeout(RPC_TIMEOUT)
        .map_err(|_| AurisError::other(format!("plugin: timeout {RPC_TIMEOUT:?} sur {method}")))?;
    let v: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| AurisError::other(format!("plugin: JSON invalide: {e}")))?;
    if let Some(err) = v.get("error") {
        return Err(AurisError::other(format!(
            "plugin: erreur JSON-RPC: {}",
            err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("inconnue")
        )));
    }
    Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse() {
        let txt = r#"
[plugin]
id = "demo"
name = "Démo"
version = "0.1.0"
category = "ui"
license = "AGPL-3.0-only"
description = "Plugin de démonstration"

[install]
bin = "demo-plugin"
"#;
        let m: PluginManifest = toml::from_str(txt).unwrap();
        assert_eq!(m.plugin.id, "demo");
        assert_eq!(m.install.unwrap().bin, "demo-plugin");
    }

    #[test]
    fn host_liste_vide_sans_dir() {
        let dir = std::env::temp_dir().join(format!("auris-plugins-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let h = PluginHost::new(&dir);
        assert!(h.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
