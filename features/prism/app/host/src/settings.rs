//! Listener settings from `settings.json` in the app's data directory. The file is optional
//! and only written by the user, for example to listen beyond loopback for plain Linux Docker.
use prism_core::pairing::DEFAULT_APP_PORT;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "settings.json";

/// Docker Desktop forwards `host.docker.internal` to the host's loopback, so nothing else
/// on the network can reach the listener by default.
const DEFAULT_BIND_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `0.0.0.0` for plain Linux Docker, where containers reach the host through the bridge
    /// network instead of loopback.
    pub bind_address: IpAddr,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bind_address: DEFAULT_BIND_ADDRESS,
            port: DEFAULT_APP_PORT,
        }
    }
}

impl Settings {
    pub fn path(data_directory: &Path) -> PathBuf {
        data_directory.join(SETTINGS_FILE)
    }

    /// A missing file means the defaults; an unreadable or invalid one is an error the app
    /// shows, while it still starts with the defaults.
    pub fn load(data_directory: &Path) -> (Self, Option<String>) {
        let path = Self::path(data_directory);
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("{}: {error}", path.display())),
                );
            }
        };
        match serde_json::from_slice(&contents) {
            Ok(settings) => (settings, None),
            Err(error) => (
                Self::default(),
                Some(format!("{}: {error}; using the defaults", path.display())),
            ),
        }
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.port)
    }
}
