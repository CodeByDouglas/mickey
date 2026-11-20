//! Keeping `Downloads/Mickey` from growing for ever (PRD 0015, #479).
//!
//! Mickey never deletes what the owner brought: the Bring question only
//! says the folder is worth cleaning when it is big or the disk runs low.
//! What Mickey does delete is its own leftovers: a file it was receiving
//! that nobody resumed for a week.

use crate::platform::space::Space;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Above this, the folder is worth a look.
pub const BIG_FOLDER: u64 = 10_000_000_000;
/// Below this share of the disk free, after a transfer, it is too.
pub const LOW_FREE_PERCENT: u64 = 10;
/// A file being received, untouched this long, was abandoned.
pub const ABANDONED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Whether a folder holding `held` bytes is worth cleaning before
/// `incoming` more arrive on a disk with `space`: it holds more than
/// `BIG_FOLDER`, or less than `LOW_FREE_PERCENT` of the disk would be left.
pub fn worth_cleaning(held: u64, incoming: u64, space: Space) -> bool {
    let left = space.available.saturating_sub(incoming);
    held > BIG_FOLDER || left < space.total / 100 * LOW_FREE_PERCENT
}

/// What `folder` holds, in bytes, links never followed.
pub fn held(folder: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(folder) else {
        return 0;
    };
    read.flatten()
        .filter_map(|entry| {
            let metadata = std::fs::symlink_metadata(entry.path()).ok()?;
            Some(if metadata.is_dir() {
                held(&entry.path())
            } else if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
        })
        .fold(0, u64::saturating_add)
}

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
    use crate::test_util::Scratch;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn a_big_folder_or_a_low_disk_is_worth_cleaning() {
        let roomy = Space {
            available: 500 * GB,
            total: 1000 * GB,
        };
        assert!(!worth_cleaning(GB, GB, roomy));
        assert!(worth_cleaning(13 * GB, 0, roomy), "over 10 GB");
        let low = Space {
            available: 120 * GB,
            total: 1000 * GB,
        };
        assert!(worth_cleaning(0, 30 * GB, low), "90 GB of 1000 left");
        assert!(!worth_cleaning(0, 10 * GB, low), "110 GB left");
    }

    #[test]
    fn a_folder_holds_what_is_in_it() {
        let scratch = Scratch::new("held");
        scratch.file("fotos/a.jpg", &[0; 100]);
        scratch.file("fotos/2026/b.jpg", &[0; 50]);
        scratch.file("notes.txt", &[0; 3]);
        assert_eq!(held(&scratch.0), 153);
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
