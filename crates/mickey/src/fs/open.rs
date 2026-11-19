//! Opening a file to send without following a link: a file listed as a
//! plain file may be swapped for a link before it is opened, and a link to
//! `~/.ssh/id_rsa` must never be read in its place.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Opens `path` to read, refusing it if it is a link (or a junction on
/// Windows) or anything but a plain file.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "file transfers send with it from #478 on")
)]
pub fn without_links(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    // Linux: opening a link fails (ELOOP) instead of opening its target.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW);
    // Windows: a link or junction is opened itself, not its target, and
    // then fails the check below.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(&mut options, FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a plain file",
            path.display()
        )));
    }
    Ok(file)
}

/// From `winbase.h`.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_file_opens() {
        let path = std::env::temp_dir().join(format!("mickey-open-{}", std::process::id()));
        std::fs::write(&path, b"ok").unwrap();
        assert!(without_links(&path).is_ok());
        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_or_a_folder_is_refused() {
        let dir = std::env::temp_dir().join(format!("mickey-open-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("secret"), b"key").unwrap();
        std::os::unix::fs::symlink(dir.join("secret"), dir.join("link")).unwrap();
        assert!(without_links(&dir.join("link")).is_err());
        assert!(without_links(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
