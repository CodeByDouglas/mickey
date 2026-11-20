//! Never filling the disk (PRD 0015): a transfer starts only if it fits
//! with a safety margin to spare, and stops while that margin is still
//! free, even if another program writes at the same time.

use crate::platform::space::Space;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The most room a transfer leaves free: on a big disk 5% is a lot (21 GB
/// of 422 GB refused a 1.2 GB transfer with 18 GB free).
const MARGIN_CAP: u64 = 5 << 30;

/// The least room a transfer leaves free: 1 GB, or 5% of the disk, at most
/// 5 GB.
pub fn margin(total: u64) -> u64 {
    (1 << 30).max(total / 20).min(MARGIN_CAP)
}

/// How many bytes are missing for `need` bytes and the margin; 0 if it
/// fits.
pub fn missing(need: u64, space: Space) -> u64 {
    need.saturating_add(margin(space.total))
        .saturating_sub(space.available)
}

/// The error for a disk too full: it says how much room is missing.
pub fn full(missing: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::StorageFull,
        format!(
            "not enough room on the disk: {:.1} GB missing",
            missing as f64 / 1e9
        ),
    )
}

/// Bytes written between two looks at the disk, at most.
const CHECK_EVERY_BYTES: u64 = 64 << 20;
/// Time between two looks at the disk, at most.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Writes to `inner`, and every 64 MB or every second asks `space` how
/// much room is left: once less than the margin is, writing fails with
/// `StorageFull` instead of going on.
pub struct Guarded<W, S> {
    pub inner: W,
    pub space: S,
    unchecked: u64,
    checked_at: Instant,
}

impl<W, S: FnMut() -> io::Result<Space>> Guarded<W, S> {
    pub fn new(inner: W, space: S) -> Self {
        Guarded {
            inner,
            space,
            unchecked: 0,
            checked_at: Instant::now(),
        }
    }
}

impl<W: Write, S: FnMut() -> io::Result<Space>> Write for Guarded<W, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.unchecked >= CHECK_EVERY_BYTES || self.checked_at.elapsed() >= CHECK_EVERY {
            let space = (self.space)()?;
            if space.available < margin(space.total) {
                return Err(full(margin(space.total) - space.available));
            }
            self.unchecked = 0;
            self.checked_at = Instant::now();
        }
        let wrote = self.inner.write(buf)?;
        self.unchecked += wrote as u64;
        Ok(wrote)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn the_margin_is_a_gigabyte_or_five_percent_at_most_five() {
        assert_eq!(margin(10 * GB), GB);
        assert_eq!(margin(60 * GB), 3 * GB);
        assert_eq!(margin(422 * GB), 5 * GB, "not 21 GB");
    }

    #[test]
    fn what_is_missing_counts_the_margin() {
        let space = Space {
            available: 5 * GB,
            total: 100 * GB,
        };
        assert_eq!(missing(3 * GB, space), 3 * GB);
        assert_eq!(missing(0, space), 0, "5 GB free is exactly the margin");
        assert_eq!(missing(u64::MAX, space), u64::MAX - 5 * GB);
    }

    #[test]
    fn writing_stops_once_the_margin_is_reached() {
        // A disk that loses 32 MB of room for every 32 MB written.
        let left = std::cell::Cell::new(2 * GB);
        let mut written = 0u64;
        let mut guarded = Guarded::new(io::sink(), || {
            Ok(Space {
                available: left.get(),
                total: 10 * GB,
            })
        });
        let piece = vec![0u8; 32 << 20];
        let err = loop {
            match guarded.write(&piece) {
                Ok(n) => {
                    written += n as u64;
                    left.set(2 * GB - written);
                }
                Err(err) => break err,
            }
        };
        assert_eq!(err.kind(), io::ErrorKind::StorageFull);
        // It stopped within one check of the margin, never past the disk.
        assert!((GB - CHECK_EVERY_BYTES..=GB + CHECK_EVERY_BYTES).contains(&written));
    }
}
