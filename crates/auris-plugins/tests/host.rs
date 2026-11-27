//! Tests d'intégration de PluginHost avec le binaire mock-plugin (#8).

use std::path::{Path, PathBuf};

use auris_plugins::PluginHost;

/// Construit un répertoire plugin source contenant le manifeste + le binaire mock.
fn make_plugin_src(root: &Path) -> PathBuf {
    let src = root.join("mock-src");
    std::fs::create_dir_all(&src).unwrap();
    let bin_name = Path::new(env!("CARGO_BIN_EXE_mock-plugin"))
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::fs::copy(env!("CARGO_BIN_EXE_mock-plugin"), src.join(&bin_name)).unwrap();
    std::fs::write(
        src.join("auris-plugin.toml"),
        format!(
            r#"
[plugin]
id = "mock"
name = "Mock Plugin"
version = "0.1.0"
category = "metadata"
license = "AGPL-3.0-only"
description = "Plugin mock pour les tests"

[install]
bin = "{bin_name}"
"#
        ),
    )
    .unwrap();
    src
}

#[test]
fn install_list_call_test_enable_uninstall() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let host = PluginHost::new(&data_dir);
    assert!(host.list().is_empty());

    let src = make_plugin_src(tmp.path());
    let info = host.install_from_dir(&src).unwrap();
    assert_eq!(info.plugin.id, "mock");
    assert!(info.enabled);
    assert!(!info.running);

    // install en double -> erreur
    assert!(host.install_from_dir(&src).is_err());

    // ping via JSON-RPC
    let pong = host.call("mock", "ping", serde_json::json!({})).unwrap();
    assert_eq!(pong, serde_json::json!({ "pong": true }));

    // echo
    let echo = host
        .call("mock", "echo", serde_json::json!({ "hello": "monde" }))
        .unwrap();
    assert_eq!(echo, serde_json::json!({ "hello": "monde" }));

    // méthode inconnue -> erreur JSON-RPC remontée
    assert!(host.call("mock", "nope", serde_json::json!({})).is_err());

    // test de santé
    let t = host.test("mock").unwrap();
    assert!(t.get("manifest").is_some());
    assert!(t.get("capabilities").is_some());

    // running après un appel
    assert!(
        host.list()
            .iter()
            .find(|p| p.plugin.id == "mock")
            .unwrap()
            .running
    );

    // désactiver -> appel refusé
    let info = host.set_enabled("mock", false).unwrap();
    assert!(!info.enabled);
    assert!(host.call("mock", "ping", serde_json::json!({})).is_err());

    // réactiver
    let info = host.set_enabled("mock", true).unwrap();
    assert!(info.enabled);
    assert!(host.call("mock", "ping", serde_json::json!({})).is_ok());

    // plugin inconnu
    assert!(host.call("inconnu", "ping", serde_json::json!({})).is_err());

    // désinstallation
    host.uninstall("mock").unwrap();
    assert!(host.list().is_empty());
    assert!(host.call("mock", "ping", serde_json::json!({})).is_err());
}

#[test]
fn manifest_invalide_ignore() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let plugins = data_dir.join("plugins").join("broken");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(plugins.join("auris-plugin.toml"), "pas du toml [[[").unwrap();
    let host = PluginHost::new(&data_dir);
    assert!(host.list().is_empty());
    assert!(host.install_from_dir(&plugins).is_err());
}
