//! The folder brought files go to (PRD 0015): `Mickey` in this user's
//! Downloads folder.

use std::path::{Path, PathBuf};

/// `Downloads/Mickey`, made if it is not there yet.
pub fn mickey_folder() -> Result<PathBuf, String> {
    let folder = downloads()
        .ok_or("no Downloads folder for this user")?
        .join("Mickey");
    std::fs::create_dir_all(&folder)
        .map_err(|err| format!("cannot create {}: {err}", folder.display()))?;
    Ok(folder)
}

/// This user's Downloads folder: on Linux the one `user-dirs.dirs` names
/// (translated on many systems, `~/Transferências`), else `~/Downloads`.
/// ponytail: on Windows `%USERPROFILE%\Downloads`, which misses a
/// Downloads folder the owner moved; the upgrade is asking the shell
/// (`SHGetKnownFolderPath`).
fn downloads() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("USERPROFILE").map(|home| Path::new(&home).join("Downloads"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let named = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| download_dir(&text, &home));
    Some(named.unwrap_or_else(|| home.join("Downloads")))
}

/// `XDG_DOWNLOAD_DIR` from a `user-dirs.dirs` text, `$HOME` expanded; only
/// a folder inside `home`, as the format allows.
fn download_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let value = line.trim().trim_matches('"');
    let below = value.strip_prefix("$HOME/")?;
    (!below.is_empty() && !below.split('/').any(|part| part == "..")).then(|| home.join(below))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_folder_comes_from_user_dirs() {
        let home = Path::new("/home/ana");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Área de Trabalho\"\nXDG_DOWNLOAD_DIR=\"$HOME/Transferências\"\n";
        assert_eq!(download_dir(text, home), Some(home.join("Transferências")));
        assert_eq!(
            download_dir("XDG_DOWNLOAD_DIR=\"$HOME/../etc\"", home),
            None
        );
        assert_eq!(download_dir("XDG_DOWNLOAD_DIR=\"/tmp\"", home), None);
        assert_eq!(download_dir("", home), None);
    }
}
