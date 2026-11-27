//! Wiring de l'application: config -> sources -> bibliothèque -> moteur -> web.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use auris_library::Library;
use auris_player::{
    all_mix_formats, device_mix_format, ensure_dop_carrier, AudioOutput, PlayerEngine,
};
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

    // Instance unique : si un Auris tourne deja sur ce port, on ouvre juste
    // le navigateur dessus au lieu de demarrer un second serveur.
    let url = local_url(&config.host, config.port);
    if already_running(config.port) {
        println!("Auris est déjà en cours d'exécution : {url}");
        if !args.no_open {
            open_browser(&url);
        }
        return Ok(());
    }

    let data_dir = config.resolve_data_dir();
    std::fs::create_dir_all(&data_dir).context("création du data-dir")?;
    if !config.config_path().exists() {
        // Sources gratuites intégrées par défaut à la première exécution (#84, #85) :
        // Internet Archive (audio libre) + Deezer (extraits 30 s), zéro compte requis.
        for (id, name, kind) in [
            (
                "archive",
                "Internet Archive",
                auris_core::SourceKind::Archive,
            ),
            (
                "deezer",
                "Deezer · extraits 30 s",
                auris_core::SourceKind::Deezer,
            ),
        ] {
            if !config.sources.iter().any(|s| s.id == id) {
                config.sources.push(auris_core::Source {
                    id: id.into(),
                    name: name.into(),
                    kind,
                    enabled: true,
                });
            }
        }
        // Jamendo (CC) : source ajoutée désactivée — elle requiert un client_id perso
        // (gratuit, developer.jamendo.com) à renseigner dans config.toml (#86).
        if !config.sources.iter().any(|s| s.id == "jamendo") {
            config.sources.push(auris_core::Source {
                id: "jamendo".into(),
                name: "Jamendo · CC (client_id requis)".into(),
                kind: auris_core::SourceKind::Jamendo { client_id: None },
                enabled: false,
            });
        }
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

    let settings = Arc::new(RwLock::new(config.settings.clone()));

    let engine = PlayerEngine::start(
        sources.clone(),
        config.audio_device.clone(),
        config.volume,
        config.audio_exclusive,
    );
    engine.apply_playback(&config.settings.playback);
    // Normalisation loudness R128 (#37) : gain statique par piste, lu dans le
    // cache de la bibliothèque à chaque démarrage de piste.
    {
        let lib = library.clone();
        let st = settings.clone();
        engine.set_loudness_lookup(Some(Arc::new(move |uri: &str| {
            let (on, target) = {
                let g = st.read().unwrap();
                (g.playback.loudness, g.playback.loudness_target_lufs)
            };
            if !on {
                return None;
            }
            let track = lib.get_by_uri(uri)?;
            let lufs = lib.loudness_get(&track.id)?;
            Some(auris_player::gain_for_target(lufs, target))
        })));
    }
    // Mode de sortie persistant (« local » / « browser ») : repris tel quel au
    // demarrage pour ne pas retomber sur le defaut Windows.
    if let Some(mode_str) = &config.output_mode {
        if let Some(m) = auris_core::OutputMode::parse(mode_str) {
            engine.set_output_mode(m);
        } else {
            tracing::warn!("output_mode inconnu dans config.toml: {mode_str}");
        }
    }

    let updater = Arc::new(Updater::new(UpdaterConfig {
        repo_owner: config.repo_owner.clone(),
        repo_name: config.repo_name.clone(),
        bin_name: "auris".into(),
        current_version: env!("CARGO_PKG_VERSION").into(),
        auto_check: config.auto_update,
    }));
    updater.spawn_background_check();

    // persistance des sources dans config.toml a chaque modification via l'API
    let sink_data_dir = data_dir.clone();
    let sources_sink = Arc::new(move |list: Vec<auris_core::Source>| {
        let mut cfg = AppConfig::load(Some(sink_data_dir.clone()));
        cfg.sources = list;
        if let Err(e) = cfg.save() {
            tracing::warn!("persistance des sources: {e}");
        }
    });

    // persistance des réglages dans config.toml a chaque modification via l'API
    let sink_data_dir2 = data_dir.clone();
    let settings_sink = Arc::new(move |st: &auris_core::Settings| {
        let mut cfg = AppConfig::load(Some(sink_data_dir2.clone()));
        cfg.settings = st.clone();
        if let Err(e) = cfg.save() {
            tracing::warn!("persistance des réglages: {e}");
        }
    });

    // persistance du DAC choisi + du mode de sortie dans config.toml, pour que
    // la sortie ne suive pas le peripherique par defaut de Windows au redemarrage
    let sink_data_dir3 = data_dir.clone();
    let player_sink = Arc::new(
        move |device: Option<String>, mode: auris_core::OutputMode| {
            let mut cfg = AppConfig::load(Some(sink_data_dir3.clone()));
            cfg.audio_device = device;
            cfg.output_mode = Some(mode.as_str().to_string());
            if let Err(e) = cfg.save() {
                tracing::warn!("persistance de la sortie audio: {e}");
            }
        },
    );

    // persistance du mode exclusif (bit-perfect) dans config.toml
    let sink_data_dir4 = data_dir.clone();
    let exclusive_sink = Arc::new(move |on: bool| {
        let mut cfg = AppConfig::load(Some(sink_data_dir4.clone()));
        cfg.audio_exclusive = on;
        if let Err(e) = cfg.save() {
            tracing::warn!("persistance du mode exclusif: {e}");
        }
    });

    // après un rip CD réussi, rescan de la bibliothèque (#27)
    let rip_lib = library.clone();
    let ripcd = {
        let hub = auris_web::ripcd::RipCdHub::new();
        hub.set_on_finished(move |ok| {
            if ok {
                let lib = rip_lib.clone();
                std::thread::spawn(move || {
                    let _ = lib.scan_all();
                });
            }
        });
        hub
    };

    let state = AppState {
        engine,
        library,
        sources,
        updater,
        started_at: chrono::Utc::now().timestamp(),
        sources_sink: Some(sources_sink),
        settings,
        settings_sink: Some(settings_sink),
        player_sink: Some(player_sink),
        exclusive_sink: Some(exclusive_sink),
        device_lister: None,
        ext_http: None,
        data_dir: data_dir.clone(),
        power_control: config.power_control,
        power_ops: None,
        log_path: Some(data_dir.join("logs").join("auris.log")),
        alarms: Arc::new(std::sync::Mutex::new(auris_core::AlarmState::default())),
        mb_cache: Arc::new(std::sync::Mutex::new(None)),
        bound_addr: Arc::new(RwLock::new(None)),
        multiroom: Arc::new(auris_web::MultiroomHub::new(
            data_dir.clone(),
            auris_web::peer_id(&data_dir),
        )),
        plugins: Arc::new(auris_plugins::PluginHost::new(&data_dir)),
        socketio: auris_web::socketio::SocketIoHub::new(),
        ripcd,
        spotify: auris_web::spotify::SpotHub::new(),
    };

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .context("adresse d'écoute invalide (host:port)")?;

    // Spotify Connect : démarrage automatique si activé dans les réglages (#24)
    if state.settings.read().unwrap().spotify.enabled {
        *state.bound_addr.write().unwrap() = Some(addr);
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = st.spotify.start(&st).await {
                tracing::warn!("Spotify Connect: démarrage auto impossible: {e}");
            }
        });
    }

    // Ouverture automatique du navigateur une fois le serveur prêt.
    if !args.no_open {
        let u = url.clone();
        let port = config.port;
        std::thread::spawn(move || {
            for _ in 0..150 {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    open_browser(&u);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
    }

    // Serveur MPD (clients tiers, #80) : port TCP dédié, 0 = désactivé.
    if config.mpd_port > 0 {
        let st = state.clone();
        let mpd_port = config.mpd_port;
        tokio::spawn(async move {
            if let Err(e) = auris_web::mpd::serve(st, mpd_port).await {
                tracing::warn!("serveur MPD: {e}");
            }
        });
    }

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("arrêt demandé (Ctrl-C)...");
    };
    tokio::select! {
        r = serve(state, addr) => r.context("serveur web"),
        _ = shutdown => { tracing::info!("au revoir"); Ok(()) }
    }
}

/// Affiche les disques amovibles (USB) détectés, pour créer une source `usb`.
pub fn list_usb_devices() {
    let mounts = auris_sources::discover_removable();
    if mounts.is_empty() {
        println!("Aucun disque amovible détecté (branchez une clé USB / un disque externe).");
        return;
    }
    for m in mounts {
        println!("{}  ->  {}", m.label, m.path);
    }
    println!("\nAjoutez une source USB depuis l'interface web (Sources & réglages > USB).");
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
    // Sous Windows, le mix format WASAPI de chaque endpoint est ce que le
    // moteur audio envoie réellement au DAC en mode partagé (cpal) : un flux
    // DoP doit être à cette cadence pour arriver intact (marqueurs DoP).
    let mix_formats = all_mix_formats();
    if !mix_formats.is_empty() {
        println!("Mix formats WASAPI (Default Format):");
        for (name, mix) in mix_formats {
            println!("    {name}: {mix}");
        }
    }
}

/// Écrit le « Default Format » WASAPI du DAC en PCM 24 bits à la porteuse DoP
/// (Windows uniquement; no-op ailleurs). À lancer au repos — aucune lecture
/// en cours — pour que l'écriture persiste dans le registre; le moteur audio
/// l'adopte au prochain rebranchement du DAC / redémarrage du service.
pub fn setup_dop_format(rate: u32, device: Option<String>) -> Result<()> {
    let name = device.as_deref();
    match ensure_dop_carrier(name, rate) {
        Ok(true) => println!(
            "Default Format ecrit en 24 bits / {rate} Hz pour {name:?} — rebranchez le DAC USB ou redemarrez le service audio Windows pour l'appliquer."
        ),
        Ok(false) => println!("Default Format deja en {rate} Hz / 24 bits PCM (ou plateforme non concernee)."),
        Err(e) => return Err(anyhow::anyhow!(e)),
    }
    if let Some(mix) = device_mix_format(name) {
        println!("mix format actuel du device: {mix}");
    }
    Ok(())
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

// ---------- lancement facilite (double-clic, instance unique) ----------

/// URL locale joignable depuis la machine hôte (0.0.0.0 -> 127.0.0.1).
pub fn local_url(host: &str, port: u16) -> String {
    let h = match host {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        other => other,
    };
    format!("http://{h}:{port}/")
}

/// `true` si un serveur Auris repond deja sur 127.0.0.1:port.
pub fn already_running(port: u16) -> bool {
    use std::io::{Read, Write};
    let mut stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
    let req = b"GET /api/system/info HTTP/1.0\r\nHost: localhost\r\n\r\n";
    if stream.write_all(req).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("Auris")
}

/// (programme, arguments) pour ouvrir une URL dans le navigateur par defaut.
pub fn browser_cmd(url: &str) -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        (
            "cmd".into(),
            vec!["/C".into(), "start".into(), "".into(), url.into()],
        )
    } else if cfg!(target_os = "macos") {
        ("open".into(), vec![url.into()])
    } else {
        ("xdg-open".into(), vec![url.into()])
    }
}

