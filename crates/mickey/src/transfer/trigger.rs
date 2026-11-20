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
use crate::net::secure::Link;
use crate::net::transport;
use crate::net::trust;
use crate::threads::LockAnyway;
use crate::transfer::offer::{self, Kind, Offer};
use crate::transfer::offers::Offers;
use crate::transfer::receive::{self, Fetched};
use crate::transfer::show::{Ended, Watcher};
use crate::transfer::wire::{self, Message};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Above this, a transfer is asked about first: Bring (to
/// `Downloads/Mickey`) or Ignore.
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
    /// The PC these files came from, if they arrived on the offering PC
    /// by a transfer: that PC has them already, and never fetches them
    /// back (#424).
    pub origin: Option<[u8; 32]>,
    pub offer: Offer,
}

impl FileOffer {
    /// `port: u16`, the IP (`0`, or `4` and 4 bytes, or `6` and 16 bytes),
    /// `key: 32 bytes`, the origin (`0`, or `1` and 32 bytes), then the
    /// offer as `transfer::wire` encodes it.
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
        match self.origin {
            None => out.push(0),
            Some(origin) => {
                out.push(1);
                out.extend_from_slice(&origin);
            }
        }
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
        let (origin, rest) = match rest.split_first().ok_or_else(short)? {
            (0, rest) => (None, rest),
            (1, rest) => {
                let (origin, rest) = rest.split_first_chunk::<32>().ok_or_else(short)?;
                (Some(*origin), rest)
            }
            (kind, _) => return Err(format!("unknown origin kind {kind} in a file offer")),
        };
        let Message::Offer(offer) = wire::decode(rest)? else {
            return Err("a file offer without its offer".into());
        };
        Ok(FileOffer {
            ip,
            port: u16::from_be_bytes(*port),
            key: *key,
            origin,
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
    /// Each file or folder transfers brought here, and the PC it came from:
    /// they are never sent back there (#424). As long as Mickey runs.
    pub came_from: std::sync::Mutex<Vec<(PathBuf, [u8; 32])>>,
    /// One lane per PC files are fetched from: a transfer from a PC waits
    /// for the one before it from that PC to end, instead of both sharing
    /// the network and the disk.
    pub lanes: Lanes,
    /// What became of each offer fetched here, while Mickey runs: one
    /// offered again (the keyboard crossed again) is fetched once, and one
    /// that broke goes on from where it stopped (#475, #502).
    pub fetches: Fetches,
}

/// Each offer fetched here, by the offering PC's key and the offer's id.
#[derive(Default)]
pub struct Fetches(std::sync::Mutex<std::collections::HashMap<([u8; 32], u64), Fetch>>);

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
enum Begin {
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
    fn begin(&self, key: ([u8; 32], u64)) -> Begin {
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

    /// Records how the fetch of `key` ended: what a broken one had done.
    fn end(&self, key: ([u8; 32], u64), ended: Result<Vec<PathBuf>, (PathBuf, Box<Fetched>)>) {
        let fetch = match ended {
            Ok(landed) => Fetch::Done(landed),
            Err((into, fetched)) => Fetch::Broken { into, fetched },
        };
        self.0.lock_anyway().insert(key, fetch);
    }

    /// Forgets `key`: the owner said Ignore, or it could not start.
    fn forget(&self, key: ([u8; 32], u64)) {
        self.0.lock_anyway().remove(&key);
    }
}

/// A lock per PC, made on first use (PRD 0015's queue).
#[derive(Default)]
pub struct Lanes(std::sync::Mutex<std::collections::HashMap<[u8; 32], Arc<std::sync::Mutex<()>>>>);

impl Lanes {
    /// The lane for the PC with `key`.
    fn of(&self, key: &[u8; 32]) -> Arc<std::sync::Mutex<()>> {
        Arc::clone(self.0.lock_anyway().entry(*key).or_default())
    }
}

impl Transfers {
    /// An offer of the files at `paths`, handed out by this PC, in place of
    /// the one before. The IP is left out: the receiving PC knows it.
    pub fn offer(&self, paths: &[PathBuf]) -> Result<FileOffer, String> {
        let listing = offer::list(paths)?;
        // Nothing left to offer (the copied files were deleted or moved):
        // the clipboard then carries what it can, as before.
        if listing.entries.is_empty() {
            return Err("none of the copied files is there any more".into());
        }
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
            origin: self.origin_of(paths),
            offer: self.offers.hand_out(listing)?,
        })
    }

    /// The PC `paths` came from, if they all arrived here from one PC.
    fn origin_of(&self, paths: &[PathBuf]) -> Option<[u8; 32]> {
        let came_from = self.came_from.lock_anyway();
        let origin_of = |path: &PathBuf| {
            came_from
                .iter()
                .find(|(folder, _)| path.starts_with(folder))
                .map(|(_, key)| *key)
        };
        let first = origin_of(paths.first()?)?;
        paths
            .iter()
            .all(|path| origin_of(path) == Some(first))
            .then_some(first)
    }

    /// Fetches `offer` (which came from the PC at `from`) on a thread of its
    /// own, and once every file is there and verified puts them on the
    /// clipboard, unless something else was copied meanwhile.
    pub fn fetch(self: &Arc<Self>, offer: FileOffer, from: IpAddr, keeper: Keeper) {
        // Files that came from here: this PC has them already, and its
        // clipboard keeps what it holds.
        if offer.origin == Some(*self.identity.public()) {
            return;
        }
        let this = Arc::clone(self);
        crate::threads::spawn("transfer fetch", move || {
            let key = (offer.key, offer.offer.id);
            let who = crate::target::data::peer_name(offer.ip.unwrap_or(from));
            let (into, mut fetched) = match this.fetches.begin(key) {
                Begin::Skip => return,
                Begin::Here(landed) => return keeper.write(Content::Files(landed)),
                Begin::Resume(into, fetched) => (into, *fetched),
                Begin::Start => match this.landing(&offer, &who) {
                    Ok(Some(into)) => {
                        let mut fetched = Fetched::default();
                        fetched.mark = crate::fs::mark::Mark::configured(&who);
                        (into, fetched)
                    }
                    Ok(None) => return this.fetches.forget(key),
                    Err(err) => {
                        this.fetches.forget(key);
                        return eprintln!("mickey: files not fetched: {err}");
                    }
                },
            };
            let before = keeper.read();
            let result = this.fetch_shown(&offer, from, &who, &into, &mut fetched);
            match &result {
                // Everything skipped by the other PC: nothing to paste.
                Ok(landed) if landed.is_empty() => {}
                Ok(landed) if keeper.read() == before => {
                    keeper.write(Content::Files(landed.clone()))
                }
                Ok(_) => println!(
                    "mickey: files arrived in {}, not pasted: something else was copied meanwhile",
                    into.display()
                ),
                Err(err) => eprintln!(
                    "mickey: files not fetched: {err}; offered again, they go on from where they stopped"
                ),
            }
            let broken = |_| (into, Box::new(fetched));
            this.fetches.end(key, result.map_err(broken));
        });
    }

    /// Fetches `offer` into `into`, going on from what `fetched` says was
    /// done before, and showing how it goes once it takes more than a
    /// moment (`show`); where its top-level files and folders landed.
    fn fetch_shown(
        &self,
        offer: &FileOffer,
        from: IpAddr,
        who: &str,
        into: &Path,
        fetched: &mut Fetched,
    ) -> Result<Vec<PathBuf>, String> {
        // Waits its turn behind another transfer from the same PC.
        let lane = self.lanes.of(&offer.key);
        let _turn = lane.lock_anyway();
        let mut link = self
            .connect(offer, from)
            .inspect_err(|why| crate::transfer::show::failed(who, why))?;
        let shown = link
            .try_clone()
            .ok()
            .map(|stop| Watcher::start(who.to_string(), fetched.progress.clone(), stop));
        let landed = self.receive(offer, &mut link, into, fetched);
        if let Some(shown) = shown {
            shown.end(match &landed {
                Ok(_) => Ended::Done {
                    files: fetched.summary.files,
                    bytes: fetched.summary.bytes,
                    folder: into.display().to_string(),
                },
                Err(err) => Ended::Failed(err.clone()),
            });
        }
        landed
    }

    /// Where `offer`'s files go: a fresh folder among the received ones; or,
    /// over `ASK_OVER`, `Downloads/Mickey` if the owner says Bring, and
    /// `None` if they say Ignore.
    fn landing(&self, offer: &FileOffer, who: &str) -> Result<Option<PathBuf>, String> {
        let size = total(&offer.offer);
        if size <= ASK_OVER {
            return crate::clipboard::received::new_folder(&self.received).map(Some);
        }
        if !ask_to_bring(offer, who, size) {
            println!(
                "mickey: {:.1} GB of files offered, not brought",
                size as f64 / 1e9
            );
            return Ok(None);
        }
        let folder = crate::fs::downloads::mickey_folder()?;
        // Only Mickey's own leftovers go, never what the owner brought.
        crate::transfer::cleanup::remove_abandoned(&folder, std::time::SystemTime::now());
        Ok(Some(folder))
    }

    /// A transfer link to the PC that made `offer`: only a paired one, and
    /// only if it answers with that PC's key.
    fn connect(&self, offer: &FileOffer, from: IpAddr) -> Result<Link, String> {
        if !self.trusted.lock_anyway().is_trusted(&offer.key) {
            return Err("files offered by a PC that is not paired".into());
        }
        let at = offer.address(from);
        let link = transport::connect_transfer(at, &self.identity).map_err(|err| {
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
        Ok(link)
    }

    /// Receives `offer` on `link` into the folder `into`; where its
    /// top-level files and folders landed.
    fn receive(
        &self,
        offer: &FileOffer,
        link: &mut Link,
        into: &Path,
        fetched: &mut Fetched,
    ) -> Result<Vec<PathBuf>, String> {
        receive::fetch(link, &offer.offer, into, fetched).map_err(|err| err.to_string())?;
        let summary = &fetched.summary;
        // Never sent back there (#424).
        let landed = summary.landed.iter().map(|path| (path.clone(), offer.key));
        self.came_from.lock_anyway().extend(landed);
        println!(
            "mickey: {} files, {:.1} MB, verified, in {}",
            summary.files,
            summary.bytes as f64 / 1e6,
            into.display()
        );
        Ok(summary.landed.clone())
    }
}

/// Asks the owner whether to bring `size` bytes of files offered by
/// `who`: Bring or Ignore, with what they are (`mickey-ui bring`).
fn ask_to_bring(offer: &FileOffer, who: &str, size: u64) -> bool {
    let size = format!("{:.1} GB", size as f64 / 1e9);
    let answer = crate::window::ui_command(&["bring", who, &size, &top_level(&offer.offer)])
        .and_then(|mut ui| ui.status());
    matches!(answer.map(|status| status.code()), Ok(Some(0)))
}

/// What `offer` holds, one line per copied file or folder (a folder with
/// how many files it holds), the first few only.
fn top_level(offer: &Offer) -> String {
    const SHOWN: usize = 4;
    let tops: Vec<&str> = offer
        .entries
        .iter()
        .filter(|entry| !entry.path.contains('/'))
        .map(|entry| entry.path.as_str())
        .collect();
    let mut lines: Vec<String> = tops
        .iter()
        .take(SHOWN)
        .map(|top| {
            let inside = offer
                .entries
                .iter()
                .filter(|entry| {
                    matches!(entry.kind, Kind::File { .. })
                        && entry
                            .path
                            .strip_prefix(top)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .count();
            if inside > 0 {
                format!("{top}/  ({inside})")
            } else {
                top.to_string()
            }
        })
        .collect();
    if tops.len() > SHOWN {
        lines.push(format!("+{}", tops.len() - SHOWN));
    }
    lines.join("\n")
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
            origin: None,
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
    fn the_bring_question_lists_the_copied_things_first_few_only() {
        let entry = |path: &str, kind| crate::transfer::offer::Entry {
            path: path.into(),
            kind,
        };
        let file = Kind::File { size: 1 };
        let mut entries = vec![
            entry("fotos", Kind::Folder),
            entry("fotos/a.jpg", file),
            entry("fotos/2026", Kind::Folder),
            entry("fotos/2026/b.jpg", file),
            entry("fotos-velhas.zip", file),
        ];
        entries.extend((0..5).map(|n| entry(&format!("{n}.txt"), file)));
        let text = top_level(&Offer { id: 1, entries });
        assert_eq!(text, "fotos/  (2)\nfotos-velhas.zip\n0.txt\n1.txt\n+3");
    }

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
        fetches.end(key, Err(("/somewhere".into(), Box::new(fetched))));
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
        fetches.end(key, Ok(vec![here.clone()]));
        assert!(matches!(fetches.begin(key), Begin::Here(landed) if landed == [here]));
        fetches.end(key, Ok(vec![PathBuf::from("/no/such/file/for/mickey")]));
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
    fn copied_files_that_are_gone_are_not_offered() {
        let scratch =
            Scratch(std::env::temp_dir().join(format!("mickey-gone-{}", std::process::id())));
        let transfers = Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: 1,
            received: scratch.0.join("received"),
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
            trusted: trusting(&scratch.0.join("trust"), None),
        };
        // Copied, then deleted before the keyboard crossed.
        assert!(
            transfers
                .offer(&[scratch.0.join("deleted folder")])
                .is_err()
        );
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
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
            trusted: trusting(&scratch.0.join("trust"), None),
        };
        // Refused before any connection: nothing listens at that address.
        let Err(err) = b.connect(&file_offer(None), "127.0.0.1".parse().unwrap()) else {
            panic!("a PC not paired must be refused");
        };
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
    fn copying_again_while_files_come_cuts_nothing_short() {
        let scratch =
            Scratch(std::env::temp_dir().join(format!("mickey-turns-{}", std::process::id())));
        let from = scratch.0.join("from");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("x.bin"), vec![1u8; 3_000_000]).unwrap();
        std::fs::write(from.join("y.txt"), b"y").unwrap();
        // PC A serves what it hands out.
        let listen = free_input_port();
        let a = Arc::new(Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: listen.port(),
            received: scratch.0.join("a-received"),
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
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
        // X is copied and crosses; then Y, before X has arrived.
        let x = a.offer(&[from.join("x.bin")]).unwrap();
        let y = a.offer(&[from.join("y.txt")]).unwrap();
        let b = Arc::new(Transfers {
            offers: Arc::new(Offers::default()),
            identity: Arc::new(Identity::generate().unwrap()),
            port: 1,
            received: scratch.0.join("b-received"),
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
            trusted: trusting(&scratch.0.join("b-trust"), Some(*a.identity.public())),
        });
        let fetch = |offer: FileOffer, name: &'static str| {
            let (b, into) = (Arc::clone(&b), scratch.0.join(name));
            std::fs::create_dir_all(&into).unwrap();
            std::thread::spawn(move || {
                b.fetch_shown(&offer, listen.ip(), "A", &into, &mut Fetched::default())
            })
        };
        let (first, second) = (fetch(x, "x"), fetch(y, "y"));
        assert!(first.join().unwrap().is_ok(), "X arrives whole");
        assert!(second.join().unwrap().is_ok(), "and Y after it");
        assert_eq!(
            std::fs::read(scratch.0.join("x/x.bin")).unwrap().len(),
            3_000_000
        );
        assert_eq!(std::fs::read(scratch.0.join("y/y.txt")).unwrap(), b"y");
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
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
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
            came_from: Default::default(),
            lanes: Default::default(),
            fetches: Default::default(),
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
        // Copied again on B, the folder says it came from A, and A never
        // fetches it back (#424): its clipboard keeps what it holds.
        let back = b.offer(&pasted).unwrap();
        assert_eq!(back.origin, Some(*a.identity.public()));
        let a_clipboard = Keeper::in_memory(Some(Content::Text("A's own".into())));
        a.fetch(back, listen.ip(), a_clipboard.clone());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(a_clipboard.read(), Some(Content::Text("A's own".into())));
        assert!(!scratch.0.join("a-received").exists(), "nothing fetched");
    }
}
