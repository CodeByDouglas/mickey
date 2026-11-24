//! The folder fetched files go to (PRD 0015): `Mickey` in this user's
//! Downloads folder.

use std::path::{Path, PathBuf};

/// `Downloads/Mickey`; not made here, only once files come (`make`).
pub fn mickey_folder() -> Result<PathBuf, String> {
    Ok(downloads()
        .ok_or("no Downloads folder for this user")?
        .join("Mickey"))
}

/// Makes `folder` if it is not there yet. Files from another PC are this
/// user's alone, not every account's on this one: on Linux it is made
/// only this user may open (on Windows, the user's folder already is).
/// One the owner made, or opened up, is left as it is.
pub fn make(folder: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(folder)
        .map_err(|err| format!("cannot create {}: {err}", folder.display()))
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

    #[cfg(unix)]
    #[test]
    fn the_mickey_folder_is_this_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::test_util::Scratch::new("downloads");
        let folder = scratch.0.join("Downloads/Mickey");
        make(&folder).unwrap();
        make(&folder).expect("there already is fine");
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
