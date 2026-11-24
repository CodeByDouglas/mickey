//! Asking for the next files before the one arriving is done (PRD 0015).
//!
//! The other PC answers fetches one at a time, in the order they came.
//! Asking for one file only once the one before has arrived costs a round
//! trip per file, which for thousands of small files is most of the time.
//! So the next few are asked for while one arrives: their answers queue up
//! behind it, and the other PC never waits for the next question.
//!
//! A file this PC has part of is asked for only in its turn: what is here
//! is read again first (to hash it), and the answers to files asked ahead
//! would pile up meanwhile.

use crate::net::secure::Link;
use crate::transfer::wire::{self, Message};
use std::collections::VecDeque;
use std::io;

/// How many files are asked for ahead of the one arriving. Each answer is
/// held by the network until its turn, so a few are enough to hide the
/// round trip.
const ASK_AHEAD: usize = 8;

/// The fetches of one offer: those to come, and those asked for already.
pub struct Asking {
    offer: u64,
    /// The files still to fetch, in order, each with whether it may be
    /// asked for ahead (false for one this PC has part of).
    to_come: VecDeque<(u32, bool)>,
    /// Asked for and not arrived yet, in the order they will.
    asked: VecDeque<u32>,
}

impl Asking {
    /// For `offer`, whose files `to_come` will be fetched in that order.
    pub fn new(offer: u64, to_come: impl IntoIterator<Item = (u32, bool)>) -> Self {
        Asking {
            offer,
            to_come: to_come.into_iter().collect(),
            asked: VecDeque::new(),
        }
    }

    /// Makes sure file `index` is asked for from byte `from` (unless it was
    /// asked for ahead, from the start) and is the next to arrive; then
    /// asks for the next few.
    pub fn now(&mut self, link: &mut Link, index: u32, from: u64) -> io::Result<()> {
        match self.asked.pop_front() {
            Some(asked) if asked == index && from == 0 => {}
            // Not `InvalidData`, which would throw away the `.part` just
            // made without forgetting it, and fail every next attempt.
            Some(_) => {
                let order = format!("file {index} was not the next asked for");
                return Err(io::Error::other(order));
            }
            None => self.fetch(link, index, from)?,
        }
        while self.to_come.front().is_some_and(|&(next, _)| next <= index) {
            self.to_come.pop_front();
        }
        while self.asked.len() < ASK_AHEAD {
            match self.to_come.front() {
                Some(&(next, true)) => {
                    self.fetch(link, next, 0)?;
                    self.asked.push_back(next);
                    self.to_come.pop_front();
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn fetch(&self, link: &mut Link, index: u32, from: u64) -> io::Result<()> {
        let offer = self.offer;
        wire::send(link, &Message::Fetch { offer, index, from })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::link_pair;

    /// The fetches that reach the other end of `link`, as (index, from),
    /// once it is dropped.
    fn fetches_seen(run: impl FnOnce(&mut Link)) -> Vec<(u32, u64)> {
        let (mut ours, mut theirs) = link_pair();
        let seen = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while let Ok(Message::Fetch { offer, index, from }) = wire::recv(&mut theirs) {
                assert_eq!(offer, 7);
                seen.push((index, from));
            }
            seen
        });
        run(&mut ours);
        drop(ours);
        seen.join().unwrap()
    }

    #[test]
    fn the_next_files_are_asked_for_ahead_up_to_one_to_resume() {
        let seen = fetches_seen(|link| {
            // File 4 is here in part: it is asked for in its turn.
            let mut asking = Asking::new(7, [(1, true), (2, true), (4, false), (5, true)]);
            asking.now(link, 1, 0).unwrap();
            asking.now(link, 2, 0).unwrap();
            asking.now(link, 4, 500).unwrap();
            asking.now(link, 5, 0).unwrap();
        });
        assert_eq!(seen, [(1, 0), (2, 0), (4, 500), (5, 0)]);
    }

    #[test]
    fn only_so_many_are_asked_for_ahead() {
        let seen = fetches_seen(|link| {
            let mut asking = Asking::new(7, (0..20).map(|index| (index, true)));
            asking.now(link, 0, 0).unwrap();
        });
        assert_eq!(seen.len(), 1 + ASK_AHEAD);
        assert_eq!(seen.last(), Some(&(ASK_AHEAD as u32, 0)), "in order");
    }

    #[test]
    fn a_file_out_of_turn_is_an_error_not_a_mixup() {
        fetches_seen(|link| {
            let mut asking = Asking::new(7, [(1, true), (2, true)]);
            asking.now(link, 1, 0).unwrap();
            assert!(asking.now(link, 3, 0).is_err(), "2 comes first");
        });
    }
}
