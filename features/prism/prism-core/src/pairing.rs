//! Pairing file: written by the desktop app on the host, read by each bridge in a container
//! through the mounted CLI configuration directory.
use serde::{Deserialize, Serialize};

/// Port the app listens on unless its pairing file says otherwise.
pub const DEFAULT_APP_PORT: u16 = 7717;
/// Name under which a container reaches the host that runs the app.
pub const DEFAULT_APP_HOST: &str = "host.docker.internal";
/// Environment variable that replaces `DEFAULT_APP_HOST`.
pub const APP_HOST_ENV: &str = "PRISM_APP_HOST";
/// Location below the CLI configuration directory (`~/.claude`, `~/.codex`).
pub const PAIRING_FILE: &str = "prism/app.json";
/// Environment variable that replaces the pairing file path.
pub const PAIRING_FILE_ENV: &str = "PRISM_APP_FILE";

/// Contains a secret: deliberately does not implement Debug.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub port: u16,
    pub token: String,
}
