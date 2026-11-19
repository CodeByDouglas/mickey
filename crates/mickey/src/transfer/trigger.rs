//! Copy and paste triggers a transfer (PRD 0015, #478): a copied file list
//! crosses as an offer from the PC where the files are, and whoever ends up
//! with the clipboard fetches them from that PC, then pastes their paths.
//!
//! The clipboard carries a `FileOffer`: who offers (its key), where it
//! listens, and the offer. A PC that sends its own offer gives its port
//! only; the receiving PC takes the address from the link it came on, as
//! the group map does.

use crate::clipboard::{Content, Keeper};
use crate::net::identity::Identity;
use crate::net::transport;
use crate::net::trust;
use crate::threads::LockAnyway;
use crate::transfer::offer::{self, Kind, Offer};
use crate::transfer::offers::Offers;
use crate::transfer::receive::{self, Fetched};
use crate::transfer::wire::{self, Message};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

/// Above this, a transfer is asked about first (Bring / Ignore); until
/// that window exists (#478's next PR), it is not fetched at all.
pub const ASK_OVER: u64 = 1 << 30;

/// An offer as the clipboard carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffer {
    /// Where the offering PC listens (its input port; transfers use + 2).
    /// No IP: the PC the message came from.
    pub ip: Option<IpAddr>,
    pub port: u16,
    /// The offering PC's key: the transfer link must answer with it.
    pub key: [u8; 32],
    pub offer: Offer,
}

impl FileOffer {
    /// `port: u16`, the IP (`0`, or `4` and 4 bytes, or `6` and 16 bytes),
    /// `key: 32 bytes`, then the offer as `transfer::wire` encodes it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.port.to_be_bytes().to_vec();
        match self.ip {
            None => out.push(0),
            Some(IpAddr::V4(ip)) => {
                out.push(4);
                out.extend_from_slice(&ip.octets());
            }
            Some(IpAddr::V6(ip)) => {
                out.push(6);
                out.extend_from_slice(&ip.octets());
            }
        }
        out.extend_from_slice(&self.key);
        out.extend_from_slice(&wire::encode(&Message::Offer(self.offer.clone())));
        out
    }

    /// Everything here comes from another PC: an error, never a panic.
    pub fn decode(bytes: &[u8]) -> Result<FileOffer, String> {
        let short = || "a file offer that ends too soon".to_string();
        let (port, rest) = bytes.split_first_chunk::<2>().ok_or_else(short)?;
        let (&kind, rest) = rest.split_first().ok_or_else(short)?;
        let (ip, rest) = match kind {
            0 => (None, rest),
            4 => {
                let (ip, rest) = rest.split_first_chunk::<4>().ok_or_else(short)?;
                (Some(IpAddr::from(*ip)), rest)
            }
            6 => {
                let (ip, rest) = rest.split_first_chunk::<16>().ok_or_else(short)?;
                (Some(IpAddr::from(*ip)), rest)
            }
            _ => return Err(format!("unknown address kind {kind} in a file offer")),
        };
        let (key, rest) = rest.split_first_chunk::<32>().ok_or_else(short)?;
        let Message::Offer(offer) = wire::decode(rest)? else {
            return Err("a file offer without its offer".into());
        };
        Ok(FileOffer {
            ip,
            port: u16::from_be_bytes(*port),
            key: *key,
            offer,
        })
    }

    /// Where to fetch from: its IP, or `from` (the PC the message came
    /// on) if it has none.
    pub fn address(&self, from: IpAddr) -> SocketAddr {
        SocketAddr::new(self.ip.unwrap_or(from), self.port)
    }
}

/// What a PC needs to offer the files copied on it and to fetch others'.
pub struct Transfers {
    pub offers: Arc<Offers>,
    pub identity: Arc<Identity>,
    /// This PC's input port, which its offers name.
    pub port: u16,
    /// Where fetched files go, a fresh folder per transfer: the received
    /// files' folder (`clipboard::received::base`).
    pub received: PathBuf,
    /// Files are fetched only from a paired PC.
    pub trusted: trust::Shared,
}

impl Transfers {
    /// An offer of the files at `paths`, handed out by this PC, in place of
    /// the one before. The IP is left out: the receiving PC knows it.
    pub fn offer(&self, paths: &[PathBuf]) -> Result<FileOffer, String> {
        let listing = offer::list(paths)?;
        // How many, never their names: what is copied stays out of the log
        // (CONTRIBUTING, the clipboard); the names are for the window.
        if !listing.skipped.is_empty() {
            println!(
                "mickey: {} copied files or folders not offered (links, names Windows refuses, unreadable)",
                listing.skipped.len()
            );
        }
        Ok(FileOffer {
            ip: None,
            port: self.port,
            key: *self.identity.public(),
            offer: self.offers.hand_out(listing)?,
        })
    }

