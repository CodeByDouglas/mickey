//! The PC where the files were copied: answers another PC's fetches for
//! one offer, a file at a time (PRD 0015).
//!
//! Only files of the offer are ever sent, by their index: the other PC
//! cannot name a path. Each file is opened without following links and
//! checked against the listing; one that changed is skipped, not sent
//! half old and half new.

use crate::fs::open;
use crate::net::secure::Link;
use crate::transfer::hash::HashRead;
use crate::transfer::offer::Listing;
use crate::transfer::wire::{self, Message, Why};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

/// Answers fetches for the offer `offer` (made from `listing`) until the
/// other PC hangs up, which is how a transfer ends.
pub fn serve(link: &mut Link, listing: &Listing, offer: u64) -> io::Result<()> {
    loop {
        let bytes = match link.recv_bulk() {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        };
        let Message::Fetch {
            offer: asked,
            index,
            from,
        } = wire::decode(&bytes).map_err(invalid)?
        else {
            return Err(invalid("expected a fetch".into()));
        };
        if asked != offer {
            return Err(invalid("a fetch for another offer".into()));
        }
        let (path, size, modified) = listing
            .file(index)
            .ok_or_else(|| invalid(format!("no file {index} in the offer")))?;
        if from > size {
            return Err(invalid(format!("a fetch from byte {from} of {size}")));
        }
        send_file(link, index, path, size, modified, from)?;
    }
}

/// Sends the file at `index` from byte `from`, then its hash; or `Skip` if
/// it is no longer as listed, before or after its bytes.
fn send_file(
    link: &mut Link,
    index: u32,
    path: &Path,
    size: u64,
    modified: Option<SystemTime>,
    from: u64,
) -> io::Result<()> {
    let skip = |link: &mut Link, why| send(link, &Message::Skip { index, why });
    let Ok(mut file) = open::without_links(path) else {
        return skip(link, Why::Gone);
    };
    if !unchanged(&file, size, modified) {
        return skip(link, Why::Changed);
    }
    // The hash covers the whole file: the bytes before `from` (which the
    // other PC already has) are read and hashed, not sent.
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut (&mut file).take(from), &mut hasher)?;
    file.seek(SeekFrom::Start(from))?;
    send(
        link,
        &Message::File {
            index,
            size: size - from,
        },
    )?;
    let reader = HashRead {
        inner: &mut file,
        hasher: &mut hasher,
    };
    link.send_stream(reader, size - from)?;
    if !unchanged(&file, size, modified) {
        return skip(link, Why::Changed);
    }
    send(
        link,
        &Message::Hash {
            index,
            hash: *hasher.finalize().as_bytes(),
        },
    )
}

/// Whether the open `file` still has the size and modification time it
/// was listed with.
fn unchanged(file: &std::fs::File, size: u64, modified: Option<SystemTime>) -> bool {
    file.metadata()
        .is_ok_and(|now| now.len() == size && now.modified().ok() == modified)
}

fn send(link: &mut Link, message: &Message) -> io::Result<()> {
    link.send_bulk(&wire::encode(message))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
