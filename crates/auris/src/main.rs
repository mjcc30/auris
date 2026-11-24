//! Auris — lecteur audiophile cross-plateforme en un seul binaire Rust.
//! Licence AGPL v3. Voir README.md et docs/.

mod app;
mod cli;
mod config;

use clap::Parser;

use cli::{Cli, Command, ServeArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,auris=debug".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Serve(args)) => app::run_serve(args).await,
        Some(Command::Update) => app::run_update(true),
        Some(Command::AudioDevices) => {
            app::list_audio_devices();
            Ok(())
        }
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
            })
            .await
        }
    }
}
