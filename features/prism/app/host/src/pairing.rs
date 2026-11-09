//! The pairing file `{port, token}` below each CLI configuration directory that exists on
//! this machine; bridges in containers read it through their mount of that directory.
use crate::files::write_atomic;
use prism_core::pairing::{PAIRING_FILE, Pairing};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// CLI configuration directories below the home directory that may receive a pairing file.
pub const CLI_DIRECTORIES: [&str; 2] = [".claude", ".codex"];

pub fn cli_directories(home: &Path) -> Vec<PathBuf> {
    CLI_DIRECTORIES
        .iter()
        .map(|directory| home.join(directory))
        .collect()
}

/// Outcome per pairing file, shown in the app so the user can see which CLIs can connect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairingFile {
    pub path: PathBuf,
    /// `None` when the file was written.
    pub error: Option<String>,
}

/// Writes the pairing file below every CLI directory that exists, creating its `prism/`
/// directory; a CLI that is not installed (no directory) is skipped.
pub fn write_pairing(cli_directories: &[PathBuf], pairing: &Pairing) -> Vec<PairingFile> {
    let contents = serde_json::to_vec_pretty(pairing).expect("the pairing file serializes");
    cli_directories
        .iter()
        .filter(|directory| directory.is_dir())
        .map(|directory| {
            let path = directory.join(PAIRING_FILE);
            let result = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| write_atomic(&path, &contents, true));
            PairingFile {
                error: result.err().map(|error| error.to_string()),
                path,
            }
        })
        .collect()
}

/// Removes the written pairing files that still hold this app's token, so bridges stop
/// dialing an app that quit; a file another app instance rewrote stays.
pub fn remove_pairing(files: &[PairingFile], token: &str) {
    for file in files.iter().filter(|file| file.error.is_none()) {
        let ours = fs::read(&file.path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<Pairing>(&contents).ok())
            .is_some_and(|pairing| pairing.token == token);
        if ours {
            let _ = fs::remove_file(&file.path);
        }
    }
}
