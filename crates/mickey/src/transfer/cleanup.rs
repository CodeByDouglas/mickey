//! Keeping `Downloads/Mickey` from growing for ever (PRD 0015, #479).
//!
//! Mickey never deletes what the owner brought: it only suggests cleaning
//! when the folder grows big or the disk runs low, saying how old each
//! item is. What it does delete is its own leftovers: a `.part` nobody
//! resumed for a week.

use crate::platform::space::Space;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Above this, the folder is worth a look.
pub const BIG_FOLDER: u64 = 10_000_000_000;
/// Below this share of the disk free, after a transfer, it is too.
pub const LOW_FREE_PERCENT: u64 = 10;
/// A `.part` untouched this long was abandoned.
pub const ABANDONED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One thing in the folder, as the suggestion shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    /// The whole size, for a folder all that is inside it.
    pub size: u64,
    /// When it arrived (the system's creation time where it keeps one).
    pub arrived: SystemTime,
    /// When anything in it last changed.
    pub modified: SystemTime,
}

/// What to suggest to the owner.
#[derive(Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// How big the folder is.
    pub size: u64,
    /// Its biggest items first, to clean where it counts.
    pub items: Vec<Item>,
    /// Incomplete transfers, and their size.
    pub parts: (usize, u64),
}

/// Whether to suggest cleaning `items` before a transfer of `incoming`
/// bytes onto a disk with `space`: the folder holds more than
/// `BIG_FOLDER`, or less than `LOW_FREE_PERCENT` of the disk would be left.
pub fn suggest(
    mut items: Vec<Item>,
    parts: (usize, u64),
    incoming: u64,
    space: Space,
) -> Option<Suggestion> {
    let size = items.iter().map(|item| item.size).sum::<u64>() + parts.1;
    let left = space.available.saturating_sub(incoming);
    let low = left < space.total / 100 * LOW_FREE_PERCENT;
    if size <= BIG_FOLDER && !low {
        return None;
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.size));
    Some(Suggestion { size, items, parts })
}

/// What is in `folder`: its items (files and folders right inside it,
/// sized with what they hold, `.part` files aside) and its `.part` files
/// anywhere (how many, how big), links never followed.
pub fn look(folder: &Path) -> (Vec<Item>, (usize, u64)) {
    let mut items = Vec::new();
    let mut parts = (0, 0);
    let Ok(read) = std::fs::read_dir(folder) else {
        return (items, parts);
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(item) = measure(&path, &mut parts) else {
            continue;
        };
        if !is_part(&path) {
            items.push(item);
        }
    }
    (items, parts)
}

/// `path`'s item: its size and times, adding up a folder (and noting the
/// `.part` files met in `parts`). `None` for a link or what cannot be read.
fn measure(path: &Path, parts: &mut (usize, u64)) -> Option<Item> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.is_symlink() {
        return None;
    }
    let modified = metadata.modified().ok()?;
    let arrived = metadata.created().unwrap_or(modified);
    let mut item = Item {
        path: path.to_path_buf(),
        size: 0,
        arrived,
        modified,
    };
    if metadata.is_file() {
        // A `.part` is counted once, with the incomplete transfers, not in
        // its folder's size too.
        if is_part(path) {
            parts.0 += 1;
            parts.1 += metadata.len();
        } else {
            item.size = metadata.len();
        }
    } else if metadata.is_dir() {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            if let Some(inside) = measure(&entry.path(), parts) {
                item.size += inside.size;
                item.modified = item.modified.max(inside.modified);
            }
        }
    }
    Some(item)
}

/// Removes the `.part` files under `folder` untouched for `ABANDONED`
/// before `now`, links never followed; says how many.
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

fn is_part(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "part")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn item(name: &str, size: u64) -> Item {
        Item {
            path: PathBuf::from(name),
            size,
            arrived: SystemTime::UNIX_EPOCH,
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    fn roomy() -> Space {
        Space {
            available: 500 * GB,
            total: 1000 * GB,
        }
    }

    #[test]
    fn a_small_folder_on_a_roomy_disk_needs_nothing() {
        assert_eq!(suggest(vec![item("a", GB)], (0, 0), GB, roomy()), None);
    }

    #[test]
    fn a_big_folder_is_suggested_biggest_first() {
        let items = vec![item("small", GB), item("big", 9 * GB), item("mid", 2 * GB)];
        let suggestion = suggest(items, (1, GB), 0, roomy()).unwrap();
        assert_eq!(suggestion.size, 13 * GB);
        let names: Vec<_> = suggestion
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(names, ["big", "mid", "small"].map(PathBuf::from));
        assert_eq!(suggestion.parts, (1, GB));
    }

    #[test]
    fn a_transfer_that_would_leave_the_disk_low_is_suggested_too() {
        let space = Space {
            available: 120 * GB,
            total: 1000 * GB,
        };
        assert!(
            suggest(vec![], (0, 0), 30 * GB, space).is_some(),
            "90 GB left"
        );
        assert!(
            suggest(vec![], (0, 0), 10 * GB, space).is_none(),
            "110 GB left"
        );
    }

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
    fn a_folder_is_measured_with_what_it_holds() {
        let scratch = Scratch::new("look");
        std::fs::create_dir_all(scratch.0.join("fotos/2026")).unwrap();
        std::fs::write(scratch.0.join("fotos/a.jpg"), [0; 100]).unwrap();
        std::fs::write(scratch.0.join("fotos/2026/b.jpg"), [0; 50]).unwrap();
        std::fs::write(scratch.0.join("fotos/2026/c.jpg.part"), [0; 7]).unwrap();
        std::fs::write(scratch.0.join("notes.txt"), [0; 3]).unwrap();
        std::fs::write(scratch.0.join("video.mp4.part"), [0; 20]).unwrap();
        let (mut items, parts) = look(&scratch.0);
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let sizes: Vec<u64> = items.iter().map(|item| item.size).collect();
        assert_eq!(sizes, [150, 3], "fotos/ without its .part, then notes.txt");
        assert_eq!(parts, (2, 27));
    }

    #[test]
    fn only_parts_untouched_for_a_week_are_removed() {
        let scratch = Scratch::new("abandoned");
        std::fs::create_dir_all(scratch.0.join("fotos")).unwrap();
        std::fs::write(scratch.0.join("fotos/old.jpg.part"), b"x").unwrap();
        std::fs::write(scratch.0.join("kept.jpg"), b"x").unwrap();
        let now = SystemTime::now();
        assert_eq!(remove_abandoned(&scratch.0, now), 0, "a fresh .part stays");
        assert_eq!(
            remove_abandoned(&scratch.0, now + ABANDONED + Duration::from_secs(60)),
            1
        );
        assert!(!scratch.0.join("fotos/old.jpg.part").exists());
        assert!(
            scratch.0.join("kept.jpg").exists(),
            "never what the owner brought"
        );
    }
}
