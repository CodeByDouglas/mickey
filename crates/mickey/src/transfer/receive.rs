//! The PC the files go to: fetches every file of an offer into a folder
//! (PRD 0015).
//!
//! Each file is written as `name.part` and gets its real name only once
//! its hash matches the one the other PC sent. Nothing is written past
//! the size the offer announced, and every path was checked when the
//! offer was decoded (`wire`), and is checked again here.

use crate::net::secure::Link;
use crate::transfer::hash::HashWrite;
use crate::transfer::offer::{Kind, Offer};
use crate::transfer::path;
use crate::transfer::wire::{self, Message, Why};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// What a transfer brought, for the owner to see.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Where each copied file or folder (the offer's top level) landed.
    pub landed: Vec<PathBuf>,
    pub files: usize,
    pub bytes: u64,
    /// Files not brought, by path in the offer, and why.
    pub skipped: Vec<(String, Why)>,
}

/// Fetches every file of `offer` into the folder `into`. A copied file or
/// folder whose name is taken there gets `name (2)`, never overwriting.
pub fn fetch(link: &mut Link, offer: &Offer, into: &Path) -> io::Result<Summary> {
    let mut summary = Summary::default();
    // The name each top-level entry got here.
    let mut tops: Vec<(String, PathBuf)> = Vec::new();
    for (index, entry) in offer.entries.iter().enumerate() {
        let relative = path::checked(&entry.path).map_err(invalid)?;
        let top = entry.path.split('/').next().unwrap_or_default();
        let here = match tops.iter().find(|(name, _)| name == top) {
            Some((_, here)) => here.clone(),
            None => {
                let is_file = top == entry.path && matches!(entry.kind, Kind::File { .. });
                let here = free_name(&into.join(top), is_file);
                tops.push((top.to_string(), here.clone()));
                here
            }
        };
        // The path below the top level, under the name the top level got
        // (joining an empty path would add a trailing slash).
        let below: PathBuf = relative.components().skip(1).collect();
        let target = if below.as_os_str().is_empty() {
            here
        } else {
            here.join(below)
        };
        match entry.kind {
            Kind::Folder => std::fs::create_dir_all(&target)?,
            Kind::File { size } => {
                let index = u32::try_from(index).map_err(|_| invalid("too many files".into()))?;
                match fetch_file(link, offer.id, index, size, &target)? {
                    Some(why) => summary.skipped.push((entry.path.clone(), why)),
                    None => {
                        summary.files += 1;
                        summary.bytes += size;
                    }
                }
            }
        }
    }
    // Only what is there: a copied file the other PC skipped never landed.
    summary.landed = tops
        .into_iter()
        .map(|(_, here)| here)
        .filter(|here| here.symlink_metadata().is_ok())
        .collect();
    Ok(summary)
}

/// Fetches one file into `target`; `Some(why)` if the other PC skipped it.
fn fetch_file(
    link: &mut Link,
    offer: u64,
    index: u32,
    size: u64,
    target: &Path,
) -> io::Result<Option<Why>> {
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let part = part_of(target);
    send(
        link,
        &Message::Fetch {
            offer,
            index,
            from: 0,
        },
    )?;
    if let Some(why) = receive_into(link, index, size, &part)? {
        return Ok(Some(why));
    }
    #[cfg(windows)]
    crate::fs::mark::as_downloaded(&part);
    std::fs::rename(&part, target)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&part);
        })
        .map(|()| None)
}

/// Receives the file at `index` into `part`, a file it creates, and
/// checks its hash. `part` is removed again unless the file arrived whole.
fn receive_into(link: &mut Link, index: u32, size: u64, part: &Path) -> io::Result<Option<Why>> {
    match recv(link)? {
        Message::Skip { index: got, why } if got == index => return Ok(Some(why)),
        Message::File {
            index: got,
            size: sent,
        } if got == index && sent == size => {}
        _ => return Err(invalid(format!("not the file {index} that was asked for"))),
    }
    // Created new: a `.part` already there (the owner's, or a link) is not
    // this transfer's to write over or to remove.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(part)?;
    let result = write_verified(link, index, size, file);
    if !matches!(result, Ok(None)) {
        let _ = std::fs::remove_file(part);
    }
    result
}

