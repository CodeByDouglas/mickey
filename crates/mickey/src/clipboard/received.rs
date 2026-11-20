//! Where files copied on another PC land on this one (PRD 0011), so the
//! clipboard can point at them: a fresh folder per transfer under Mickey's
//! own cache folder. Only the last few transfers are kept.
//!
//! The names come from the network: `fs::names` refuses any that could
//! leave the folder.

use crate::fs::names;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Transfers kept; older folders are removed.
const KEEP: usize = 5;

/// `~/.cache/mickey/received` on Linux, `%LOCALAPPDATA%\mickey\received`
/// on Windows.
pub fn base() -> Result<PathBuf, String> {
    let cache = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))
    };
    cache
        .map(|dir| dir.join("mickey").join("received"))
        .ok_or_else(|| "no folder to put received files in".to_string())
}

/// Writes `files` (name, bytes) into a new folder and returns their paths.
pub fn save(files: Vec<(String, Vec<u8>)>) -> Result<Vec<PathBuf>, String> {
    save_in(&base()?, files)
}

fn save_in(base: &Path, files: Vec<(String, Vec<u8>)>) -> Result<Vec<PathBuf>, String> {
    let folder = fresh_folder(base)?;
    // The clipboard's files say they came from Mickey, not which PC: the
    // data link does not say.
    let mark = crate::fs::mark::Mark::configured("");
    let mut paths = Vec::new();
    for (name, bytes) in files {
        let name = names::safe_name(&name).ok_or("refused a file name from the network")?;
        let path = folder.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("cannot write a received file: {err}"))?;
        mark.put_on(&path);
        paths.push(path);
    }
    prune(base);
    Ok(paths)
}

/// A new, empty folder for one transfer's files (PRD 0015), under `base`
/// with the other received ones; the oldest beyond the last few go.
pub fn new_folder(base: &Path) -> Result<PathBuf, String> {
    let folder = fresh_folder(base)?;
    prune(base);
    Ok(folder)
}

/// A new folder under `base`, named by the time, that only this user may
/// open.
fn fresh_folder(base: &Path) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let folder = base.join(stamp.to_string());
    std::fs::create_dir_all(&folder)
        .map_err(|err| format!("cannot create {}: {err}", folder.display()))?;
    // Copies from another PC are this user's alone, not every account's on
    // this one (on Windows, %LOCALAPPDATA% already is).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("cannot protect {}: {err}", base.display()))?;
    }
    Ok(folder)
}

/// Removes every file received from another PC (`[clipboard] share =
/// "off"`, #437). Nothing received yet is not an error.
pub fn clear() -> Result<(), String> {
    clear_in(&base()?)
}

fn clear_in(base: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(base) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {err}", base.display()))
        }
        _ => Ok(()),
    }
}

/// Removes all but the newest `KEEP` transfer folders.
fn prune(base: &Path) {
    let Ok(read) = std::fs::read_dir(base) else {
        return;
    };
    let mut folders: Vec<(u128, PathBuf)> = read
        .flatten()
        .filter_map(|entry| Some((entry.file_name().to_str()?.parse().ok()?, entry.path())))
        .collect();
    folders.sort();
    let extra = folders.len().saturating_sub(KEEP);
    for (_, folder) in folders.into_iter().take(extra) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_land_in_a_fresh_folder_and_old_ones_go() {
        let base = std::env::temp_dir().join(format!("mickey-received-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = save_in(&base, vec![("a.txt".into(), b"hello".to_vec())]).unwrap();
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"hello");
        assert!(paths[0].starts_with(&base));
        assert!(save_in(&base, vec![("../x".into(), vec![])]).is_err());
        for _ in 0..KEEP + 2 {
            save_in(&base, vec![("b".into(), vec![])]).unwrap();
        }
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), KEEP);
        clear_in(&base).unwrap();
        assert!(!base.exists(), "every received file is gone");
        clear_in(&base).expect("clearing nothing is fine");
    }
}
