//! Hashing a file while it crosses, so it is read and written once: the
//! sending PC hashes what it reads, the receiving PC what it writes.

use std::io::{self, Read, Write};

/// Reads from `inner` and hashes every byte read.
pub struct HashRead<'a, R> {
    pub inner: R,
    pub hasher: &'a mut blake3::Hasher,
}

impl<R: Read> Read for HashRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let got = self.inner.read(buf)?;
        self.hasher.update(&buf[..got]);
        Ok(got)
    }
}

/// Writes to `inner` and hashes every byte written.
pub struct HashWrite<'a, W> {
    pub inner: W,
    pub hasher: &'a mut blake3::Hasher,
}

impl<W: Write> Write for HashWrite<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let wrote = self.inner.write(buf)?;
        self.hasher.update(&buf[..wrote]);
        Ok(wrote)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ends_hash_what_crosses() {
        let mut read_hash = blake3::Hasher::new();
        let mut write_hash = blake3::Hasher::new();
        let mut reader = HashRead {
            inner: &b"some bytes"[..],
            hasher: &mut read_hash,
        };
        let mut writer = HashWrite {
            inner: Vec::new(),
            hasher: &mut write_hash,
        };
        io::copy(&mut reader, &mut writer).unwrap();
        assert_eq!(writer.inner, b"some bytes");
        assert_eq!(read_hash.finalize(), write_hash.finalize());
        assert_eq!(read_hash.finalize(), blake3::hash(b"some bytes"));
    }
}
