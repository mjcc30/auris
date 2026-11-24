//! Test du binaire CLI: `auris version`, `--help`, `audio-devices`.

use std::process::Command;

#[test]
fn version_affiche_nom_et_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_auris"))
        .arg("version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("auris"), "sortie: {s}");
    assert!(s.contains("0.1.0"), "sortie: {s}");
}

#[test]
fn help_liste_les_commandes() {
    let out = Command::new(env!("CARGO_BIN_EXE_auris"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("serve"), "sortie: {s}");
    assert!(s.contains("update"), "sortie: {s}");
    assert!(s.contains("audio-devices"), "sortie: {s}");
}

#[test]
fn audio_devices_repond() {
    // sans DAC en CI la liste peut etre vide, mais la commande doit reussir
    let out = Command::new(env!("CARGO_BIN_EXE_auris"))
        .arg("audio-devices")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn commande_inconnue_echoue() {
    let out = Command::new(env!("CARGO_BIN_EXE_auris"))
        .arg("nimportequoi")
        .output()
        .unwrap();
    assert!(!out.status.success());
}
