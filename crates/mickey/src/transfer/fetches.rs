//! What became of each offer fetched here, and one lane per PC files come
//! from (PRD 0015): an offer that crosses again is fetched once, one that
//! broke goes on from where it stopped (#475, #502), and transfers from
//! one PC take turns.

use crate::threads::LockAnyway;
use crate::transfer::receive::Fetched;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The most broken transfers kept to go on, per PC: one that started and
/// broke off many (by design or not) cannot keep their folders for ever,
/// nor push out another PC's.
const MAX_BROKEN: usize = 8;

/// The most finished transfers remembered, to paste again rather than
/// fetch again: older ones are fetched again if offered again.
const MAX_DONE: usize = 64;

/// The most transfers from one PC running or waiting their turn at once:
/// each holds a thread and a folder, and a PC in control could otherwise
/// offer new files over and over (found in review). Far above what a
/// person copies and crosses.
const MAX_RUNNING: usize = 4;

/// Each offer fetched here, by the offering PC's key and the offer's id.
#[derive(Default)]
pub struct Fetches(Mutex<HashMap<Key, Fetch>>);

/// An offer: the offering PC's key and the offer's id.
pub type Key = ([u8; 32], u64);

/// What became of one offer.
enum Fetch {
    /// Being fetched now, or waiting its turn.
    Running,
    /// Stopped (the network, a sleeping PC, a full disk, Cancel): what it
    /// had done, and where, to go on; since when.
    Broken {
        into: PathBuf,
        fetched: Box<Fetched>,
        at: Instant,
    },
    /// All there: where its top-level files and folders landed; since when.
    Done { landed: Vec<PathBuf>, at: Instant },
}

/// What to do with an offer that arrives.
pub enum Begin {
    /// Nothing: it is being fetched.
    Skip,
    /// Nothing: that PC has `MAX_RUNNING` transfers going already.
    TooMany,
    /// It is all here already, at these paths: pasted again, not fetched
    /// again (copied again on the other PC, or crossing again).
    Here(Vec<PathBuf>),
    /// Go on with it, into the same folder.
    Resume(PathBuf, Box<Fetched>),
    /// Fetch it for the first time.
    Start,
}

impl Fetches {
    /// Says what to do with offer `key`, and marks it as being fetched
    /// unless it is running or here already. Files deleted since they
    /// arrived are fetched again.
    pub fn begin(&self, key: Key) -> Begin {
        let mut fetches = self.0.lock_anyway();
        match fetches.get(&key) {
            Some(Fetch::Running) => return Begin::Skip,
            // All skipped by the other PC, it is fetched again: nothing
            // was pasted, and the files may be steady now.
            Some(Fetch::Done { landed, .. })
                if !landed.is_empty() && landed.iter().all(|path| path.exists()) =>
            {
                return Begin::Here(landed.clone());
            }
            _ => {}
        }
        let running = fetches
            .iter()
            .filter(|(other, fetch)| other.0 == key.0 && matches!(fetch, Fetch::Running))
            .count();
        if running >= MAX_RUNNING {
            return Begin::TooMany;
        }
        match fetches.insert(key, Fetch::Running) {
            // What it wrote deleted since (by the owner): nothing to go on
            // with, so it starts over.
            Some(Fetch::Broken { into, fetched, .. }) if into.is_dir() && fetched.still_there() => {
                Begin::Resume(into, fetched)
            }
            _ => Begin::Start,
        }
    }

    /// Records that `key` is all here, at `landed`.
    /// The oldest beyond `MAX_DONE` are forgotten.
    pub fn done(&self, key: Key, landed: Vec<PathBuf>) {
        let mut fetches = self.0.lock_anyway();
        let at = Instant::now();
        fetches.insert(key, Fetch::Done { landed, at });
        let done = |_: &Key, fetch: &Fetch| match fetch {
            Fetch::Done { at, .. } => Some(*at),
            _ => None,
        };
        forget_oldest(&mut fetches, done, MAX_DONE);
    }

    /// Records that `key` broke, into `into`, with what it had done.
    /// The oldest of that PC's beyond `MAX_BROKEN` are forgotten: they
    /// would start over.
    pub fn broke(&self, key: Key, into: PathBuf, fetched: Fetched) {
        let mut fetches = self.0.lock_anyway();
        let fetched = Box::new(fetched);
        let at = Instant::now();
        fetches.insert(key, Fetch::Broken { into, fetched, at });
        let from = key.0;
        let broken = |key: &Key, fetch: &Fetch| match fetch {
            Fetch::Broken { at, .. } if key.0 == from => Some(*at),
            _ => None,
        };
        forget_oldest(&mut fetches, broken, MAX_BROKEN);
    }

    /// Forgets `key`: the owner said Ignore, or it could not start.
    pub fn forget(&self, key: Key) {
        self.0.lock_anyway().remove(&key);
    }
}

/// Forgets the oldest of the fetches `since` dates, beyond the `most`
/// newest; the others are not counted.
fn forget_oldest(
    fetches: &mut HashMap<Key, Fetch>,
    since: impl Fn(&Key, &Fetch) -> Option<Instant>,
    most: usize,
) {
    let mut counted: Vec<(Instant, Key)> = fetches
        .iter()
        .filter_map(|(key, fetch)| since(key, fetch).map(|at| (at, *key)))
        .collect();
    counted.sort();
    let extra = counted.len().saturating_sub(most);
    for (_, key) in counted.into_iter().take(extra) {
        fetches.remove(&key);
    }
}

