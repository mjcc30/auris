//! Configuration: fichier TOML dans le data-dir + surcharges env (AURIS_*).

use std::path::PathBuf;

use auris_core::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Port de l'interface web.
    pub port: u16,
    /// Adresse d'ecoute (0.0.0.0 = toutes les interfaces, acces depuis le LAN).
    pub host: String,
    /// Repertoire de donnees (bibliotheque, config). Surcharge: --data-dir / AURIS_DATA_DIR.
    pub data_dir: Option<PathBuf>,
    /// DAC a utiliser (nom exact depuis /api/audio/devices). None = defaut.
    pub audio_device: Option<String>,
    /// Volume initial (0-100).
    pub volume: u8,
    /// Verification/mise a jour automatique depuis GitHub.
    pub auto_update: bool,
    pub repo_owner: String,
    pub repo_name: String,
    /// Sources musicales configurees (local, SMB, ...).
    pub sources: Vec<Source>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            port: 3000,
            host: "0.0.0.0".into(),
            data_dir: None,
            audio_device: None,
            volume: 80,
            auto_update: true,
            repo_owner: "mjcc30".into(),
            repo_name: "auris".into(),
            sources: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Repertoire de donnees effectif (config > env > ~/.local/share/auris).
    pub fn resolve_data_dir(&self) -> PathBuf {
        if let Some(d) = &self.data_dir {
            return d.clone();
        }
        if let Ok(d) = std::env::var("AURIS_DATA_DIR") {
            return PathBuf::from(d);
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("auris")
    }

    pub fn config_path(&self) -> PathBuf {
        self.resolve_data_dir().join("config.toml")
    }

    /// Charge la config depuis `<data_dir>/config.toml` si present, puis applique
    /// les surcharges d'environnement.
    pub fn load(data_dir_override: Option<PathBuf>) -> Self {
        let mut cfg = AppConfig::default();
        if let Some(d) = data_dir_override {
            cfg.data_dir = Some(d);
        }
        let path = cfg.config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match toml::from_str::<AppConfig>(&content) {
                    Ok(loaded) => {
                        // le data_dir du fichier ne sert que s'il est explicite
                        let mut merged = loaded;
                        if merged.data_dir.is_none() {
                            merged.data_dir = cfg.data_dir.clone();
                        }
                        cfg = merged;
                    }
                    Err(e) => {
                        eprintln!(
                            "config invalide ({}): {e} — config par defaut utilisee",
                            path.display()
                        );
                    }
                },
                Err(e) => eprintln!("lecture config {}: {e}", path.display()),
            }
        }
        // surcharges env
        if let Ok(v) = std::env::var("AURIS_PORT") {
            if let Ok(p) = v.parse() {
                cfg.port = p;
            }
        }
        if let Ok(v) = std::env::var("AURIS_HOST") {
            cfg.host = v;
        }
        if let Ok(v) = std::env::var("AURIS_DATA_DIR") {
            cfg.data_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("AURIS_DEVICE") {
            cfg.audio_device = Some(v);
        }
        if let Ok(v) = std::env::var("AURIS_VOLUME") {
            if let Ok(n) = v.parse() {
                cfg.volume = n;
            }
        }
        if let Ok(v) = std::env::var("AURIS_AUTO_UPDATE") {
            cfg.auto_update = matches!(v.as_str(), "1" | "true" | "yes" | "on");
        }
        cfg
    }

    /// Ecrit la config courante (pour que l'utilisateur puisse l'editer).
    pub fn save(&self) -> std::io::Result<()> {
        let dir = self.resolve_data_dir();
        std::fs::create_dir_all(&dir)?;
        let content =
            toml::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(self.config_path(), content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("auris-cfg-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn defauts_sains() {
        let c = AppConfig::default();
        assert_eq!(c.port, 3000);
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.volume, 80);
        assert!(c.auto_update);
        assert_eq!(c.repo_owner, "mjcc30");
        assert_eq!(c.repo_name, "auris");
        assert!(c.sources.is_empty());
    }

    #[test]
    fn save_puis_load_roundtrip() {
        let dir = tmp("rt");
        let mut c = AppConfig {
            data_dir: Some(dir.clone()),
            port: 8080,
            ..Default::default()
        };
        c.sources
            .push(auris_core::Source::new_local("nas", "/mnt/nas"));
        c.save().unwrap();
        assert!(dir.join("config.toml").exists());

        let loaded = AppConfig::load(Some(dir.clone()));
        assert_eq!(loaded.port, 8080);
        assert_eq!(loaded.sources.len(), 1);
        assert!(matches!(
            &loaded.sources[0].kind,
            auris_core::SourceKind::Local { path } if path == "/mnt/nas"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_sans_fichier_et_env() {
        // un seul test: les variables d'env sont partagees entre tests paralleles
        let dir = tmp("empty");
        let c = AppConfig::load(Some(dir.clone()));
        assert_eq!(c.port, 3000);
        assert_eq!(c.resolve_data_dir(), dir);

        let dir2 = tmp("env");
        unsafe { std::env::set_var("AURIS_DATA_DIR", &dir2) };
        let c = AppConfig::default();
        assert_eq!(c.resolve_data_dir(), dir2);
        unsafe { std::env::remove_var("AURIS_DATA_DIR") };
    }
}
