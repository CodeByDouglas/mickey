//! How much room a disk has, for file transfers (PRD 0015): the standard
//! library cannot tell. `statvfs` on Linux, `GetDiskFreeSpaceExW` on
//! Windows.

use std::io;
use std::path::Path;

/// A disk's room, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// Free for this user to write (on Linux, not counting what is kept
    /// for the system).
    pub available: u64,
    pub total: u64,
}

/// The room on the disk that holds `path` (which must exist).
#[cfg(target_os = "linux")]
pub fn of(path: &Path) -> io::Result<Space> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("a path with a NUL byte"))?;
    // SAFETY: an all-zero `statvfs` is a valid value of that plain C struct.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a NUL-terminated string that lives across the call,
    // and `stats` is a writable `statvfs` the call fills in.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let block = stats.f_frsize;
    Ok(Space {
        available: stats.f_bavail.saturating_mul(block),
        total: stats.f_blocks.saturating_mul(block),
    })
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        available_to_caller: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> i32;
}

/// The room on the disk that holds `path` (which must exist).
#[cfg(windows)]
pub fn of(path: &Path) -> io::Result<Space> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut available, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that lives across the
    // call; the two counts are writable u64s; the last one may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Space { available, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temp_folder_has_some_room() {
        let space = of(&std::env::temp_dir()).unwrap();
        assert!(space.total > 0);
        assert!(space.available <= space.total);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        assert!(of(Path::new("/no/such/folder/for/mickey")).is_err());
    }
}
