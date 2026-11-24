//! Wiring de l'application: config -> sources -> bibliothèque -> moteur -> web.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};

use auris_library::Library;
use auris_player::{AudioOutput, PlayerEngine};
use auris_sources::SourcesManager;
use auris_updater::{Updater, UpdaterConfig};
use auris_web::{serve, AppState};

use crate::cli::ServeArgs;
use crate::config::AppConfig;

/// Lance le serveur complet (bloquant jusqu'au Ctrl-C).
pub async fn run_serve(args: ServeArgs) -> Result<()> {
    let mut config = AppConfig::load(args.data_dir.clone());

    // surcharges CLI > env > fichier
    if let Some(p) = args.port {
        config.port = p;
    }
    if let Some(h) = args.host {
        config.host = h;
    }
    if let Some(d) = args.device {
        config.audio_device = Some(d);
    }
    if let Some(v) = args.volume {
        config.volume = v.min(100);
    }
    if args.no_auto_update {
        config.auto_update = false;
    }

    let data_dir = config.resolve_data_dir();
    std::fs::create_dir_all(&data_dir).context("création du data-dir")?;
    if !config.config_path().exists() {
        config.save().context("écriture de la config par défaut")?;
        tracing::info!("config créée: {}", config.config_path().display());
    }
    // le fichier contient potentiellement des mots de passe SMB
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(config.config_path(), std::fs::Permissions::from_mode(0o600));
    }

    tracing::info!("data-dir: {}", data_dir.display());

    let sources = Arc::new(SourcesManager::new(config.sources.clone())?);
    tracing::info!("{} source(s) configurée(s)", sources.list().len());

    let library = Arc::new(Library::open(&data_dir, sources.clone())?);
    tracing::info!("bibliothèque: {} pistes indexées", library.track_count());

    let engine = PlayerEngine::start(sources.clone(), config.audio_device.clone(), config.volume);

    let updater = Arc::new(Updater::new(UpdaterConfig {
        repo_owner: config.repo_owner.clone(),
        repo_name: config.repo_name.clone(),
        bin_name: "auris".into(),
        current_version: env!("CARGO_PKG_VERSION").into(),
        auto_check: config.auto_update,
    }));
    updater.spawn_background_check();

    let state = AppState {
        engine,
        library,
        sources,
        updater,
        started_at: chrono::Utc::now().timestamp(),
    };

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .context("adresse d'écoute invalide (host:port)")?;

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("arrêt demandé (Ctrl-C)...");
    };
    tokio::select! {
        r = serve(state, addr) => r.context("serveur web"),
        _ = shutdown => { tracing::info!("au revoir"); Ok(()) }
    }
}

/// Affiche les périphériques de sortie détectés.
pub fn list_audio_devices() {
    let devices = AudioOutput::devices();
    if devices.is_empty() {
        println!("Aucun périphérique de sortie audio détecté.");
        return;
    }
    for d in devices {
        println!(
            "{} {}\n    taux: {:?}\n    I24 (DoP/DSD): {}  f32: {}  canaux max: {}",
            if d.is_default { "*" } else { " " },
            d.name,
            d.sample_rates,
            d.supports_i24,
            d.supports_f32,
            d.max_channels
        );
    }
}

/// Vérifie et applique une mise à jour.
pub fn run_update(auto: bool) -> Result<()> {
    let updater = Updater::new(UpdaterConfig::default());
    let info = updater.check().context("vérification des mises à jour")?;
    println!(
        "version courante: {} — dernière release: {} ({})",
        info.current_version,
        info.latest_version,
        if info.update_available {
            "mise à jour disponible"
        } else {
            "à jour"
        }
    );
    if info.update_available && !auto {
        println!("Lancement avec --auto pour appliquer la mise à jour.");
        return Ok(());
    }
    if info.update_available {
        let res = updater.update().context("application de la mise à jour")?;
        println!(
            "→ {} (redémarrez Auris pour utiliser la nouvelle version)",
            res.latest_version
        );
    }
    Ok(())
}
