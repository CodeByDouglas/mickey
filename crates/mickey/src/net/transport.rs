//! The connection from source to target: TCP carrying the secure link.

use crate::edges;
use crate::input::event::InputEvent;
use crate::input::held::HeldSet;
use crate::monitors::Monitor;
use crate::net::identity::Identity;
use crate::net::protocol::{Message, VERSION};
use crate::net::secure::Link;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// How long a send may wait for a target that stopped reading. The kernel
/// buffers megabytes first, so this only fires when the target is gone or
/// stuck, never on a slow network.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// A peer that is off does not answer at all; give up on it quickly so it
/// never delays the others.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Connects to a target and runs the secure handshake. The caller checks
/// the target's key (pairing) before saying `hello`.
/// `TCP_NODELAY` stops the kernel from holding back tiny frames to batch
/// them (Nagle's algorithm), which would add up to tens of milliseconds.
///
/// Sends fail after `WRITE_TIMEOUT` instead of blocking forever, so a
/// target that stops reading (asleep, crashed, malicious) can never keep
/// the source's keyboard grabbed: the error ends `mickey source`, which
/// drops the grabs.
pub fn connect(addr: SocketAddr, identity: &Identity) -> io::Result<Link> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Link::initiate(stream, identity)
}

/// Says hello and waits for the target's hello, which it sends only once it
/// trusts us. Until then nothing else is sent.
pub fn hello(link: &mut Link) -> io::Result<()> {
    link.send(&Message::Hello { version: VERSION })?;
    // The target may be showing its owner a pair window: give them time,
    // but not for ever (a frozen target would hold this up for good).
    link.set_read_timeout(Some(PAIR_WAIT))?;
    let answer = link.recv();
    link.set_read_timeout(None)?;
    match answer? {
        Message::Hello { version: VERSION } => Ok(()),
        Message::Hello { version } => Err(invalid(format!(
            "target speaks protocol {version}, this Mickey speaks {VERSION}"
        ))),
        _ => Err(invalid("target did not answer with Hello".into())),
    }
}

/// How long a new PC's owner has to answer the pair window.
const PAIR_WAIT: Duration = Duration::from_secs(90);

/// How long a source may say nothing before the target gives up on it. A
/// source pings every second, so only one that is gone (asleep, switched
/// off, off the network) is this quiet: its keys are let go and its
/// connection slot comes back (#452).
#[cfg(not(test))]
const SOURCE_SILENT: Duration = Duration::from_secs(10);
#[cfg(test)]
const SOURCE_SILENT: Duration = Duration::from_millis(300);

/// The target's side of the handshake, for one accepted connection. The
/// caller checks the source's key (pairing) before calling `serve`.
pub fn accept(stream: TcpStream, identity: &Identity) -> io::Result<Link> {
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Link::respond(stream, identity)
}

/// The data link (PRD 0011) listens on the port after the input link's.
pub fn data_address(input: SocketAddr) -> SocketAddr {
    SocketAddr::new(input.ip(), input.port().wrapping_add(1))
}

/// A bulk payload of megabytes takes longer to write than a key press.
const DATA_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens the data link to the target whose input link is at `input`.
pub fn connect_data(input: SocketAddr, identity: &Identity) -> io::Result<Link> {
    connect_paired(data_address(input), identity, Bulk::Data)
}

/// The target's side of a data link. Only a PC already paired may open
/// one (`trusted` says): a data link never asks to pair.
pub fn accept_data(
    stream: TcpStream,
    identity: &Identity,
    trusted: impl Fn(&[u8; 32]) -> bool,
) -> io::Result<Link> {
    accept_paired(stream, identity, trusted, Bulk::Data)
}

/// Transfers (PRD 0015) listen on the port after the data link's: the data
/// port keeps one link per PC (#452), and a transfer must not close it.
#[cfg_attr(not(test), expect(dead_code, reason = "transfers listen from #474 on"))]
pub fn transfer_address(input: SocketAddr) -> SocketAddr {
    SocketAddr::new(input.ip(), input.port().wrapping_add(2))
}

