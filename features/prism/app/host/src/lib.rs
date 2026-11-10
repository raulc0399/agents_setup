//! Prism desktop app without its window shell, so that everything with logic runs and is
//! tested on any platform: the bridge listener and handshake, the pairing file, the session
//! registry, the window arrangement and the diagram layout. `prism-app` maps it onto Tauri
//! windows.

pub mod arrangement;
pub mod diagram;
mod files;
mod host;
mod listener;
pub mod pairing;
pub mod registry;
pub mod settings;

pub use host::{AppState, EventSink, Host, HostConfig, Status, Timing};
