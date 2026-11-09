use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Owner read and write only; the pairing file holds the token that admits bridges.
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

/// Writes through a temporary file in the same directory and a rename, so a reader (a bridge
/// re-reading the pairing file, the app reading its arrangement) sees the old or the new
/// content, never a partial file. `private` limits the file to its owner on Unix; on Windows
/// files below the user profile are private to the user already.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn write_atomic(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    let temporary = temporary_path(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_MODE);
    }

    let result = (|| {
        let mut file = options.open(&temporary)?;
        // A leftover temporary file keeps its old mode on open; set it explicitly.
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(PRIVATE_MODE))?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    let mut temporary = name.to_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    Ok(path.with_file_name(temporary))
}
