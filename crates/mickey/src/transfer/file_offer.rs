//! An offer as the clipboard carries it (PRD 0015, #478): who offers (its
//! key), where it listens, the PC the files first came from, and the offer.
//! A PC that sends its own offer gives its port only; the receiving PC
//! takes the address from the link it came on, as the group map does.

use crate::transfer::offer::Offer;
use crate::transfer::wire::{self, Message};
use std::net::{IpAddr, SocketAddr};

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

/// An offer to test with: a folder holding one file, from `ip`.
#[cfg(test)]
pub fn example(ip: Option<IpAddr>) -> FileOffer {
    use crate::transfer::offer::{Entry, Kind};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_offer_reads_back_with_or_without_an_address() {
        for ip in [
            None,
            Some("192.0.2.7".parse().unwrap()),
            Some("fe80::1".parse().unwrap()),
        ] {
            let sent = example(ip);
            assert_eq!(FileOffer::decode(&sent.encode()).unwrap(), sent);
        }
    }

    #[test]
    fn a_broken_file_offer_is_an_error() {
        let bytes = example(None).encode();
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
            example(None).address(from),
            "192.0.2.9:4242".parse().unwrap()
        );
        let given = example(Some("192.0.2.7".parse().unwrap()));
        assert_eq!(given.address(from), "192.0.2.7:4242".parse().unwrap());
    }
}
