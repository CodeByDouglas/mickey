//! The offers this PC handed out, which `serve` answers fetches for (PRD
//! 0015, #478).
//!
//! An offer's id is random, and only the PC the clipboard was carried to
//! learns it, over the encrypted link: it is what lets that PC fetch the
//! files. Another paired PC, which never got it, can fetch nothing, so the
//! files are read only when the clipboard crossed, as the clipboard itself
//! is (#185). The last few offers are served, so copying something else
//! while files are still fetched does not cut that transfer short; the same
//! files keep the offer they had.

use crate::threads::LockAnyway;
use crate::transfer::offer::{Listing, Offer};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// How many offers are served at once: the newest ones.
const KEPT: usize = 4;

/// The offers handed out, oldest first.
#[derive(Default)]
pub struct Offers(Mutex<VecDeque<(u64, Arc<Listing>)>>);

impl Offers {
    /// Hands out `listing` under a new random id, after the offers before
    /// (the oldest beyond `KEPT` go); the offer to send to the other PC.
    /// The same files, unchanged, keep their offer.
    pub fn hand_out(&self, listing: Listing) -> Result<Offer, String> {
        let mut held = self.0.lock_anyway();
        if let Some((id, same)) = held
            .iter()
            .find(|(_, held)| held.entries == listing.entries && held.local == listing.local)
        {
            return Ok(same.offer(*id));
        }
        let id = random_id()?;
        let offer = listing.offer(id);
        held.push_back((id, Arc::new(listing)));
        if held.len() > KEPT {
            held.pop_front();
        }
        Ok(offer)
    }

    /// The listing of offer `id`, if it is still served.
    pub fn find(&self, id: u64) -> Option<Arc<Listing>> {
        self.0
            .lock_anyway()
            .iter()
            .find(|(held, _)| *held == id)
            .map(|(_, listing)| Arc::clone(listing))
    }
}

/// A random id, from the system's secure generator.
fn random_id() -> Result<u64, String> {
    let bytes = crate::net::identity::random_bytes()?;
    let [a, b, c, d, e, f, g, h, ..] = bytes;
    Ok(u64::from_ne_bytes([a, b, c, d, e, f, g, h]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listing of one file of `size` bytes.
    fn one_file(size: u64) -> Listing {
        Listing {
            entries: vec![crate::transfer::offer::Entry {
                path: "a.txt".into(),
                kind: crate::transfer::offer::Kind::File { size },
            }],
            local: vec![("a.txt".into(), None)],
            skipped: Vec::new(),
        }
    }

    #[test]
    fn the_last_few_offers_are_found_by_their_ids() {
        let offers = Offers::default();
        let ids: Vec<u64> = (1..=KEPT as u64 + 1)
            .map(|size| offers.hand_out(one_file(size)).unwrap().id)
            .collect();
        assert!(offers.find(ids[0]).is_none(), "the oldest went");
        for id in &ids[1..] {
            assert!(offers.find(*id).is_some(), "still fetched, maybe");
        }
        assert!(offers.find(ids[1] ^ 1).is_none());
        assert_ne!(ids[1], ids[2], "random");
    }

    #[test]
    fn the_same_files_keep_their_offer() {
        let offers = Offers::default();
        let first = offers.hand_out(one_file(1)).unwrap();
        let again = offers.hand_out(one_file(1)).unwrap();
        assert_eq!(first, again, "a transfer of it goes on");
        assert!(offers.find(first.id).is_some());
    }
}
