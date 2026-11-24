//! Mise a jour automatique du binaire depuis les releases GitHub (mjcc30/auris).
//!
//! - `check()` interroge l'API GitHub sans rien installer.
//! - `update()` telecharge et remplace le binaire courant (self_update).
//! - `spawn_background_check()` verifie toutes les 6 h si active.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use auris_core::UpdateInfo;

/// Configuration du depot de releases.
pub struct UpdaterConfig {
    pub repo_owner: String,
    pub repo_name: String,
    pub bin_name: String,
    pub current_version: String,
    /// Active la verification periodique en arriere-plan.
    pub auto_check: bool,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        UpdaterConfig {
            repo_owner: "mjcc30".into(),
            repo_name: "auris".into(),
            bin_name: "auris".into(),
            current_version: env!("CARGO_PKG_VERSION").into(),
            auto_check: true,
        }
    }
}

pub struct Updater {
    pub config: UpdaterConfig,
    /// Dernier resultat de verification partage avec l'API.
    pub last_check: Arc<Mutex<Option<UpdateInfo>>>,
}

impl Updater {
    pub fn new(config: UpdaterConfig) -> Self {
        Updater {
            config,
            last_check: Arc::new(Mutex::new(None)),
        }
    }

    fn target() -> String {
        let arch = std::env::consts::ARCH;
        let os = match std::env::consts::OS {
            "linux" => "unknown-linux-gnu",
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            other => other,
        };
        format!("{arch}-{os}")
    }

    fn release_page_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/releases/latest",
            self.config.repo_owner, self.config.repo_name
        )
    }

    fn make_update(&self) -> Result<self_update::backends::github::Update> {
        Ok(self_update::backends::github::Update::configure()
            .repo_owner(&self.config.repo_owner)
            .repo_name(&self.config.repo_name)
            .bin_name(&self.config.bin_name)
            .current_version(&self.config.current_version)
            .target(Self::target())
            .build()?)
    }

    /// Interroge GitHub: une mise a jour est-elle disponible ?
    pub fn check(&self) -> Result<UpdateInfo> {
        let upd = self.make_update()?;
        let latest = upd.get_latest_release()?;
        let latest_version = latest
            .latest()
            .map(|r| r.version().to_string())
            .unwrap_or_default();
        let cur = self.config.current_version.clone();
        let available = semver::Version::parse(latest_version.trim_start_matches('v'))
            .ok()
            .zip(semver::Version::parse(cur.trim_start_matches('v')).ok())
            .map(|(l, c)| l > c)
            .unwrap_or(false);
        let info = UpdateInfo {
            current_version: cur,
            latest_version,
            update_available: available,
            release_url: Some(self.release_page_url()),
            checked_at: chrono::Utc::now().timestamp(),
        };
        *self.last_check.lock().unwrap() = Some(info.clone());
        Ok(info)
    }

    pub fn last_check(&self) -> Option<UpdateInfo> {
        self.last_check.lock().unwrap().clone()
    }

    /// Telecharge et applique la mise a jour (remplace le binaire courant).
    /// Necessite les droits d'ecriture sur le binaire; redemarrage requis ensuite.
    pub fn update(&self) -> Result<UpdateInfo> {
        let upd = self.make_update()?;
        let status = upd.update()?;
        let info = UpdateInfo {
            current_version: self.config.current_version.clone(),
            latest_version: status.version().to_string(),
            update_available: false,
            release_url: Some(self.release_page_url()),
            checked_at: chrono::Utc::now().timestamp(),
        };
        *self.last_check.lock().unwrap() = Some(info.clone());
        if status.is_updated() {
            tracing::info!("mise a jour appliquee: {}", status.version());
        } else {
            tracing::info!("deja a jour: {}", status.version());
        }
        Ok(info)
    }

    /// Verifie en arriere-plan toutes les 6 h (si auto_check).
    pub fn spawn_background_check(self: &Arc<Self>) {
        if !self.config.auto_check {
            return;
        }
        let updater = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(6 * 3600));
            match updater.check() {
                Ok(i) if i.update_available => {
                    tracing::info!(
                        "mise a jour disponible: {} -> {}",
                        i.current_version,
                        i.latest_version
                    )
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("check mise a jour: {e}"),
            }
        });
    }
}
