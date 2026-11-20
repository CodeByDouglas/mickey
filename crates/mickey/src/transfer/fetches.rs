//! What became of each offer fetched here, and one lane per PC files come
//! from (PRD 0015): an offer that crosses again is fetched once, one that
//! broke goes on from where it stopped (#475, #502), and transfers from
//! one PC take turns.

use crate::threads::LockAnyway;
use crate::transfer::receive::Fetched;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Each offer fetched here, by the offering PC's key and the offer's id.
#[derive(Default)]
pub struct Fetches(Mutex<HashMap<Key, Fetch>>);

/// An offer: the offering PC's key and the offer's id.
pub type Key = ([u8; 32], u64);

/// What became of one offer.
enum Fetch {
    /// Being fetched now.
    Running,
    /// Stopped (the network, a sleeping PC, a full disk, Cancel): what it
    /// had done, and where, to go on.
    Broken {
        into: PathBuf,
        fetched: Box<Fetched>,
    },
    /// All there: where its top-level files and folders landed.
    Done(Vec<PathBuf>),
}

/// What to do with an offer that arrives.
pub enum Begin {
    /// Nothing: it is being fetched.
    Skip,
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
            Some(Fetch::Done(landed)) if landed.iter().all(|path| path.exists()) => {
                return Begin::Here(landed.clone());
            }
            _ => {}
        }
        match fetches.insert(key, Fetch::Running) {
            Some(Fetch::Broken { into, fetched }) => Begin::Resume(into, fetched),
            _ => Begin::Start,
        }
    }

    /// Records that `key` is all here, at `landed`.
    pub fn done(&self, key: Key, landed: Vec<PathBuf>) {
        self.0.lock_anyway().insert(key, Fetch::Done(landed));
    }

    /// Records that `key` broke, into `into`, with what it had done.
    pub fn broke(&self, key: Key, into: PathBuf, fetched: Fetched) {
        let fetched = Box::new(fetched);
        self.0
            .lock_anyway()
            .insert(key, Fetch::Broken { into, fetched });
    }

    /// The folders of transfers that broke and wait to go on: never
    /// cleaned away, or they would start over.
    pub fn waiting(&self) -> Vec<PathBuf> {
        self.0
            .lock_anyway()
            .values()
            .filter_map(|fetch| match fetch {
                Fetch::Broken { into, .. } => Some(into.clone()),
                _ => None,
            })
            .collect()
    }

    /// Forgets `key`: the owner said Ignore, or it could not start.
    pub fn forget(&self, key: Key) {
        self.0.lock_anyway().remove(&key);
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
        fetches.broke(key, "/somewhere".into(), fetched);
        let Begin::Resume(into, fetched) = fetches.begin(key) else {
            panic!("a broken fetch goes on");
        };
        assert_eq!(
            (into, fetched.summary.files),
            (PathBuf::from("/somewhere"), 7)
        );
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
}
