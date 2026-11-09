//! Listening, the pairing file and the settings file.
mod common;

use common::*;
use prism_app_host::settings::{SETTINGS_FILE, Settings};
use prism_core::pairing::{DEFAULT_APP_PORT, PAIRING_FILE};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use tempfile::TempDir;

#[test]
fn writes_a_private_pairing_file_for_each_installed_cli() {
    let app = start(
        home_with(&[".claude", ".codex"]),
        TempDir::new().unwrap(),
        |_| {},
    );
    let state = app.host.state();
    assert!(state.status.listening);
    assert_eq!(state.status.pairing.len(), 2);

    for directory in [".claude", ".codex"] {
        let path = app.home.path().join(directory).join(PAIRING_FILE);
        let pairing: prism_core::pairing::Pairing =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(pairing.port, app.port());
        assert_eq!(pairing.token, app.token());
        assert_eq!(pairing.token.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
        // Written through a rename: no temporary file is left next to it.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["app.json"]);
    }
}

#[test]
fn a_cli_without_its_directory_is_skipped() {
    let app = start_app();
    assert_eq!(app.host.state().status.pairing.len(), 1);
    assert!(!app.home.path().join(".codex").exists());
}

#[test]
fn each_start_writes_a_fresh_token() {
    let home = home_with(&[".claude"]);
    let first = start(home, TempDir::new().unwrap(), |_| {});
    let token = first.token();
    let home = first.home;
    let second = start(home, TempDir::new().unwrap(), |_| {});
    assert_ne!(second.token(), token);
}

#[test]
fn shutdown_removes_only_a_pairing_file_holding_its_own_token() {
    let app = start(
        home_with(&[".claude", ".codex"]),
        TempDir::new().unwrap(),
        |_| {},
    );
    let claude = app.home.path().join(".claude").join(PAIRING_FILE);
    let codex = app.home.path().join(".codex").join(PAIRING_FILE);
    // Another app instance rewrote the Codex file in the meantime.
    std::fs::write(&codex, r#"{"port": 7717, "token": "other"}"#).unwrap();

    app.host.shutdown();
    assert!(!claude.exists());
    assert!(codex.exists());
}

#[test]
fn a_port_in_use_is_a_clear_error_and_writes_no_pairing_file() {
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let app = start(home_with(&[".claude"]), TempDir::new().unwrap(), |config| {
        config.address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    });

    let status = app.host.state().status;
    assert!(!status.listening);
    let error = status.listen_error.unwrap();
    assert!(error.contains(&format!("Port {port}")), "{error}");
    assert!(error.contains("already in use"), "{error}");
    assert!(status.pairing.is_empty());
    assert!(!app.home.path().join(".claude").join(PAIRING_FILE).exists());
}

#[test]
fn settings_default_to_loopback_on_the_prism_port() {
    let data = TempDir::new().unwrap();
    let (settings, error) = Settings::load(data.path());
    assert_eq!(error, None);
    assert_eq!(
        settings.address(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_APP_PORT))
    );
}

#[test]
fn settings_can_bind_all_addresses_for_plain_linux_docker() {
    let data = TempDir::new().unwrap();
    std::fs::write(
        data.path().join(SETTINGS_FILE),
        r#"{"bind_address": "0.0.0.0"}"#,
    )
    .unwrap();
    let (settings, error) = Settings::load(data.path());
    assert_eq!(error, None);
    assert_eq!(settings.bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(settings.port, DEFAULT_APP_PORT);
}

#[test]
fn an_invalid_settings_file_is_reported_and_the_defaults_apply() {
    let data = TempDir::new().unwrap();
    std::fs::write(data.path().join(SETTINGS_FILE), "{bind").unwrap();
    let (settings, error) = Settings::load(data.path());
    assert_eq!(settings, Settings::default());
    assert!(error.unwrap().contains(SETTINGS_FILE));

    let app = start(home_with(&[".claude"]), data, |_| {});
    assert_eq!(app.host.state().status.warnings.len(), 1);
    assert!(app.host.state().status.listening);
}

#[test]
fn a_settings_error_and_a_bind_error_are_both_shown() {
    let data = TempDir::new().unwrap();
    std::fs::write(data.path().join(SETTINGS_FILE), "{bind").unwrap();
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let app = start(home_with(&[".claude"]), data, |config| {
        config.address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    });
    let status = app.host.state().status;
    assert!(status.listen_error.unwrap().contains("already in use"));
    assert!(status.warnings[0].contains(SETTINGS_FILE));
}