/// Writes the file's bytes into `file` while hashing them, then checks
/// the hash the other PC sends after them.
fn write_verified(
    link: &mut Link,
    index: u32,
    size: u64,
    file: std::fs::File,
) -> io::Result<Option<Why>> {
    let mut hasher = blake3::Hasher::new();
    let mut writer = HashWrite {
        inner: BufWriter::new(file),
        hasher: &mut hasher,
    };
    link.recv_stream(&mut writer, size)?;
    writer.flush()?;
    match recv(link)? {
        Message::Skip { index: got, why } if got == index => Ok(Some(why)),
        Message::Hash { index: got, hash } if got == index => {
            if hasher.finalize() == blake3::Hash::from_bytes(hash) {
                Ok(None)
            } else {
                Err(invalid(format!("file {index} arrived damaged")))
            }
        }
        _ => Err(invalid(format!("no hash after file {index}"))),
    }
}

/// `path`, or `path (2)`, `path (3)`… for the first that is free: nothing
/// there, and for a file no `.part` beside it either. A file keeps its
/// extension last (`foto (2).jpg`); a folder's dot is part of its name.
fn free_name(path: &Path, is_file: bool) -> PathBuf {
    let (stem, extension) = match (is_file, path.extension()) {
        (true, Some(ext)) => (path.file_stem(), Some(ext.to_string_lossy())),
        _ => (path.file_name(), None),
    };
    let stem = stem.unwrap_or_default().to_string_lossy();
    let taken = |candidate: &Path| {
        candidate.symlink_metadata().is_ok()
            || (is_file && part_of(candidate).symlink_metadata().is_ok())
    };
    let mut candidate = path.to_path_buf();
    let mut n = 2;
    while taken(&candidate) {
        let name = match &extension {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        candidate.set_file_name(name);
        n += 1;
    }
    candidate
}

/// `target` with `.part` added to its name.
fn part_of(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

fn send(link: &mut Link, message: &Message) -> io::Result<()> {
    link.send_bulk(&wire::encode(message))
}

fn recv(link: &mut Link) -> io::Result<Message> {
    wire::decode(&link.recv_bulk()?).map_err(invalid)
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::identity::Identity;
    use crate::transfer::offer::{self, Entry, Listing};
    use crate::transfer::send;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// A fresh folder under the system's temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("mickey-receive-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("from")).unwrap();
            std::fs::create_dir_all(dir.join("into")).unwrap();
            Scratch(dir)
        }

        fn file(&self, path: &str, bytes: &[u8]) {
            let path = self.0.join("from").join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        fn dest(&self) -> PathBuf {
            self.0.join("into")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Both ends of a secure link over localhost.
    fn link_pair() -> (Link, Link) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let theirs = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Link::respond(stream, &Identity::generate().unwrap()).unwrap()
        });
        let ours = Link::initiate(
            TcpStream::connect(addr).unwrap(),
            &Identity::generate().unwrap(),
        );
        (ours.unwrap(), theirs.join().unwrap())
    }

    /// Serves `listing` on one end and fetches it into `into` on the other.
    fn transfer(listing: Listing, offer: &Offer, into: &Path) -> io::Result<Summary> {
        let (mut fetching, mut serving) = link_pair();
        let id = offer.id;
        let server = thread::spawn(move || send::serve(&mut serving, &listing, id));
        let summary = fetch(&mut fetching, offer, into);
        drop(fetching);
        server.join().unwrap().unwrap();
        summary
    }

    #[test]
    fn a_tree_arrives_whole_and_verified() {
        let scratch = Scratch::new("tree");
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        scratch.file("fotos/praia.jpg", &big);
        scratch.file("fotos/2026/a.txt", b"hello");
        scratch.file("fotos/empty.txt", b"");
        std::fs::create_dir(scratch.0.join("from/fotos/nothing")).unwrap();
        let listing = offer::list(&[scratch.0.join("from/fotos")]).unwrap();
        let offer = listing.offer();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        let fotos = scratch.dest().join("fotos");
        assert_eq!(std::fs::read(fotos.join("praia.jpg")).unwrap(), big);
        assert_eq!(std::fs::read(fotos.join("2026/a.txt")).unwrap(), b"hello");
        assert!(fotos.join("empty.txt").is_file());
        assert!(fotos.join("nothing").is_dir());
        assert_eq!(summary.landed, [fotos]);
        assert_eq!((summary.files, summary.bytes), (3, big.len() as u64 + 5));
        assert!(summary.skipped.is_empty());
    }

    #[test]
    fn a_name_already_taken_gets_a_number() {
        let scratch = Scratch::new("taken");
        scratch.file("notes.txt", b"new");
        std::fs::write(scratch.dest().join("notes.txt"), b"old").unwrap();
        let listing = offer::list(&[scratch.0.join("from/notes.txt")]).unwrap();
        let offer = listing.offer();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        assert_eq!(
            std::fs::read(scratch.dest().join("notes.txt")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(scratch.dest().join("notes (2).txt")).unwrap(),
            b"new"
        );
        assert_eq!(summary.landed, [scratch.dest().join("notes (2).txt")]);
    }

    #[test]
    fn the_owners_part_file_and_dotted_folders_are_left_alone() {
        let scratch = Scratch::new("owners");
        scratch.file("notes.txt", b"new");
        scratch.file("my.project/a.txt", b"a");
        std::fs::write(scratch.dest().join("notes.txt.part"), b"mine").unwrap();
        std::fs::create_dir(scratch.dest().join("my.project")).unwrap();
        let roots = [
            scratch.0.join("from/notes.txt"),
            scratch.0.join("from/my.project"),
        ];
        let listing = offer::list(&roots).unwrap();
        let offer = listing.offer();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        let dest = scratch.dest();
        assert_eq!(std::fs::read(dest.join("notes.txt.part")).unwrap(), b"mine");
        assert_eq!(std::fs::read(dest.join("notes (2).txt")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(dest.join("my.project (2)/a.txt")).unwrap(),
            b"a"
        );
        assert_eq!(
            summary.landed,
            [dest.join("notes (2).txt"), dest.join("my.project (2)")]
        );
    }

    #[test]
    fn a_skipped_file_did_not_land() {
        let scratch = Scratch::new("skipped");
        scratch.file("gone.txt", b"x");
        let listing = offer::list(&[scratch.0.join("from/gone.txt")]).unwrap();
        let offer = listing.offer();
        std::fs::remove_file(scratch.0.join("from/gone.txt")).unwrap();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        assert!(summary.landed.is_empty());
        assert_eq!(summary.skipped, [("gone.txt".to_string(), Why::Gone)]);
    }

    #[test]
    fn a_file_changed_or_deleted_since_the_offer_is_skipped() {
        let scratch = Scratch::new("changed");
        scratch.file("docs/changes.txt", b"before");
        scratch.file("docs/goes.txt", b"here");
        scratch.file("docs/stays.txt", b"same");
        let listing = offer::list(&[scratch.0.join("from/docs")]).unwrap();
        let offer = listing.offer();
        scratch.file("docs/changes.txt", b"after, longer");
        std::fs::remove_file(scratch.0.join("from/docs/goes.txt")).unwrap();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        let docs = scratch.dest().join("docs");
        assert_eq!(
            summary.skipped,
            [
                ("docs/changes.txt".to_string(), Why::Changed),
                ("docs/goes.txt".to_string(), Why::Gone)
            ]
        );
        assert!(!docs.join("changes.txt").exists() && !docs.join("changes.txt.part").exists());
        assert_eq!(std::fs::read(docs.join("stays.txt")).unwrap(), b"same");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_swapped_for_a_link_after_the_offer_is_not_followed() {
        let scratch = Scratch::new("swapped");
        scratch.file("secret", b"key");
        scratch.file("shared/a.txt", b"ok");
        let listing = offer::list(&[scratch.0.join("from/shared")]).unwrap();
        let offer = listing.offer();
        let a = scratch.0.join("from/shared/a.txt");
        std::fs::remove_file(&a).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("from/secret"), &a).unwrap();
        let summary = transfer(listing, &offer, &scratch.dest()).unwrap();
        assert_eq!(summary.skipped, [("shared/a.txt".to_string(), Why::Gone)]);
        assert!(!scratch.dest().join("shared/a.txt").exists());
    }

    #[test]
    fn a_sender_that_lies_about_the_size_leaves_nothing() {
        let scratch = Scratch::new("lies");
        let (mut fetching, mut serving) = link_pair();
        let offer = Offer {
            id: 1,
            entries: vec![Entry {
                path: "x.bin".into(),
                kind: Kind::File { size: 4 },
            }],
        };
        let liar = thread::spawn(move || {
            serving.recv_bulk().unwrap();
            let file = Message::File {
                index: 0,
                size: 1 << 30,
            };
            serving.send_bulk(&wire::encode(&file)).unwrap();
        });
        assert!(fetch(&mut fetching, &offer, &scratch.dest()).is_err());
        liar.join().unwrap();
        assert_eq!(std::fs::read_dir(scratch.dest()).unwrap().count(), 0);
    }
}