    /// Fetches `offer` (which came from the PC at `from`) on a thread of its
    /// own, and once every file is there and verified puts them on the
    /// clipboard, unless something else was copied meanwhile.
    pub fn fetch(self: &Arc<Self>, offer: FileOffer, from: IpAddr, keeper: Keeper) {
        let size = total(&offer.offer);
        if size > ASK_OVER {
            println!(
                "mickey: files of {:.1} GB offered; over {} GB they will be asked about first, not yet built (#478)",
                size as f64 / 1e9,
                ASK_OVER >> 30
            );
            return;
        }
        let this = Arc::clone(self);
        crate::threads::spawn("transfer fetch", move || {
            let before = keeper.read();
            match this.fetch_now(&offer, from) {
                Ok(landed) if keeper.read() == before => keeper.write(Content::Files(landed)),
                Ok(landed) => println!(
                    "mickey: files arrived in {}, not pasted: something else was copied meanwhile",
                    landed
                        .first()
                        .and_then(|path| path.parent())
                        .map_or_else(String::new, |folder| folder.display().to_string())
                ),
                Err(err) => eprintln!("mickey: files not fetched: {err}"),
            }
        });
    }

    /// Fetches `offer` into a fresh folder for received files; where its
    /// top-level files and folders landed.
    fn fetch_now(&self, offer: &FileOffer, from: IpAddr) -> Result<Vec<PathBuf>, String> {
        if !self.trusted.lock_anyway().is_trusted(&offer.key) {
            return Err("files offered by a PC that is not paired".into());
        }
        let at = offer.address(from);
        let mut link = transport::connect_transfer(at, &self.identity).map_err(|err| {
            format!(
                "cannot reach {} on TCP {} (a firewall?): {err}",
                at.ip(),
                transport::transfer_address(at).port()
            )
        })?;
        // Whoever took over that address gets nothing.
        if *link.peer_key() != offer.key {
            return Err(format!("{} answered with another PC's key", at.ip()));
        }
        let into = crate::clipboard::received::new_folder(&self.received)?;
        let mut fetched = Fetched::default();
        receive::fetch(&mut link, &offer.offer, &into, &mut fetched)
            .map_err(|err| err.to_string())?;
        let summary = &fetched.summary;
        println!(
            "mickey: {} files, {:.1} MB, verified, in {}",
            summary.files,
            summary.bytes as f64 / 1e6,
            into.display()
        );
        Ok(summary.landed.clone())
    }
}

