//! Keeping `Downloads/Mickey` from growing for ever (PRD 0015, #479).
//!
//! Mickey never deletes what the owner brought. What it does delete is
//! its own leftovers: a file it was receiving that nobody resumed for a
//! week.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// A file being received, untouched this long, was abandoned.
pub const ABANDONED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Removes the files being received under `folder` untouched for
/// `ABANDONED` before `now`, links never followed; says how many.
pub fn remove_abandoned(folder: &Path, now: SystemTime) -> usize {
    let Ok(read) = std::fs::read_dir(folder) else {
        return 0;
    };
    let mut removed = 0;
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            removed += remove_abandoned(&path, now);
        } else if metadata.is_file()
            && is_part(&path)
            && metadata
                .modified()
                .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age >= ABANDONED))
            && std::fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// A file Mickey was receiving, never one that only ends in `.part`.
fn is_part(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .ends_with(crate::transfer::path::RECEIVING)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh folder under the system's temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("mickey-cleanup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_files_being_received_untouched_for_a_week_are_removed() {
        let scratch = Scratch::new("abandoned");
        std::fs::create_dir_all(scratch.0.join("fotos")).unwrap();
        std::fs::write(scratch.0.join("fotos/old.jpg.mickey-part"), b"x").unwrap();
        std::fs::write(scratch.0.join("kept.jpg"), b"x").unwrap();
        std::fs::write(scratch.0.join("chapter.part"), b"x").unwrap();
        let now = SystemTime::now();
        assert_eq!(remove_abandoned(&scratch.0, now), 0, "a fresh one stays");
        let later = now + ABANDONED + Duration::from_secs(60);
        assert_eq!(remove_abandoned(&scratch.0, later), 1);
        assert!(!scratch.0.join("fotos/old.jpg.mickey-part").exists());
        assert!(
            scratch.0.join("kept.jpg").exists(),
            "never what the owner brought"
        );
        assert!(
            scratch.0.join("chapter.part").exists(),
            "nor what only ends in .part"
        );
    }
}
