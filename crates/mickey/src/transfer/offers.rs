//! The offers this PC handed out, which `serve` answers fetches for (PRD
//! 0015, #478).
//!
//! An offer's id is random, and only the PC the clipboard was carried to
//! learns it, over the encrypted link: it is what lets that PC fetch the
//! files. Another paired PC, which never got it, can fetch nothing, so the
//! files are read only when the clipboard crossed, as the clipboard itself
//! is (#185). Only the latest offer is served: copying something else
//! ends the one before, and the same files keep the offer they had.

use crate::threads::LockAnyway;
use crate::transfer::offer::{Listing, Offer};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Offers(Mutex<Option<(u64, Arc<Listing>)>>);

impl Offers {
    /// Hands out `listing` under a new random id, in place of the offer
    /// before; the offer to send to the other PC. The same files, unchanged,
    /// keep their offer: crossing again while they are fetched must not cut
    /// the transfer short.
    pub fn hand_out(&self, listing: Listing) -> Result<Offer, String> {
        let mut current = self.0.lock_anyway();
        if let Some((id, held)) = &*current
            && held.entries == listing.entries
            && held.local == listing.local
        {
            return Ok(held.offer(*id));
        }
        let id = random_id()?;
        let offer = listing.offer(id);
        *current = Some((id, Arc::new(listing)));
        Ok(offer)
    }

    /// The listing of offer `id`, if it is the one handed out last.
    pub fn find(&self, id: u64) -> Option<Arc<Listing>> {
        match &*self.0.lock_anyway() {
            Some((current, listing)) if *current == id => Some(Arc::clone(listing)),
            _ => None,
        }
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
    fn only_the_latest_offer_is_found_by_its_id() {
        let offers = Offers::default();
        let first = offers.hand_out(one_file(1)).unwrap();
        assert!(offers.find(first.id).is_some());
        let second = offers.hand_out(one_file(2)).unwrap();
        assert_ne!(first.id, second.id, "random");
        assert!(offers.find(first.id).is_none(), "copying again ends it");
        assert!(offers.find(second.id).is_some());
        assert!(offers.find(second.id ^ 1).is_none());
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