/// A transfer whose peer says nothing for this long has lost it (asleep,
/// unplugged): it pauses, to resume later, instead of waiting for ever.
/// Both sides send something more often than this while a transfer is on.
pub const TRANSFER_SILENT: Duration = Duration::from_secs(15);

/// Opens a transfer connection to the PC whose input link is at `input`.
#[cfg_attr(not(test), expect(dead_code, reason = "transfers fetch from #474 on"))]
pub fn connect_transfer(input: SocketAddr, identity: &Identity) -> io::Result<Link> {
    connect_paired(transfer_address(input), identity, Bulk::Transfer)
}

/// The side that serves a transfer. Like a data link, only a paired PC
/// may open one.
#[cfg_attr(not(test), expect(dead_code, reason = "transfers listen from #474 on"))]
pub fn accept_transfer(
    stream: TcpStream,
    identity: &Identity,
    trusted: impl Fn(&[u8; 32]) -> bool,
) -> io::Result<Link> {
    accept_paired(stream, identity, trusted, Bulk::Transfer)
}

/// The two kinds of bulk connection, which differ in how long a read may
/// wait.
#[derive(Clone, Copy)]
enum Bulk {
    /// The data link: waits for ever, as before (#452 closes old ones).
    Data,
    /// A transfer: reads give up after `TRANSFER_SILENT`.
    Transfer,
}

impl Bulk {
    /// Both send short messages that leave at once (`TCP_NODELAY`). Without
    /// it, a short message written right after another waits for the
    /// peer's delayed acknowledgement, and a bulk payload is two messages
    /// (its length, then its bytes). Measured on localhost, release build:
    /// a clipboard ask and its short answer took 82 ms instead of 0.4 ms
    /// (#485), a handshake 44 ms instead of 2 ms (#472).
    fn prepare(self, stream: &TcpStream) -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(DATA_WRITE_TIMEOUT))
    }

    fn name(self) -> &'static str {
        match self {
            Bulk::Data => "data link",
            Bulk::Transfer => "transfer",
        }
    }

    fn silent(self) -> Option<Duration> {
        match self {
            Bulk::Data => None,
            Bulk::Transfer => Some(TRANSFER_SILENT),
        }
    }
}

/// A secure link to `addr` for bulk data.
fn connect_paired(addr: SocketAddr, identity: &Identity, bulk: Bulk) -> io::Result<Link> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    bulk.prepare(&stream)?;
    let link = Link::initiate(stream, identity)?;
    link.set_read_timeout(bulk.silent())?;
    Ok(link)
}

/// The listening side of `connect_paired`: refuses a PC that is not
/// paired, after the handshake that proves its key.
fn accept_paired(
    stream: TcpStream,
    identity: &Identity,
    trusted: impl Fn(&[u8; 32]) -> bool,
    bulk: Bulk,
) -> io::Result<Link> {
    bulk.prepare(&stream)?;
    let link = Link::respond(stream, identity)?;
    if !trusted(link.peer_key()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("a PC that is not paired asked for a {}", bulk.name()),
        ));
    }
    link.set_read_timeout(bulk.silent())?;
    Ok(link)
}

/// What the target does for the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Input(InputEvent),
    /// Put the cursor at this point of the desktop.
    Place {
        x: i32,
        y: i32,
    },
    /// The source's keyboard and mouse now control this PC, or not any
    /// more (PRD 0009).
    Controlled(bool),
    /// The source sits at this point of this PC's desktop (PRD 0005).
    Arranged([i32; 2]),
}

