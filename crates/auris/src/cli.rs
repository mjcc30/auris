//! Interface en ligne de commande (clap).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "auris", version, about = "Lecteur audiophile cross-plateforme (style Volumio) en Rust", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Lance le serveur (interface web + lecteur). Commande par defaut.
    Serve(ServeArgs),
    /// Verifie et applique une mise a jour depuis GitHub.
    Update,
    /// Liste les peripheriques de sortie audio (DAC) detectes.
    AudioDevices,
    /// Sur Windows, ecrit le « Default Format » WASAPI du DAC en PCM 24 bits
    /// a la porteuse DoP pour que les .dsf passent en DoP natif. A lancer au
    /// repos (aucune lecture en cours), puis rebrancher le DAC ou redemarrer
    /// le service audio Windows pour que le moteur adopte le format.
    AudioDopSetup {
        /// Porteuse DoP en Hz (defaut 176400 = DSD64).
        #[arg(long, default_value_t = 176_400)]
        rate: u32,
        /// Nom du DAC (voir `auris audio-devices`) [defaut: sortie par defaut].
        #[arg(long)]
        device: Option<String>,
    },
    /// Liste les disques amovibles (cles USB, disques externes) detectes.
    UsbDevices,
    /// Installe un raccourci Bureau + un lancement au demarrage de Windows.
    Shortcut,
    /// Affiche la version.
    Version,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ServeArgs {
    /// Port de l'interface web [env: AURIS_PORT=] [default: 3000]
    #[arg(long, env = "AURIS_PORT")]
    pub port: Option<u16>,
    /// Adresse d'ecoute [env: AURIS_HOST=] [default: 0.0.0.0]
    #[arg(long, env = "AURIS_HOST")]
    pub host: Option<String>,
    /// Repertoire de donnees (bibliotheque, config) [env: AURIS_DATA_DIR=]
    #[arg(long, env = "AURIS_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Nom du DAC a utiliser (voir `auris audio-devices`) [env: AURIS_DEVICE=]
    #[arg(long, env = "AURIS_DEVICE")]
    pub device: Option<String>,
    /// Volume initial 0-100 [env: AURIS_VOLUME=] [default: 80]
    #[arg(long, env = "AURIS_VOLUME")]
    pub volume: Option<u8>,
    /// Desactive la mise a jour automatique [env: AURIS_AUTO_UPDATE=]
    #[arg(long, env = "AURIS_AUTO_UPDATE")]
    pub no_auto_update: bool,
    /// N'ouvre pas le navigateur automatiquement au demarrage [env: AURIS_NO_OPEN=]
    #[arg(long, env = "AURIS_NO_OPEN")]
    pub no_open: bool,
}