/// The bytes of every file in `offer`.
fn total(offer: &Offer) -> u64 {
    offer.entries.iter().fold(0, |sum, entry| match entry.kind {
        Kind::File { size } => sum.saturating_add(size),
        Kind::Folder => sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::offer::Entry;

    fn file_offer(ip: Option<IpAddr>) -> FileOffer {
        FileOffer {
            ip,
            port: 4242,
            key: [7; 32],
            offer: Offer {
                id: 99,
                entries: vec![
                    Entry {
                        path: "fotos".into(),
                        kind: Kind::Folder,
                    },
                    Entry {
                        path: "fotos/a.jpg".into(),
                        kind: Kind::File { size: 5 },
                    },
                ],
            },
        }
    }

    #[test]
    fn a_file_offer_reads_back_with_or_without_an_address() {
        for ip in [
            None,
            Some("192.0.2.7".parse().unwrap()),
            Some("fe80::1".parse().unwrap()),
        ] {
            let sent = file_offer(ip);
            assert_eq!(FileOffer::decode(&sent.encode()).unwrap(), sent);
        }
    }

    #[test]
    fn a_broken_file_offer_is_an_error() {
        let bytes = file_offer(None).encode();
        for cut in 0..bytes.len() {
            assert!(FileOffer::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut bad = bytes.clone();
        bad[2] = 5;
        assert!(FileOffer::decode(&bad).is_err());
    }

    #[test]
    fn the_address_falls_back_to_the_sending_pc() {
        let from: IpAddr = "192.0.2.9".parse().unwrap();
        assert_eq!(
            file_offer(None).address(from),
            "192.0.2.9:4242".parse().unwrap()
        );
        let given = file_offer(Some("192.0.2.7".parse().unwrap()));
        assert_eq!(given.address(from), "192.0.2.7:4242".parse().unwrap());
    }

    /// A fresh folder under the system's temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A list of trusted PCs kept in `dir`, holding `key` if given.
    fn trusting(dir: &std::path::Path, key: Option<[u8; 32]>) -> trust::Shared {
        std::fs::create_dir_all(dir).unwrap();
        let mut peers = trust::TrustedPeers::load(dir).unwrap();
        if let Some(key) = key {
            peers.add(key, "test").unwrap();
        }
        Arc::new(std::sync::Mutex::new(peers))
    }

    #[test]
    fn files_offered_by_a_pc_not_paired_are_not_fetched() {
        let scratch =
            Scratch(std::env::temp_dir().join(format!("mickey-unpaired-{}", std::process::id())));
        let b = Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: 1,
            received: scratch.0.join("received"),
            trusted: trusting(&scratch.0.join("trust"), None),
        };
        // Refused before any connection: nothing listens at that address.
        let err = b
            .fetch_now(&file_offer(None), "127.0.0.1".parse().unwrap())
            .unwrap_err();
        assert!(err.contains("not paired"), "{err}");
        assert!(!scratch.0.join("received").exists());
    }

    /// A free port for an input link, with the transfer port after it free.
    fn free_input_port() -> SocketAddr {
        loop {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = probe.local_addr().unwrap();
            drop(probe);
            if std::net::TcpListener::bind(transport::transfer_address(addr)).is_ok() {
                return addr;
            }
        }
    }

    #[test]
    fn copied_files_cross_as_an_offer_and_are_pasted_once_verified() {
        use crate::clipboard::wire::{self as clip, Wire};
        use crate::net::secure::Link;
        let scratch =
            Scratch(std::env::temp_dir().join(format!("mickey-trigger-{}", std::process::id())));
        let fotos = scratch.0.join("from/fotos");
        std::fs::create_dir_all(&fotos).unwrap();
        std::fs::write(fotos.join("a.txt"), b"hello").unwrap();
        // PC A, where the folder is copied: it serves its offers.
        let listen = free_input_port();
        let a = Arc::new(Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: listen.port(),
            received: scratch.0.join("a-received"),
            trusted: trusting(&scratch.0.join("a-trust"), None),
        });
        let offers = Arc::clone(&a.offers);
        crate::transfer::serve::serve(
            listen,
            Arc::clone(&a.identity),
            |_| true,
            move |mut link| {
                let _ = crate::transfer::send::serve(&mut link, |id| offers.find(id));
            },
        )
        .unwrap();
        // The source asks A's clipboard for an offer, over A's data link.
        let data = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let data_at = data.local_addr().unwrap();
        let a_side = Arc::clone(&a);
        let a_keeper = Keeper::in_memory(Some(Content::Files(vec![fotos.clone()])));
        std::thread::spawn(move || {
            let (stream, _) = data.accept().unwrap();
            let link = Link::respond(stream, &a_side.identity).unwrap();
            let read = || a_keeper.read();
            let _ = crate::clipboard::sync::answer(
                link,
                read,
                || Ok(()),
                |_| {},
                |_, _| {},
                || true,
                Some((&a_side, &a_keeper)),
            );
        });
        let source = Identity::generate().unwrap();
        let mut link =
            Link::initiate(std::net::TcpStream::connect(data_at).unwrap(), &source).unwrap();
        link.send_bulk(&clip::encode(&Wire::AskOffer).unwrap())
            .unwrap();
        let Wire::Offer(offer) = clip::decode(&link.recv_bulk().unwrap()).unwrap() else {
            panic!("copied files should come as an offer");
        };
        assert_eq!(
            (offer.ip, offer.port, offer.key),
            (None, listen.port(), *a.identity.public())
        );
        // PC B fetches it from A, and pastes it once it is all there.
        let b = Arc::new(Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: 1,
            received: scratch.0.join("b-received"),
            trusted: trusting(&scratch.0.join("b-trust"), Some(*a.identity.public())),
        });
        let b_keeper = Keeper::in_memory(Some(Content::Text("copied before".into())));
        b.fetch(offer, listen.ip(), b_keeper.clone());
        let pasted = (0..100)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                match b_keeper.read() {
                    Some(Content::Files(paths)) => Some(paths),
                    _ => None,
                }
            })
            .expect("the files reach the clipboard");
        assert_eq!(pasted.len(), 1);
        assert!(pasted[0].starts_with(scratch.0.join("b-received")));
        assert_eq!(std::fs::read(pasted[0].join("a.txt")).unwrap(), b"hello");
    }
}