/// Serves one source until it disconnects, passing every input event to
/// `act`. Whatever is still held when the connection ends, cleanly or not,
/// is released through `act` before returning. `monitors` are this PC's,
/// sent to the source after `Hello` so it can lay the PCs out and cross
/// their edges (PRD 0005), and again after a `Ping` whenever `moved` has
/// newer ones (#365). `cursor` says where this PC's cursor is, to notice
/// it pushing off the desktop; `None` when the system cannot say.
pub fn serve(
    link: &mut Link,
    monitors: &[Monitor],
    mut moved: impl FnMut() -> Option<Vec<Monitor>>,
    mut act: impl FnMut(Effect) -> bool,
    cursor: fn() -> Option<(i32, i32)>,
    still_trusted: impl Fn() -> bool,
) -> io::Result<()> {
    let first = link.recv().map_err(|err| match err.kind() {
        io::ErrorKind::UnexpectedEof => {
            invalid("the source left before saying hello (it may have refused to pair)".into())
        }
        _ => err,
    })?;
    match first {
        Message::Hello { version: VERSION } => {
            link.send(&Message::Hello { version: VERSION })?;
            link.send(&Message::monitors(monitors))?;
            link.set_read_timeout(Some(SOURCE_SILENT))?;
        }
        Message::Hello { version } => {
            return Err(invalid(format!(
                "peer speaks protocol {version}, this Mickey speaks {VERSION}"
            )));
        }
        _ => return Err(invalid("peer did not start with Hello".into())),
    }

    // The last Leave sent, so pushing against a wall sends it once.
    let mut left = None;
    let mut edge_check = edges::Throttle::new();
    let edge = edges::EdgeWatch::start(monitors.to_vec(), cursor);
    let mut refused_at = None;
    let mut keys = HeldSet::new();
    let mut buttons = HeldSet::new();
    let result = loop {
        let message = match link.recv() {
            Ok(message) => message,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break Ok(()),
            // Linux says WouldBlock, Windows TimedOut.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the source went silent (no ping for {SOURCE_SILENT:?})"),
                ));
            }
            Err(err) => break Err(err),
        };
        // Forgotten while connected (a lost laptop): nothing more from it,
        // and what it holds down is let go below.
        if !still_trusted() {
            break Err(invalid("this PC forgot the peer".into()));
        }
        match message {
            Message::Input(event) => {
                match event {
                    InputEvent::Key { code, pressed } => keys.set(code, pressed),
                    InputEvent::Button { code, pressed } => buttons.set(code, pressed),
                    InputEvent::Motion { .. } | InputEvent::Scroll { .. } => {}
                }
                // The system refused it: tell the source, at most once a
                // second, so the keyboard and mouse go back.
                if !act(Effect::Input(event))
                    && refused_at.is_none_or(|at: Instant| at.elapsed() > Duration::from_secs(1))
                {
                    refused_at = Some(Instant::now());
                    if let Err(err) = link.send(&Message::Refused) {
                        break Err(err);
                    }
                }
                if let InputEvent::Motion { dx, dy } = event
                    && let Some(delta) = edge_check.feed((dx, dy))
                {
                    edge.push(delta);
                }
                // An answer about earlier motion, if one came: the cursor
                // is still being pushed, so acting on it now is in time.
                if let Some(off) = edge.answer() {
                    // Never mid-drag: a button held would stay held over
                    // there, and the drag would end on the wrong PC
                    // (PRD 0005). Pushing on after letting go crosses.
                    let free = buttons.is_empty();
                    if let Some((x, y)) = off
                        && free
                        && left != off
                        && let Err(err) = link.send(&Message::Leave { x, y })
                    {
                        break Err(err);
                    }
                    left = if free { off } else { None };
                }
            }
            Message::ReleaseAll => release_all(&mut keys, &mut buttons, &mut act),
            Message::Ping { id } => {
                if let Err(err) = link.send(&Message::Pong { id }) {
                    break Err(err);
                }
                // The source pings once a second: a good time to tell it
                // this PC's monitors moved, so its layout and this PC's
                // edges follow them (#365).
                if let Some(found) = moved() {
                    if let Err(err) = link.send(&Message::monitors(&found)) {
                        break Err(err);
                    }
                    edge.set_monitors(found);
                }
            }
            Message::Enter { x, y } => {
                left = None;
                // Motion from before the cursor left is stale now: an
                // answer about it, or quick pushes summed but not yet
                // checked, read on the first motion back here, sent the
                // focus straight back through the edge it left by.
                let _ = edge.answer();
                edge_check = edges::Throttle::new();
                act(Effect::Place { x, y });
            }
            Message::Focus { here } => {
                act(Effect::Controlled(here));
            }
            Message::Arrange { x, y } => {
                act(Effect::Arranged([x, y]));
            }
            Message::Pong { .. }
            | Message::Monitors { .. }
            | Message::Leave { .. }
            | Message::Refused
            | Message::Picked { .. } => {}
            Message::Hello { .. } => break Err(invalid("second Hello".into())),
        }
    };
    release_all(&mut keys, &mut buttons, &mut act);
    result
}

