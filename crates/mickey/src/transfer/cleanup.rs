//! Keeping `Downloads/Mickey` from growing for ever (PRD 0015, #479).
//!
//! Mickey never deletes what the owner brought: the Bring question only
//! says the folder is worth cleaning when it is big or the disk runs low,
//! with how old each thing in it is. What Mickey does delete is its own
//! leftovers: a file it was receiving that nobody resumed for a week.

use crate::platform::space::Space;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// A file being received, untouched this long, was abandoned.
pub const ABANDONED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// When the folder is worth cleaning (`[interface] clean_over_gb` and
/// `clean_below_percent`).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Holding more than this many bytes.
    pub over: u64,
    /// Less than this share of the disk free after the transfer.
    pub below_percent: u64,
}

/// Whether a folder holding `held` bytes is worth cleaning before
/// `incoming` more arrive on a disk with `space`.
pub fn worth_cleaning(held: u64, incoming: u64, space: Space, limits: Limits) -> bool {
    let left = space.available.saturating_sub(incoming);
    held > limits.over || left < space.total / 100 * limits.below_percent
}

/// One thing at the top of the folder, as the suggestion shows it.
#[derive(Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub bytes: u64,
    /// When it arrived: when it was made here (or, where the system does
    /// not keep that, last changed).
    pub arrived: SystemTime,
    /// The last change to it or anything inside.
    pub changed: SystemTime,
    /// A transfer into it broke and has not gone on yet.
    pub incomplete: bool,
}

/// What is at the top of `folder`, biggest first; links never followed.
pub fn look(folder: &Path) -> Vec<Item> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = read
        .flatten()
        .filter_map(|entry| {
            let metadata = std::fs::symlink_metadata(entry.path()).ok()?;
            let made = metadata.created().or_else(|_| metadata.modified()).ok()?;
            let mut item = Item {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: 0,
                arrived: made,
                changed: metadata.modified().unwrap_or(made),
                incomplete: false,
            };
            measure(&entry.path(), &metadata, &mut item);
            Some(item)
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.bytes));
    items
}

/// Adds `path` (with its `metadata`), and what is inside, to `item`.
fn measure(path: &Path, metadata: &std::fs::Metadata, item: &mut Item) {
    if let Ok(modified) = metadata.modified() {
        item.changed = item.changed.max(modified);
    }
    if metadata.is_file() {
        item.bytes = item.bytes.saturating_add(metadata.len());
        item.incomplete |= is_part(path);
    } else if metadata.is_dir()
        && let Ok(read) = std::fs::read_dir(path)
    {
        for entry in read.flatten() {
            if let Ok(inner) = std::fs::symlink_metadata(entry.path()) {
                measure(&entry.path(), &inner, item);
            }
        }
    }
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
    const LIMITS: Limits = Limits {
        over: 10 * GB,
        below_percent: 10,
    };

    #[test]
    fn a_big_folder_or_a_low_disk_is_worth_cleaning() {
        let roomy = Space {
            available: 500 * GB,
            total: 1000 * GB,
        };
        assert!(!worth_cleaning(GB, GB, roomy, LIMITS));
        assert!(worth_cleaning(13 * GB, 0, roomy, LIMITS), "over 10 GB");
        let low = Space {
            available: 120 * GB,
            total: 1000 * GB,
        };
        assert!(
            worth_cleaning(0, 30 * GB, low, LIMITS),
            "90 GB of 1000 left"
        );
        assert!(!worth_cleaning(0, 10 * GB, low, LIMITS), "110 GB left");
        let stricter = Limits {
            over: 50 * GB,
            below_percent: 15,
        };
        assert!(worth_cleaning(0, 10 * GB, low, stricter), "under 15% left");
        assert!(!worth_cleaning(13 * GB, 0, roomy, stricter), "under 50 GB");
    }

    #[test]
    fn each_thing_in_the_folder_is_measured_biggest_first() {
        let scratch = Scratch::new("look");
        scratch.file("fotos/a.jpg", &[0; 100]);
        scratch.file("fotos/2026/b.jpg", &[0; 50]);
        scratch.file("notes.txt", &[0; 3]);
        scratch.file("video/v.mp4.mickey-part", &[0; 7]);
        let items = look(&scratch.0);
        let summary: Vec<(&str, u64, bool)> = items
            .iter()
            .map(|item| (item.name.as_str(), item.bytes, item.incomplete))
            .collect();
        assert_eq!(
            summary,
            [
                ("fotos", 150, false),
                ("video", 7, true),
                ("notes.txt", 3, false)
            ]
        );
        assert!(items.iter().all(|item| item.changed >= item.arrived));
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
