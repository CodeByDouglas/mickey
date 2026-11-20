//! Helpers shared by tests: a scratch folder, two linked PCs, a free port.

use crate::net::identity::Identity;
use crate::net::secure::Link;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh folder under the system's temp folder, removed on drop. Each
/// one has its own name, so tests running at once never share one.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mickey-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// Writes `bytes` at `path` inside, making its folders.
    pub fn file(&self, path: impl AsRef<Path>, bytes: &[u8]) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two ends of a secure link between two new identities, as two PCs.
pub fn link_pair() -> (Link, Link) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let theirs = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        Link::respond(stream, &Identity::generate().unwrap()).unwrap()
    });
    let ours = Link::initiate(
        TcpStream::connect(addr).unwrap(),
        &Identity::generate().unwrap(),
    );
    (ours.unwrap(), theirs.join().unwrap())
}

/// An input port whose transfer port (+2) is free too.
pub fn free_input_port() -> SocketAddr {
    loop {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        if TcpListener::bind(crate::net::transport::transfer_address(addr)).is_ok() {
            return addr;
        }
    }
}
