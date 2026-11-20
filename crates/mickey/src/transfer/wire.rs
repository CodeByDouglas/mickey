//! What crosses for a transfer (PRD 0015): one bulk payload per message, a
//! kind byte and its body. Numbers are big-endian.
//!
//! ```text
//! 0  Offer  id: u64, count: u32, then per entry
//!           kind: u8 (0 folder, 1 file), path length: u16, path (UTF-8),
//!           and for a file size: u64
//! 1  Fetch  offer id: u64, file index: u32, from byte: u64
//! 2  File   file index: u32, size: u64; then the file's bytes as a stream
//! 3  Hash   file index: u32, BLAKE3 of the whole file: 32 bytes
//! 4  Skip   file index: u32, why: u8 (0 changed, 1 gone)
//! 5  StillHere  (no body): sent every few seconds while a side reads a
//!           big file without sending anything, so the other does not
//!           give up
//! ```
//!
//! For each file the fetching PC sends `Fetch`; the other answers `File`,
//! the bytes and `Hash`, or `Skip` if the file is no longer as offered (it
//! may also send `Skip` after the bytes, if the file changed meanwhile).
//!
//! Everything decoded comes from the other PC: every path is checked, the
//! counts and sizes are capped, and garbage is an error, never a panic.

use crate::net::invalid;
use crate::net::secure::Link;
use crate::transfer::offer::{Entry, Kind, MAX_ENTRIES, Offer};
use crate::transfer::path;
use std::collections::HashSet;
use std::io;

const OFFER: u8 = 0;
const FETCH: u8 = 1;
const FILE: u8 = 2;
const HASH: u8 = 3;
const SKIP: u8 = 4;
const STILL_HERE: u8 = 5;
const FOLDER_ENTRY: u8 = 0;
const FILE_ENTRY: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Offer(Offer),
    /// Send the file at `index` of offer `offer`, starting at byte `from`
    /// (0 unless resuming).
    Fetch {
        offer: u64,
        index: u32,
        from: u64,
    },
    /// The file at `index` follows: `size` bytes from where it was asked.
    File {
        index: u32,
        size: u64,
    },
    /// The BLAKE3 hash of the whole file at `index`, after its bytes.
    Hash {
        index: u32,
        hash: [u8; 32],
    },
    /// The file at `index` is not sent, or what was sent must be thrown
    /// away.
    Skip {
        index: u32,
        why: Why,
    },
    /// Nothing to say yet, but still here (`transport::TRANSFER_SILENT`).
    StillHere,
}

/// Why a file is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Its size or modification time is no longer what the offer says.
    Changed,
    /// Deleted, moved, or no longer readable.
    Gone,
}

/// Sends `message` on `link`.
pub fn send(link: &mut Link, message: &Message) -> io::Result<()> {
    link.send_bulk(&encode(message))
}

/// The next message on `link`, past any `StillHere` (which only says the
/// other PC is still there).
pub fn recv(link: &mut Link) -> io::Result<Message> {
    loop {
        match decode(&link.recv_bulk()?).map_err(invalid)? {
            Message::StillHere => continue,
            message => return Ok(message),
        }
    }
}

pub fn encode(message: &Message) -> Vec<u8> {
    match message {
        Message::Offer(offer) => {
            let mut out = vec![OFFER];
            out.extend_from_slice(&offer.id.to_be_bytes());
            out.extend_from_slice(&(offer.entries.len() as u32).to_be_bytes());
            for entry in &offer.entries {
                let kind = match entry.kind {
                    Kind::Folder => FOLDER_ENTRY,
                    Kind::File { .. } => FILE_ENTRY,
                };
                out.push(kind);
                out.extend_from_slice(&(entry.path.len() as u16).to_be_bytes());
                out.extend_from_slice(entry.path.as_bytes());
                if let Kind::File { size } = entry.kind {
                    out.extend_from_slice(&size.to_be_bytes());
                }
            }
            out
        }
        Message::Fetch { offer, index, from } => [
            &[FETCH][..],
            &offer.to_be_bytes(),
            &index.to_be_bytes(),
            &from.to_be_bytes(),
        ]
        .concat(),
        Message::File { index, size } => {
            [&[FILE][..], &index.to_be_bytes(), &size.to_be_bytes()].concat()
        }
        Message::Hash { index, hash } => [&[HASH][..], &index.to_be_bytes(), hash].concat(),
        Message::Skip { index, why } => {
            let why = match why {
                Why::Changed => 0,
                Why::Gone => 1,
            };
            [&[SKIP][..], &index.to_be_bytes(), &[why]].concat()
        }
        Message::StillHere => vec![STILL_HERE],
    }
}