fn release_all(keys: &mut HeldSet, buttons: &mut HeldSet, act: &mut impl FnMut(Effect) -> bool) {
    keys.release_all(|code| {
        act(Effect::Input(InputEvent::Key {
            code,
            pressed: false,
        }));
    });
    buttons.release_all(|code| {
        act(Effect::Input(InputEvent::Button {
            code,
            pressed: false,
        }));
    });
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// What the fake target returns: how `serve` ended and what it emitted.
    type Outcome = (io::Result<()>, Vec<InputEvent>);

    fn listener() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a free localhost port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        (listener, addr)
    }

    /// Starts a target with a fake emitter that records what it is given.
    fn fake_target() -> (SocketAddr, thread::JoinHandle<Outcome>) {
        fake_target_trusting(usize::MAX)
    }

    /// The same, but the source is forgotten after `messages` messages.
    fn fake_target_trusting(messages: usize) -> (SocketAddr, thread::JoinHandle<Outcome>) {
        let (listener, addr) = listener();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the test connects");
            let identity = Identity::generate().unwrap();
            let mut link = accept(stream, &identity).expect("handshake");
            let mut emitted = Vec::new();
            let trusted_for = std::cell::Cell::new(messages);
            let result = serve(
                &mut link,
                &[],
                || None,
                |effect| {
                    if let Effect::Input(event) = effect {
                        emitted.push(event);
                    }
                    true
                },
                || None,
                || {
                    let left = trusted_for.get();
                    trusted_for.set(left.saturating_sub(1));
                    left > 0
                },
            );
            (result, emitted)
        });
        (addr, handle)
    }

    /// A source that has said hello and read the target's monitors, which
    /// the target sends right after its own hello.
    fn source(addr: SocketAddr) -> Link {
        let mut link = connect(addr, &Identity::generate().unwrap()).unwrap();
        hello(&mut link).unwrap();
        assert!(matches!(link.recv().unwrap(), Message::Monitors { .. }));
        link
    }

    const KEY_A: InputEvent = InputEvent::Key {
        code: 30,
        pressed: true,
    };

    #[test]
    fn a_source_that_goes_silent_is_let_go_and_its_keys_released() {
        let (addr, target) = fake_target();
        let mut link = source(addr);
        link.send(&Message::Input(KEY_A)).unwrap();
        // Asleep with the key down: the connection stays open, nothing comes.
        let (result, emitted) = target.join().unwrap();
        let err = result.expect_err("a silent source is let go");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            emitted,
            [
                KEY_A,
                InputEvent::Key {
                    code: 30,
                    pressed: false
                }
            ],
            "the key it held is let go"
        );
        drop(link);
    }

    #[test]
    fn a_source_that_pings_is_kept() {
        let (addr, target) = fake_target();
        let mut link = source(addr);
        // Pings closer together than SOURCE_SILENT, for longer than it.
        // Each pong is read, as a source does: closing with unread data
        // would reset the connection instead of closing it.
        for id in 0..6 {
            link.send(&Message::Ping { id }).unwrap();
            assert_eq!(link.recv().unwrap(), Message::Pong { id });
            thread::sleep(SOURCE_SILENT / 3);
        }
        drop(link);
        let (result, _) = target.join().unwrap();
        assert!(
            result.is_ok(),
            "a source that keeps pinging is not cut off: {result:?}"
        );
    }

    #[test]
    fn events_cross_and_disconnect_releases_held_keys() {
        let (addr, target) = fake_target();
        let captured = [
            KEY_A,
            InputEvent::Motion { dx: 5, dy: -3 },
            InputEvent::Button {
                code: 0x110,
                pressed: true,
            },
            InputEvent::Button {
                code: 0x110,
                pressed: false,
            },
            InputEvent::Scroll {
                vertical: 120,
                horizontal: 0,
            },
        ];
        {
            let mut link = source(addr);
            for event in captured {
                link.send(&Message::Input(event)).unwrap();
            }
        } // the link is dropped here while key 30 is still held

        let (result, emitted) = target.join().unwrap();
        assert!(result.is_ok());
        let mut expected = captured.to_vec();
        expected.push(InputEvent::Key {
            code: 30,
            pressed: false,
        });
        assert_eq!(emitted, expected);
    }

    #[test]
    fn a_source_forgotten_while_connected_is_cut_off_and_its_keys_let_go() {
        let (addr, target) = fake_target_trusting(1);
        let mut link = source(addr);
        link.send(&Message::Input(KEY_A)).unwrap();
        let key_b = InputEvent::Key {
            code: 48,
            pressed: true,
        };
        // Forgotten now: this one must not reach the system.
        let _ = link.send(&Message::Input(key_b));
        let (result, emitted) = target.join().unwrap();
        assert!(result.is_err());
        let released = InputEvent::Key {
            code: 30,
            pressed: false,
        };
        assert_eq!(emitted, [KEY_A, released]);
    }

    #[test]
    fn the_target_places_on_enter_and_says_once_when_pushed_off() {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link = accept(stream, &Identity::generate().unwrap()).unwrap();
            let laptop = [Monitor {
                x: 0,
                y: 0,
                width: 1536,
                height: 864,
            }];
            let mut placed = Vec::new();
            // The cursor sits on the laptop's right edge.
            let result = serve(
                &mut link,
                &laptop,
                || None,
                |effect| {
                    if let Effect::Place { x, y } = effect {
                        placed.push((x, y));
                    }
                    true
                },
                || Some((1535, 284)),
                || true,
            );
            (result, placed)
        });
        let mut link = source(addr);
        link.send(&Message::Enter { x: 1535, y: 284 }).unwrap();
        let push = Message::Input(InputEvent::Motion { dx: 4, dy: 0 });
        link.send(&push).unwrap();
        // Still pushing a moment later: edges are checked at most every
        // EDGE_CHECK, with the motion summed in between.
        thread::sleep(edges::EDGE_CHECK + std::time::Duration::from_millis(2));
        link.send(&push).unwrap();
        // The answer comes from the edge thread: the next motion carries it.
        thread::sleep(std::time::Duration::from_millis(20));
        link.send(&push).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Leave { x: 1536, y: 284 });
        // Only once: the next message is the pong, not a second Leave.
        link.send(&Message::Ping { id: 7 }).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Pong { id: 7 });
        drop(link);
        let (result, placed) = target.join().unwrap();
        assert!(result.is_ok());
        assert_eq!(placed, [(1535, 284)]);
    }

    #[test]
    fn coming_back_does_not_leave_on_an_old_push() {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link = accept(stream, &Identity::generate().unwrap()).unwrap();
            let laptop = [Monitor {
                x: 0,
                y: 0,
                width: 1536,
                height: 864,
            }];
            // The cursor sits on the laptop's right edge.
            serve(
                &mut link,
                &laptop,
                || None,
                |_| true,
                || Some((1535, 284)),
                || true,
            )
        });
        let mut link = source(addr);
        let wait = || thread::sleep(edges::EDGE_CHECK + std::time::Duration::from_millis(2));
        let right = Message::Input(InputEvent::Motion { dx: 4, dy: 0 });
        let left = Message::Input(InputEvent::Motion { dx: -4, dy: 0 });
        link.send(&Message::Enter { x: 1535, y: 284 }).unwrap();
        for _ in 0..3 {
            wait();
            link.send(&right).unwrap();
        }
        assert_eq!(link.recv().unwrap(), Message::Leave { x: 1536, y: 284 });
        // Still pushing as the focus goes: an answer never read here, and
        // quick pushes summed but not yet checked.
        wait();
        link.send(&right).unwrap();
        for _ in 0..3 {
            link.send(&right).unwrap();
        }
        thread::sleep(std::time::Duration::from_millis(20));
        // Back on the same edge, moving away from it: no Leave.
        link.send(&Message::Enter { x: 1535, y: 284 }).unwrap();
        for _ in 0..2 {
            wait();
            link.send(&left).unwrap();
            // The edge thread answers; the next motion reads the answer.
            thread::sleep(std::time::Duration::from_millis(20));
        }
        link.send(&Message::Ping { id: 9 }).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Pong { id: 9 });
        drop(link);
        assert!(target.join().unwrap().is_ok());
    }

    #[test]
    fn no_leave_while_a_button_is_held() {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link = accept(stream, &Identity::generate().unwrap()).unwrap();
            let laptop = [Monitor {
                x: 0,
                y: 0,
                width: 1536,
                height: 864,
            }];
            serve(
                &mut link,
                &laptop,
                || None,
                |_| true,
                || Some((1535, 284)),
                || true,
            )
        });
        let mut link = source(addr);
        let push = Message::Input(InputEvent::Motion { dx: 4, dy: 0 });
        let left_button = |pressed| {
            Message::Input(InputEvent::Button {
                code: 0x110,
                pressed,
            })
        };
        let wait = || thread::sleep(edges::EDGE_CHECK + std::time::Duration::from_millis(2));
        // Dragging against the edge: no Leave.
        link.send(&left_button(true)).unwrap();
        for _ in 0..3 {
            wait();
            link.send(&push).unwrap();
        }
        link.send(&Message::Ping { id: 1 }).unwrap();
        assert_eq!(
            link.recv().unwrap(),
            Message::Pong { id: 1 },
            "no Leave mid-drag"
        );
        // Let go and push on: now it leaves.
        link.send(&left_button(false)).unwrap();
        wait();
        link.send(&push).unwrap();
        thread::sleep(std::time::Duration::from_millis(20));
        link.send(&push).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Leave { x: 1536, y: 284 });
        drop(link);
        assert!(target.join().unwrap().is_ok());
    }

    #[test]
    fn moved_monitors_follow_a_ping() {
        let moved_to = Monitor {
            x: 0,
            y: 845,
            width: 1080,
            height: 1920,
        };
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link = accept(stream, &Identity::generate().unwrap()).unwrap();
            // Moved once, then still.
            let mut news = Some(vec![moved_to]);
            serve(&mut link, &[], || news.take(), |_| true, || None, || true)
        });
        let mut link = source(addr);
        link.send(&Message::Ping { id: 1 }).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Pong { id: 1 });
        assert_eq!(link.recv().unwrap(), Message::monitors(&[moved_to]));
        link.send(&Message::Ping { id: 2 }).unwrap();
        assert_eq!(
            link.recv().unwrap(),
            Message::Pong { id: 2 },
            "sent once, not on every ping"
        );
        drop(link);
        assert!(target.join().unwrap().is_ok());
    }

    /// A data link from a source with `source` to a target that trusts the
    /// keys `trusted` says; returns both ends, or the target's error.
    fn data_link(source: Identity, trust: bool) -> (io::Result<Link>, io::Result<Link>) {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            accept_data(stream, &Identity::generate().unwrap(), |_| trust)
        });
        // `connect_data` dials the port after `addr`; test the pair directly.
        let stream = TcpStream::connect(addr).unwrap();
        let ours = Link::initiate(stream, &source);
        (ours, target.join().unwrap())
    }

    #[test]
    fn a_big_payload_crosses_the_data_link_both_ways() {
        let (ours, theirs) = data_link(Identity::generate().unwrap(), true);
        let (mut ours, mut theirs) = (ours.unwrap(), theirs.unwrap());
        // Three megabytes that are not all alike, so a lost chunk shows.
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let sender = thread::spawn(move || {
            ours.send_bulk(&big).unwrap();
            (ours, big)
        });
        let got = theirs.recv_bulk().unwrap();
        let (mut ours, big) = sender.join().unwrap();
        assert_eq!(got, big);
        theirs.send_bulk(b"").unwrap();
        assert_eq!(ours.recv_bulk().unwrap(), b"");
        assert!(
            ours.send_bulk(&vec![0; crate::net::secure::MAX_BULK + 1])
                .is_err()
        );
    }

    /// Bytes `i % 251` from where it is: read it to make them, write to it
    /// to check them, so a long stream is tested without holding it.
    struct Pattern(u64);

    impl io::Read for Pattern {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            for byte in buf.iter_mut() {
                *byte = (self.0 % 251) as u8;
                self.0 += 1;
            }
            Ok(buf.len())
        }
    }

    impl io::Write for Pattern {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for &byte in buf {
                if byte != (self.0 % 251) as u8 {
                    return Err(io::Error::other(format!("byte {} is wrong", self.0)));
                }
                self.0 += 1;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stream_bigger_than_a_bulk_payload_crosses_the_data_link() {
        let (ours, theirs) = data_link(Identity::generate().unwrap(), true);
        let (mut ours, mut theirs) = (ours.unwrap(), theirs.unwrap());
        let length = crate::net::secure::MAX_BULK as u64 + 3_000_001;
        let sender = thread::spawn(move || ours.send_stream(Pattern(0), length));
        let mut checked = Pattern(0);
        theirs.recv_stream(&mut checked, length).unwrap();
        sender.join().unwrap().unwrap();
        assert_eq!(checked.0, length);
    }

    /// A `Pattern` that stops after its first megabyte until `open` says
    /// so (or fails after five seconds).
    struct Gate {
        pattern: Pattern,
        open: std::sync::mpsc::Receiver<()>,
    }

    impl io::Read for Gate {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pattern.0 == 1 << 20 {
                self.open
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|_| io::Error::other("the peer was not heard"))?;
            }
            let room = if self.pattern.0 < 1 << 20 {
                buf.len().min((1 << 20) - self.pattern.0 as usize)
            } else {
                buf.len()
            };
            self.pattern.read(&mut buf[..room])
        }
    }

    #[test]
    fn the_peer_is_heard_in_the_middle_of_a_stream() {
        let (ours, theirs) = data_link(Identity::generate().unwrap(), true);
        let (mut ours, mut theirs) = (ours.unwrap(), theirs.unwrap());
        let mut listening = ours.try_clone().unwrap();
        let (heard, open) = std::sync::mpsc::channel();
        thread::spawn(move || {
            assert_eq!(listening.recv_bulk().unwrap(), b"cancel");
            heard.send(()).unwrap();
        });
        // The stream waits for the peer's message to be read: if it held
        // the lock throughout, that message could never be decrypted.
        let source = Gate {
            pattern: Pattern(0),
            open,
        };
        let length = 2 << 20;
        let sender = thread::spawn(move || ours.send_stream(source, length));
        theirs.send_bulk(b"cancel").unwrap();
        theirs.recv_stream(Pattern(0), length).unwrap();
        sender.join().unwrap().unwrap();
    }

    #[test]
    fn a_source_that_ends_early_is_an_error() {
        let (ours, _theirs) = data_link(Identity::generate().unwrap(), true);
        let err = ours.unwrap().send_stream(&b"abc"[..], 10).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_stream_longer_than_announced_writes_nothing() {
        let (ours, theirs) = data_link(Identity::generate().unwrap(), true);
        let (mut ours, mut theirs) = (ours.unwrap(), theirs.unwrap());
        ours.send_stream(&[7u8; 10][..], 10).unwrap();
        let mut sink = Vec::new();
        let err = theirs.recv_stream(&mut sink, 5).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(sink.is_empty());
    }

    #[test]
    fn waiting_for_the_peer_reads_nothing() {
        let (ours, theirs) = data_link(Identity::generate().unwrap(), true);
        let (mut ours, mut theirs) = (ours.unwrap(), theirs.unwrap());
        let short = Duration::from_millis(100);
        assert!(!ours.sends_within(short).unwrap(), "nothing sent yet");
        theirs.send_bulk(b"hello").unwrap();
        assert!(ours.sends_within(short).unwrap());
        assert!(ours.sends_within(short).unwrap(), "a peek reads nothing");
        assert_eq!(ours.recv_bulk().unwrap(), b"hello");
    }

    #[test]
    fn a_pc_not_paired_gets_no_data_link() {
        let (_, theirs) = data_link(Identity::generate().unwrap(), false);
        assert!(matches!(theirs, Err(err) if err.kind() == io::ErrorKind::PermissionDenied));
    }

    #[test]
    fn transfers_are_on_the_port_after_the_data_link() {
        assert_eq!(
            transfer_address("192.0.2.7:4242".parse().unwrap()),
            "192.0.2.7:4244".parse().unwrap()
        );
    }

    #[test]
    fn a_silent_peer_ends_a_read_after_the_timeout() {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link =
                accept_transfer(stream, &Identity::generate().unwrap(), |_| true).unwrap();
            assert_eq!(Bulk::Transfer.silent(), Some(TRANSFER_SILENT));
            // As `accept_transfer` sets it, only shorter for the test.
            link.set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let started = Instant::now();
            let err = link.recv_bulk().unwrap_err();
            (err.kind(), started.elapsed())
        });
        let _ours = Link::initiate(
            TcpStream::connect(addr).unwrap(),
            &Identity::generate().unwrap(),
        );
        let (kind, waited) = target.join().unwrap();
        assert!(matches!(
            kind,
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(waited < Duration::from_secs(2));
    }

    #[test]
    fn the_data_link_is_on_the_next_port() {
        assert_eq!(
            data_address("192.0.2.7:4242".parse().unwrap()),
            "192.0.2.7:4243".parse().unwrap()
        );
    }

    #[test]
    fn refused_input_is_reported_once_a_second() {
        let (listener, addr) = listener();
        let target = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut link = accept(stream, &Identity::generate().unwrap()).unwrap();
            // A system that refuses every injected event.
            serve(&mut link, &[], || None, |_| false, || None, || true)
        });
        let mut link = source(addr);
        link.send(&Message::Input(KEY_A)).unwrap();
        link.send(&Message::Input(KEY_A)).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Refused);
        // The second refusal, within the second, is not sent again.
        link.send(&Message::Ping { id: 3 }).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Pong { id: 3 });
        drop(link);
        assert!(target.join().unwrap().is_ok());
    }

    #[test]
    fn ping_gets_a_pong() {
        let (addr, target) = fake_target();
        let mut link = source(addr);
        link.send(&Message::Ping { id: 42 }).unwrap();
        assert_eq!(link.recv().unwrap(), Message::Pong { id: 42 });
        drop(link);
        assert!(target.join().unwrap().0.is_ok());
    }

    #[test]
    fn a_protocol_error_closes_the_link_and_still_releases() {
        let (addr, target) = fake_target();
        let mut link = source(addr);
        link.send(&Message::Input(KEY_A)).unwrap();
        link.send(&Message::Hello { version: VERSION }).unwrap(); // a second Hello

        let (result, emitted) = target.join().unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            emitted,
            [
                KEY_A,
                InputEvent::Key {
                    code: 30,
                    pressed: false
                }
            ]
        );
    }

    #[test]
    fn a_target_that_stops_reading_makes_send_fail() {
        let (listener, addr) = listener();
        // Answers the handshake, then never reads, until the test ends.
        let stuck = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the test connects");
            accept(stream, &Identity::generate().unwrap())
        });
        let mut link = connect(addr, &Identity::generate().unwrap()).unwrap();
        let event = Message::Input(InputEvent::Motion { dx: 1, dy: 1 });
        let failed = (0..10_000_000).find_map(|_| link.send(&event).err());
        let kind = failed
            .expect("send must fail once the buffers are full")
            .kind();
        // Linux reports a send timeout as WouldBlock, Windows as TimedOut.
        assert!(matches!(
            kind,
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        drop(stuck);
    }

    #[test]
    fn wrong_version_is_refused() {
        let (addr, target) = fake_target();
        let stream = TcpStream::connect(addr).unwrap();
        let mut link = Link::initiate(stream, &Identity::generate().unwrap()).unwrap();
        link.send(&Message::Hello {
            version: VERSION + 1,
        })
        .unwrap();
        assert!(target.join().unwrap().0.is_err());
    }
}