/// Ouvre l'interface web dans le navigateur par defaut (sans bloquer).
pub fn open_browser(url: &str) {
    let (prog, args) = browser_cmd(url);
    let _ = std::process::Command::new(prog).args(args).spawn();
}

/// Contenu du .vbs de demarrage automatique (fenetre cachee).
pub fn vbs_content(exe: &str) -> String {
    format!("CreateObject(\"Wscript.Shell\").Run \"\"\"{exe}\"\" serve\", 0, False\r\n")
}

/// Script PowerShell creant le raccourci Bureau (.lnk).
pub fn ps_lnk_script(exe: &str, lnk: &str) -> String {
    format!(
        "$s = New-Object -ComObject WScript.Shell; \
         $l = $s.CreateShortcut('{lnk}'); \
         $l.TargetPath = '{exe}'; \
         $l.Arguments = 'serve'; \
         $l.WorkingDirectory = '{work}'; \
         $l.Save()",
        work = std::path::Path::new(exe)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

/// Installe un raccourci Bureau + un lancement au demarrage (Windows).
/// Sur les autres OS, affiche la procedure manuelle.
pub fn install_shortcut() -> Result<()> {
    let exe = std::env::current_exe().context("chemin du binaire courant")?;
    let exe_s = exe.to_string_lossy().into_owned();
    if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("variable APPDATA introuvable")?;
        let startup = std::path::Path::new(&appdata)
            .join("Microsoft/Windows/Start Menu/Programs/Startup/Auris.vbs");
        std::fs::write(&startup, vbs_content(&exe_s)).context("ecriture du .vbs de demarrage")?;
        let lnk = std::env::var("USERPROFILE")
            .map(|p| format!("{p}/Desktop/Auris.lnk"))
            .unwrap_or_else(|_| "Auris.lnk".to_string());
        let ps = ps_lnk_script(&exe_s, &lnk);
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .status()
            .context("lancement de PowerShell")?;
        if status.success() {
            println!("Raccourci Bureau cree: {lnk}");
        } else {
            println!("Raccourci Bureau non cree (PowerShell a echoue) — script:\n{ps}");
        }
        println!(
            "Lancement au demarrage de Windows installe: {}",
            startup.display()
        );
        println!("Double-cliquez sur le raccourci Bureau : le navigateur s'ouvre automatiquement.");
    } else {
        println!("Raccourci automatique non supporte sur cet OS. Procedure manuelle:");
        println!("  - lancez `{exe_s} serve` (le navigateur s'ouvre tout seul),");
        println!(
            "  - creez un raccourci vers ce binaire dans votre session (systemd, .desktop, etc.)."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn local_url_remplace_wildcard_par_loopback() {
        assert_eq!(local_url("0.0.0.0", 3000), "http://127.0.0.1:3000/");
        assert_eq!(local_url("::", 8080), "http://127.0.0.1:8080/");
        assert_eq!(local_url("192.168.1.10", 3000), "http://192.168.1.10:3000/");
    }

    #[test]
    fn already_running_detecte_auris_ou_pas() {
        // port ferme -> false
        assert!(!already_running(1));
        // serveur HTTP qui repond "Auris" -> true
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"name\":\"Auris\"}"
                );
            }
        });
        assert!(already_running(port));
        // serveur HTTP qui ne repond pas "Auris" -> false
        let listener2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port2 = listener2.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener2.incoming().flatten() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n{\"name\":\"autre\"}");
            }
        });
        assert!(!already_running(port2));
    }

    #[test]
    fn browser_cmd_contient_l_url() {
        let (prog, args) = browser_cmd("http://127.0.0.1:3000/");
        assert!(!prog.is_empty());
        assert!(args.contains(&"http://127.0.0.1:3000/".to_string()));
    }

    #[test]
    fn vbs_et_lnk_contiennent_le_chemin_exe() {
        let vbs = vbs_content("C:\\apps\\auris.exe");
        assert!(vbs.contains("C:\\apps\\auris.exe"));
        assert!(vbs.contains("serve"));
        let ps = ps_lnk_script("C:\\apps\\auris.exe", "C:\\Users\\x\\Desktop\\Auris.lnk");
        assert!(ps.contains("TargetPath = 'C:\\apps\\auris.exe'"));
        assert!(ps.contains("Auris.lnk"));
        assert!(ps.contains("serve"));
    }
}
