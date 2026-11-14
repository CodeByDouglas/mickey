//! Placing and following the cursor through an absolute uinput pointer, on
//! desktops with neither a RemoteDesktop portal nor a compositor IPC to
//! place it (COSMIC on the Pop!_OS 24.04 ISO, #459). As on the portal path
//! (PRD 0004), Mickey keeps the position itself: it adds each motion, keeps
//! it on the monitors (`portal::slide`) and puts the cursor there, so it
//! always knows where it is and sees it pushed off an edge.
//!
//! The pointer spans the box around every monitor; the compositor maps it
//! onto the whole desktop, as it does a drawing tablet. Checked on one
//! monitor (COSMIC): a point placed this way is the exact pixel.

use crate::monitors::Monitor;
use crate::platform::uinput::{ABS_X, ABS_Y, EV_ABS, VirtualDevice};
use crate::threads::LockAnyway;
use std::io;
use std::sync::Mutex;

struct Absolute {
    device: VirtualDevice,
    monitors: Vec<Monitor>,
    /// The desktop point the pointer's 0, 0 stands for.
    origin: (i32, i32),
    /// Where Mickey last put the cursor.
    at: Option<(i32, i32)>,
    /// The values the pointer last sent, on its own axes (it starts at 0).
    sent: (i32, i32),
}

/// Made on first use, from this PC's monitors at that moment.
static ABSOLUTE: Mutex<Option<Absolute>> = Mutex::new(None);

/// When making the pointer last failed: it is not tried again for a few
/// seconds, so a PC that cannot make one is not asked on every motion.
static FAILED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// When Mickey last put the cursor somewhere.
static PLACED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Whether Mickey put the cursor somewhere in the last `within`: coming
/// home, it puts it on the edge, under the strip there, and that must not
/// count as a push onto the edge (`edges::strips`).
pub fn placed_within(within: std::time::Duration) -> bool {
    PLACED.lock_anyway().is_some_and(|at| at.elapsed() < within)
}

/// The box around `monitors`: its top-left corner and its size.
fn span(monitors: &[Monitor]) -> Option<((i32, i32), (i32, i32))> {
    let left = monitors.iter().map(|m| m.x).min()?;
    let top = monitors.iter().map(|m| m.y).min()?;
    let right = monitors.iter().map(|m| m.x + m.width).max()?;
    let bottom = monitors.iter().map(|m| m.y + m.height).max()?;
    Some(((left, top), (right - left, bottom - top)))
}

/// Runs `f` on the pointer, making it first if needed.
fn with<T>(f: impl FnOnce(&mut Absolute) -> io::Result<T>) -> io::Result<T> {
    let mut absolute = ABSOLUTE.lock_anyway();
    if absolute.is_none() {
        if FAILED.lock_anyway().is_some_and(|at| at.elapsed() < RETRY) {
            return Err(io::Error::other("no absolute pointer on this PC"));
        }
        match make() {
            Ok(made) => *absolute = Some(made),
            Err(err) => {
                *FAILED.lock_anyway() = Some(std::time::Instant::now());
                return Err(err);
            }
        }
    }
    f(absolute.as_mut().expect("made just above"))
}

fn make() -> io::Result<Absolute> {
    let monitors = crate::monitors::this_pc().map_err(io::Error::other)?;
    let (origin, (width, height)) =
        span(&monitors).ok_or_else(|| io::Error::other("no monitors to place the cursor on"))?;
    let device = VirtualDevice::create_absolute(width, height)?;
    // The compositor needs a moment to notice a new device.
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(Absolute {
        device,
        monitors,
        origin,
        at: None,
        sent: (0, 0),
    })
}

/// The kernel drops an axis value equal to the one the device last sent,
/// but the cursor may have moved since (the touchpad of this PC): to place
/// it at the same point again, send a neighbour first, in the same frame.
fn nudge(sent: i32, wanted: i32) -> Option<i32> {
    (sent == wanted).then(|| if wanted > 0 { wanted - 1 } else { wanted + 1 })
}

/// Puts the cursor at `x, y` of the desktop, exactly.
pub fn place(x: i32, y: i32) -> io::Result<()> {
    with(|absolute| {
        let (left, top) = absolute.origin;
        let wanted = (x - left, y - top);
        for (axis, sent, value) in [
            (ABS_X, absolute.sent.0, wanted.0),
            (ABS_Y, absolute.sent.1, wanted.1),
        ] {
            if let Some(other) = nudge(sent, value) {
                absolute.device.emit(EV_ABS, axis, other);
            }
            absolute.device.emit(EV_ABS, axis, value);
        }
        *PLACED.lock_anyway() = Some(std::time::Instant::now());
        absolute.device.sync()?;
        absolute.sent = wanted;
        absolute.at = Some((x, y));
        Ok(())
    })
}

/// Moves the cursor by `(dx, dy)`, within the monitors.
pub fn move_by(dx: i32, dy: i32) -> io::Result<()> {
    let (monitors, at) = with(|absolute| Ok((absolute.monitors.clone(), absolute.at)))?;
    // Never placed yet: start from the middle of the first monitor.
    let Some(at) = at.or_else(|| {
        monitors
            .first()
            .map(|m| (m.x + m.width / 2, m.y + m.height / 2))
    }) else {
        return Ok(());
    };
    let (x, y) = crate::desktop::portal::slide(&monitors, at, (dx, dy));
    place(x, y)
}

/// Where Mickey last put the cursor, if it ever did.
pub fn cursor() -> Option<(i32, i32)> {
    ABSOLUTE.lock_anyway().as_ref()?.at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placing_at_the_same_point_again_sends_a_neighbour_first() {
        assert_eq!(nudge(100, 250), None);
        assert_eq!(nudge(250, 250), Some(249));
        assert_eq!(nudge(0, 0), Some(1));
    }

    #[test]
    fn the_pointer_spans_every_monitor() {
        let one = [Monitor {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        }];
        assert_eq!(span(&one), Some(((0, 0), (1280, 800))));
        let two = [
            Monitor {
                x: 0,
                y: 0,
                width: 2560,
                height: 1080,
            },
            Monitor {
                x: 2560,
                y: -840,
                width: 1080,
                height: 1920,
            },
        ];
        assert_eq!(span(&two), Some(((0, -840), (3640, 1920))));
        assert_eq!(span(&[]), None);
    }
}
