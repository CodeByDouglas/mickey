//! Opening the next files of an offer while one is sent (PRD 0015).
//!
//! The other PC fetches an offer's files one after another, in the order
//! of the listing. Opening a file can be slow: on Windows the antivirus
//! checks each file as it is opened, about 10 ms for a small one it has
//! not seen, so a folder of thousands of small files (a `node_modules`)
//! crossed at about 100 files a second. Opening the next few files on
//! threads of their own, while the current one is sent, overlaps those
//! waits.
//!
//! Only files of the listing are opened, the same way a fetch opens them
//! (`fs::open::to_read`, never through a link), and nothing is sent until
//! the other PC asks for it.

use crate::fs::open;
use crate::transfer::offer::Listing;
use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::thread::{Builder, Scope, ScopedJoinHandle};

/// How many files are opened ahead. On Windows 10 with Defender on
/// (2026-11-24), opening 200 small files it had not seen took 2.0 s one at
/// a time, 0.6 s four at a time and 0.4 s sixteen at a time.
const OPEN_AHEAD: usize = 8;

/// The files being opened ahead, in the order they will be asked for.
/// Their threads belong to `scope`: none outlives the connection.
pub struct Ahead<'scope, 'env> {
    scope: &'scope Scope<'scope, 'env>,
    opening: VecDeque<Opening<'scope>>,
}

/// One file being opened: of which offer, which file, and the thread
/// opening it.
struct Opening<'scope> {
    offer: u64,
    index: u32,
    file: ScopedJoinHandle<'scope, io::Result<File>>,
}

impl<'scope, 'env> Ahead<'scope, 'env> {
    pub fn new(scope: &'scope Scope<'scope, 'env>) -> Self {
        Ahead {
            scope,
            opening: VecDeque::new(),
        }
    }

    /// The file at `index` of `offer`, if it was being opened ahead. Those
    /// opened before it were not asked for (the other PC had them
    /// already): they are waited for and closed, so no more than `OPEN_AHEAD`
    /// files are ever being opened.
    pub fn take(&mut self, offer: u64, index: u32) -> Option<io::Result<File>> {
        while let Some(next) = self.opening.pop_front() {
            let file = next
                .file
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("opening a file panicked")));
            if (next.offer, next.index) == (offer, index) {
                return Some(file);
            }
        }
        None
    }

    /// Starts opening the files of `listing` (offer `offer`) after `index`,
    /// up to `OPEN_AHEAD` of them. Folders are skipped: they are not fetched.
    pub fn open_after(&mut self, offer: u64, listing: &Listing, index: u32) {
        let mut next = match self.opening.back() {
            Some(last) if last.offer == offer => last.index,
            _ => index,
        };
        while self.opening.len() < OPEN_AHEAD {
            next = match next.checked_add(1) {
                Some(next) if (next as usize) < listing.entries.len() => next,
                _ => return,
            };
            let Some((path, ..)) = listing.file(next) else {
                continue;
            };
            let path = path.to_path_buf();
            let opening = Builder::new()
                .name("transfer open".into())
                .spawn_scoped(self.scope, move || open::to_read(&path));
            // No thread: the file is opened when it is asked for, as before.
            let Ok(file) = opening else {
                return;
            };
            self.opening.push_back(Opening {
                offer,
                index: next,
                file,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::Scratch;
    use crate::transfer::offer;
    use std::io::Read;

    /// A listing of a folder with files `0.txt` to `19.txt` and one empty
    /// folder, each file holding its own number.
    fn listing(scratch: &Scratch) -> Listing {
        let folder = scratch.0.join("many");
        std::fs::create_dir_all(folder.join("empty")).unwrap();
        for n in 0..20 {
            std::fs::write(folder.join(format!("{n:02}.txt")), n.to_string()).unwrap();
        }
        offer::list(&[folder]).unwrap()
    }

    /// What `file` holds.
    fn text(file: Option<io::Result<File>>) -> String {
        let mut text = String::new();
        file.unwrap().unwrap().read_to_string(&mut text).unwrap();
        text
    }

    /// The index of the listed file named `name`.
    fn index_of(listing: &Listing, name: &str) -> u32 {
        let found = listing.entries.iter().position(|e| e.path.ends_with(name));
        found.unwrap() as u32
    }

    #[test]
    fn the_next_files_are_opened_ahead_in_order() {
        let scratch = Scratch::new("ahead");
        let listing = listing(&scratch);
        let first = index_of(&listing, "00.txt");
        std::thread::scope(|scope| {
            let mut ahead = Ahead::new(scope);
            assert!(ahead.take(1, first).is_none(), "nothing opened yet");
            ahead.open_after(1, &listing, first);
            assert_eq!(ahead.opening.len(), OPEN_AHEAD);
            let second = index_of(&listing, "01.txt");
            assert_eq!(text(ahead.take(1, second)), "1");
            // The other PC had 02 to 04 already: they are closed.
            let fifth = index_of(&listing, "05.txt");
            assert_eq!(text(ahead.take(1, fifth)), "5");
            ahead.open_after(1, &listing, fifth);
            assert_eq!(ahead.opening.len(), OPEN_AHEAD, "topped up");
            let last = listing.file(ahead.opening.back().unwrap().index);
            assert!(last.unwrap().0.ends_with("13.txt"), "in order");
        });
    }

    #[test]
    fn a_file_not_guessed_is_not_there() {
        let scratch = Scratch::new("ahead-miss");
        let listing = listing(&scratch);
        let first = index_of(&listing, "00.txt");
        std::thread::scope(|scope| {
            let mut ahead = Ahead::new(scope);
            ahead.open_after(1, &listing, first);
            assert!(ahead.take(2, first + 1).is_none(), "another offer");
            assert!(ahead.opening.is_empty(), "and none left open");
            ahead.open_after(1, &listing, first);
            assert!(ahead.take(1, first).is_none(), "one already sent");
        });
    }

    #[test]
    fn nothing_is_opened_past_the_last_file() {
        let scratch = Scratch::new("ahead-end");
        let listing = listing(&scratch);
        let last = index_of(&listing, "19.txt");
        std::thread::scope(|scope| {
            let mut ahead = Ahead::new(scope);
            ahead.open_after(1, &listing, last);
            let left = listing.entries.len() - 1 - last as usize;
            let files_left = (last as usize + 1..listing.entries.len())
                .filter(|&i| listing.file(i as u32).is_some())
                .count();
            assert!(ahead.opening.len() <= left);
            assert_eq!(ahead.opening.len(), files_left);
        });
    }
}