pub fn decode(bytes: &[u8]) -> Result<Message, String> {
    let mut body = Body(bytes);
    let message = match body.u8()? {
        OFFER => Message::Offer(offer(&mut body)?),
        FETCH => Message::Fetch {
            offer: body.u64()?,
            index: body.u32()?,
            from: body.u64()?,
        },
        FILE => Message::File {
            index: body.u32()?,
            size: body.u64()?,
        },
        HASH => Message::Hash {
            index: body.u32()?,
            hash: body.array()?,
        },
        SKIP => Message::Skip {
            index: body.u32()?,
            why: match body.u8()? {
                0 => Why::Changed,
                1 => Why::Gone,
                why => return Err(format!("unknown reason {why} to skip a file")),
            },
        },
        STILL_HERE => Message::StillHere,
        kind => return Err(format!("unknown transfer message {kind}")),
    };
    if !body.0.is_empty() {
        return Err("bytes after the end of a transfer message".into());
    }
    Ok(message)
}

fn offer(body: &mut Body) -> Result<Offer, String> {
    let id = body.u64()?;
    let count = body.u32()? as usize;
    if count > MAX_ENTRIES {
        return Err(format!(
            "an offer of {count} entries, more than {MAX_ENTRIES}"
        ));
    }
    let mut entries = Vec::new();
    // Two paths that only differ in case are the same file on Windows: one
    // would overwrite the other.
    let mut seen = HashSet::new();
    for _ in 0..count {
        let kind = body.u8()?;
        let length = usize::from(body.u16()?);
        let path = std::str::from_utf8(body.take(length)?)
            .map_err(|_| "a path that is not UTF-8")?
            .to_string();
        path::checked(&path)?;
        if !seen.insert(path.to_lowercase()) {
            return Err(format!("the path {path:?} is in the offer twice"));
        }
        let kind = match kind {
            FOLDER_ENTRY => Kind::Folder,
            FILE_ENTRY => Kind::File { size: body.u64()? },
            _ => return Err(format!("unknown entry kind {kind}")),
        };
        entries.push(Entry { path, kind });
    }
    Ok(Offer { id, entries })
}

/// The bytes of a message not read yet.
struct Body<'a>(&'a [u8]);

impl<'a> Body<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let (head, rest) = self
            .0
            .split_at_checked(len)
            .ok_or("a transfer message that ends too soon")?;
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let bytes = self.take(N)?;
        Ok(bytes.try_into().expect("take returned exactly N bytes"))
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        self.array().map(u16::from_be_bytes)
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.array().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Result<u64, String> {
        self.array().map(u64::from_be_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(entries: &[(&str, Kind)]) -> Offer {
        Offer {
            id: 0xC0FFEE,
            entries: entries
                .iter()
                .map(|&(path, kind)| Entry {
                    path: path.into(),
                    kind,
                })
                .collect(),
        }
    }

    #[test]
    fn every_message_reads_back() {
        let messages = [
            Message::Offer(offer(&[
                ("fotos", Kind::Folder),
                ("fotos/praia.jpg", Kind::File { size: 5 << 30 }),
            ])),
            Message::Offer(offer(&[])),
            Message::Fetch {
                offer: 7,
                index: 3,
                from: 1 << 40,
            },
            Message::File {
                index: 3,
                size: u64::MAX,
            },
            Message::Hash {
                index: 3,
                hash: [9; 32],
            },
            Message::Skip {
                index: 4,
                why: Why::Changed,
            },
            Message::Skip {
                index: 5,
                why: Why::Gone,
            },
            Message::StillHere,
        ];
        for message in messages {
            assert_eq!(decode(&encode(&message)).unwrap(), message);
        }
    }

    #[test]
    fn hostile_offers_are_refused() {
        for path in ["../x", "/etc/passwd", "a/../../b", "a\\b", "CON"] {
            let bytes = encode(&Message::Offer(offer(&[(path, Kind::Folder)])));
            assert!(decode(&bytes).is_err(), "{path:?}");
        }
        let twins = offer(&[("Foto.jpg", Kind::Folder), ("foto.jpg", Kind::Folder)]);
        assert!(decode(&encode(&Message::Offer(twins))).is_err());
        let mut too_many = vec![OFFER];
        too_many.extend_from_slice(&0u64.to_be_bytes());
        too_many.extend_from_slice(&(MAX_ENTRIES as u32 + 1).to_be_bytes());
        assert!(decode(&too_many).is_err());
    }

    #[test]
    fn short_long_or_unknown_bytes_are_errors() {
        let fetch = encode(&Message::Fetch {
            offer: 1,
            index: 2,
            from: 3,
        });
        for cut in 0..fetch.len() {
            assert!(decode(&fetch[..cut]).is_err(), "cut at {cut}");
        }
        assert!(decode(&[fetch.clone(), vec![0]].concat()).is_err());
        assert!(decode(&[9]).is_err());
        let mut bad_kind = encode(&Message::Offer(offer(&[("a", Kind::Folder)])));
        bad_kind[13] = 7;
        assert!(decode(&bad_kind).is_err());
    }
}
