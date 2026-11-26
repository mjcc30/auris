//! Auris — lecteur audiophile cross-plateforme en un seul binaire Rust.
//! Licence AGPL v3. Voir README.md et docs/.

mod app;
mod cli;
mod config;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::Parser;

use cli::{Cli, Command, ServeArgs};

/// Journalisation vers stderr + fichier (quand un data-dir est connu).
struct DualMake {
    file: Option<Arc<Mutex<std::fs::File>>>,
}

struct DualWriter {
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl Write for DualWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(f) = &self.file {
            let _ = f.lock().unwrap().write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &self.file {
            f.lock().unwrap().flush()?;
        }
        std::io::stderr().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DualMake {
    type Writer = DualWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DualWriter {
            file: self.file.clone(),
        }
    }
}

/// Initialise tracing : stderr toujours, + `<data-dir>/logs/auris.log` en mode `serve`.
fn init_logging(cli: &Cli) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,auris=debug".into());
    let serve_args = match &cli.command {
        Some(Command::Serve(a)) => Some(a.clone()),
        None => Some(ServeArgs {
            port: None,
            host: None,
            data_dir: None,
            device: None,
            volume: None,
            no_auto_update: false,
            no_open: false,
        }),
        _ => None,
    };
    if let Some(args) = serve_args {
        let data_dir = args
            .data_dir
            .clone()
            .or_else(|| std::env::var("AURIS_DATA_DIR").ok().map(PathBuf::from))
            .unwrap_or_else(config::default_data_dir);
        let logs = data_dir.join("logs");
        if std::fs::create_dir_all(&logs).is_ok() {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(logs.join("auris.log"));
            if let Ok(f) = file {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(DualMake {
                        file: Some(Arc::new(Mutex::new(f))),
                    })
                    .init();
                return;
            }
        }
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_logging(&cli);

    match cli.command {
        Some(Command::Serve(args)) => app::run_serve(args).await,
        Some(Command::Update) => app::run_update(true),
        Some(Command::AudioDevices) => {
            app::list_audio_devices();
            Ok(())
        }
        Some(Command::AudioDopSetup { rate, device }) => app::setup_dop_format(rate, device),
        Some(Command::UsbDevices) => {
            app::list_usb_devices();
            Ok(())
        }
        Some(Command::Shortcut) => app::install_shortcut(),
        Some(Command::Version) => {
            println!("auris {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            app::run_serve(ServeArgs {
                port: None,
                host: None,
                data_dir: None,
                device: None,
                volume: None,
                no_auto_update: false,
                no_open: false,
            })
            .await
        }
    }
}
