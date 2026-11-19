//! Where other PCs fetch the files copied on this one (PRD 0015): a port
//! of its own, the input port + 2, so a transfer never closes the
//! clipboard's data link.
//!
//! Each connection is a separate transfer on its own thread. Only paired
//! PCs get past the handshake, and at most `MAX_PER_PC` at once from one.

use crate::net::identity::Identity;
use crate::net::secure::Link;
use crate::net::transport;
use crate::target::slots::{self, SharedSlots, Slot};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

/// The most transfers one PC may have open here at once: one running and
/// one starting while the last one ends.
pub const MAX_PER_PC: usize = 2;

/// Listens for transfers on the port for `listen` (the input link's) and
/// hands each paired PC's link to `handle`, on its own thread. Returns once
/// it listens; the listening goes on for as long as Mickey runs.
pub fn serve(
    listen: SocketAddr,
    identity: Arc<Identity>,
    trusted: impl Fn(&[u8; 32]) -> bool + Clone + Send + 'static,
    handle: impl Fn(Link) + Clone + Send + 'static,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(transport::transfer_address(listen))?;
    crate::threads::spawn("transfer accept", move || {
        let slots = SharedSlots::default();
        let refused_said = Mutex::new(None);
        for stream in listener.incoming().flatten() {
            let Ok(from) = stream.peer_addr() else {
                continue;
            };
            // Counted before the handshake: a PC not paired may hold one
            // only for the few seconds the handshake allows.
            let Some(slot) = Slot::take_up_to(&slots, from.ip(), MAX_PER_PC) else {
                slots::say_refused(&refused_said, from.ip(), MAX_PER_PC);
                continue;
            };
            let (identity, trusted, handle) = (identity.clone(), trusted.clone(), handle.clone());
            crate::threads::try_spawn("transfer", move || {
                let _slot = slot;
                match transport::accept_transfer(stream, &identity, trusted) {
                    Ok(link) => handle(link),
                    Err(err) => eprintln!("mickey: transfer refused: {err}"),
                }
            });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A free port for the input link, with the two after it free too.
    fn free_input_port() -> SocketAddr {
        loop {
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = probe.local_addr().unwrap();
            drop(probe);
            if TcpListener::bind(transport::transfer_address(addr)).is_ok() {
                return addr;
            }
        }
    }

    /// Serves on a free port; each link is held until the test ends, and
    /// its peer's key sent on `got`.
    fn serving(trust: bool) -> (SocketAddr, mpsc::Receiver<[u8; 32]>) {
        let listen = free_input_port();
        let (got, links) = mpsc::channel();
        let identity = Arc::new(Identity::generate().unwrap());
        serve(
            listen,
            identity,
            move |_| trust,
            move |link: Link| {
                let _ = got.send(*link.peer_key());
                std::thread::sleep(Duration::from_secs(3));
            },
        )
        .unwrap();
        (listen, links)
    }

    #[test]
    fn a_paired_pc_gets_two_transfers_at_once_and_no_third() {
        let (listen, links) = serving(true);
        let ours = Identity::generate().unwrap();
        let first = transport::connect_transfer(listen, &ours).unwrap();
        let second = transport::connect_transfer(listen, &ours).unwrap();
        for _ in 0..2 {
            let key = links.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(key, *ours.public(), "the link is ours, by key");
        }
        assert!(
            transport::connect_transfer(listen, &ours).is_err(),
            "a third one is closed before its handshake"
        );
        drop((first, second));
    }

    #[test]
    fn a_pc_not_paired_gets_no_transfer() {
        let (listen, links) = serving(false);
        let mut link = transport::connect_transfer(listen, &Identity::generate().unwrap()).unwrap();
        // The handshake ends on both sides; the other one then hangs up.
        assert!(link.recv_bulk().is_err());
        assert!(links.recv_timeout(Duration::from_millis(300)).is_err());
    }
}
