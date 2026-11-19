//! How far a transfer is, for the window that shows it (PRD 0015): the
//! stage it is at, the bytes so far, the speed and the time left.
//!
//! Only real stages: nothing is packed (no compression), and encryption
//! happens piece by piece while sending, so neither is a stage. The speed
//! is the average over the last few seconds, not the last instant's,
//! which jumps around too much to say how long is left.

use crate::threads::LockAnyway;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A transfer's progress, written by the transfer, read by the window.
pub type Shared = Arc<Mutex<Progress>>;

/// What a transfer is doing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    /// Waiting for another transfer from the same PC to end.
    #[default]
    Queued,
    /// Checking the disk has room.
    CheckingSpace,
    /// Reading again what arrived before a break, to resume.
    Rereading,
    Transferring,
    /// Stopped by the network or the disk; fetching again resumes it.
    Paused,
    Done,
}

/// The window the speed is averaged over.
const WINDOW: Duration = Duration::from_secs(5);

/// A transfer's progress.
#[derive(Debug, Default)]
pub struct Progress {
    pub stage: Stage,
    /// Bytes this PC has of all the offer's files, and their total.
    pub done: u64,
    pub total: u64,
    /// When `done` was what, over the last `WINDOW`, oldest first.
    samples: VecDeque<(Instant, u64)>,
}

impl Progress {
    pub fn new(total: u64) -> Progress {
        Progress {
            total,
            ..Progress::default()
        }
    }

    /// `done` bytes at `now`.
    pub fn advance(&mut self, done: u64, now: Instant) {
        self.done = done.min(self.total);
        self.samples.push_back((now, self.done));
        while self
            .samples
            .front()
            .is_some_and(|&(at, _)| now.duration_since(at) > WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Bytes per second over the last few seconds; `None` until there is
    /// a second of samples to average.
    pub fn speed(&self) -> Option<f64> {
        let (&(first_at, first), &(last_at, last)) = (self.samples.front()?, self.samples.back()?);
        let seconds = last_at.duration_since(first_at).as_secs_f64();
        (seconds >= 1.0).then(|| (last - first) as f64 / seconds)
    }

    /// The time left at the current speed; `None` while the speed is not
    /// known or zero.
    pub fn time_left(&self) -> Option<Duration> {
        let speed = self.speed().filter(|&speed| speed > 0.0)?;
        Some(Duration::from_secs_f64(
            (self.total - self.done) as f64 / speed,
        ))
    }
}

/// Writes to `inner`, counting every byte written as done in `progress`.
pub struct Counted<'a, W> {
    pub inner: W,
    pub progress: &'a Mutex<Progress>,
}

impl<W: Write> Write for Counted<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let wrote = self.inner.write(buf)?;
        let mut progress = self.progress.lock_anyway();
        let done = progress.done + wrote as u64;
        progress.advance(done, Instant::now());
        Ok(wrote)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_is_the_average_of_the_last_seconds() {
        let start = Instant::now();
        let mut progress = Progress::new(1000);
        progress.advance(0, start);
        assert_eq!(progress.speed(), None, "not one second yet");
        // 10 bytes a second for 8 seconds, then a burst.
        for second in 1..=8 {
            progress.advance(second * 10, start + Duration::from_secs(second));
        }
        assert_eq!(progress.speed(), Some(10.0));
        progress.advance(200, start + Duration::from_secs(9));
        // The window is seconds 4 to 9: 160 bytes in 5 seconds.
        assert_eq!(progress.speed(), Some(32.0));
        assert_eq!(progress.time_left(), Some(Duration::from_secs(25)));
    }

    #[test]
    fn no_time_left_while_nothing_moves() {
        let start = Instant::now();
        let mut progress = Progress::new(100);
        progress.advance(5, start);
        progress.advance(5, start + Duration::from_secs(2));
        assert_eq!(progress.speed(), Some(0.0));
        assert_eq!(progress.time_left(), None);
    }

    #[test]
    fn what_is_written_counts_as_done() {
        let progress = Mutex::new(Progress::new(100));
        let mut counted = Counted {
            inner: io::sink(),
            progress: &progress,
        };
        counted.write_all(&[0; 30]).unwrap();
        counted.write_all(&[0; 12]).unwrap();
        assert_eq!(progress.lock_anyway().done, 42);
    }

    #[test]
    fn done_never_passes_the_total() {
        let mut progress = Progress::new(10);
        progress.advance(50, Instant::now());
        assert_eq!(progress.done, 10);
    }
}