/// A lock per PC, made on first use (PRD 0015's queue).
#[derive(Default)]
pub struct Lanes(Mutex<HashMap<[u8; 32], Arc<Mutex<()>>>>);

impl Lanes {
    /// The lane for the PC with `key`.
    pub fn of(&self, key: &[u8; 32]) -> Arc<Mutex<()>> {
        Arc::clone(self.0.lock_anyway().entry(*key).or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_is_fetched_once_and_a_broken_one_goes_on() {
        let fetches = Fetches::default();
        let key = ([5; 32], 42);
        assert!(matches!(fetches.begin(key), Begin::Start));
        assert!(
            matches!(fetches.begin(key), Begin::Skip),
            "offered again while it runs"
        );
        let mut fetched = Fetched::default();
        fetched.summary.files = 7;
        let folder = std::env::temp_dir();
        fetches.broke(key, folder.clone(), fetched);
        let Begin::Resume(into, fetched) = fetches.begin(key) else {
            panic!("a broken fetch goes on");
        };
        assert_eq!((into, fetched.summary.files), (folder, 7));
        // All here: pasted again, not fetched again; fetched again once
        // the files are gone.
        let here = std::env::temp_dir();
        fetches.done(key, vec![here.clone()]);
        assert!(matches!(fetches.begin(key), Begin::Here(landed) if landed == [here]));
        fetches.done(key, vec![PathBuf::from("/no/such/file/for/mickey")]);
        assert!(matches!(fetches.begin(key), Begin::Start), "deleted since");
        fetches.forget(key);
        assert!(matches!(fetches.begin(key), Begin::Start), "forgotten");
    }

    #[test]
    fn transfers_from_one_pc_take_turns() {
        let lanes = Lanes::default();
        let (a, b) = (lanes.of(&[1; 32]), lanes.of(&[2; 32]));
        let _turn = a.lock_anyway();
        assert!(lanes.of(&[1; 32]).try_lock().is_err(), "the same PC waits");
        assert!(b.try_lock().is_ok(), "another PC goes on");
    }

    #[test]
    fn one_pc_has_only_so_many_transfers_going() {
        let fetches = Fetches::default();
        for id in 0..MAX_RUNNING as u64 {
            assert!(matches!(fetches.begin(([4; 32], id)), Begin::Start));
        }
        assert!(matches!(fetches.begin(([4; 32], 99)), Begin::TooMany));
        assert!(
            matches!(fetches.begin(([5; 32], 99)), Begin::Start),
            "another PC's are its own"
        );
        fetches.done(([4; 32], 0), Vec::new());
        assert!(matches!(fetches.begin(([4; 32], 99)), Begin::Start));
    }

    #[test]
    fn a_broken_transfer_whose_folder_is_gone_starts_over() {
        let fetches = Fetches::default();
        let key = ([6; 32], 1);
        fetches.begin(key);
        fetches.broke(key, "/no/such/folder/for/mickey".into(), Fetched::default());
        assert!(matches!(fetches.begin(key), Begin::Start));
    }

    #[test]
    fn another_pcs_broken_transfers_are_not_pushed_out() {
        let fetches = Fetches::default();
        let folder = std::env::temp_dir();
        fetches.begin(([1; 32], 0));
        fetches.broke(([1; 32], 0), folder.clone(), Fetched::default());
        for id in 0..MAX_BROKEN as u64 + 3 {
            let key = ([2; 32], id);
            fetches.begin(key);
            fetches.broke(key, folder.clone(), Fetched::default());
        }
        assert!(matches!(fetches.begin(([1; 32], 0)), Begin::Resume(..)));
    }

    #[test]
    fn only_the_newest_finished_transfers_are_remembered() {
        let fetches = Fetches::default();
        for id in 0..MAX_DONE as u64 + 5 {
            fetches.done(([3; 32], id), Vec::new());
        }
        assert_eq!(fetches.0.lock_anyway().len(), MAX_DONE);
    }

    #[test]
    fn only_the_newest_broken_transfers_are_kept() {
        let fetches = Fetches::default();
        let folder = std::env::temp_dir();
        for id in 0..MAX_BROKEN as u64 + 3 {
            let key = ([2; 32], id);
            fetches.begin(key);
            fetches.broke(key, folder.clone(), Fetched::default());
        }
        let broken = |fetch: &&Fetch| matches!(fetch, Fetch::Broken { .. });
        let kept = fetches.0.lock_anyway().values().filter(broken).count();
        assert_eq!(kept, MAX_BROKEN);
        let newest = ([2; 32], MAX_BROKEN as u64 + 2);
        assert!(matches!(fetches.begin(newest), Begin::Resume(..)));
        assert!(
            matches!(fetches.begin(([2; 32], 0)), Begin::Start),
            "the oldest starts over"
        );
    }

    #[test]
    fn an_offer_all_skipped_is_fetched_again_never_pasted_empty() {
        let fetches = Fetches::default();
        let key = ([3; 32], 1);
        fetches.begin(key);
        fetches.done(key, Vec::new());
        assert!(matches!(fetches.begin(key), Begin::Start));
    }
}
