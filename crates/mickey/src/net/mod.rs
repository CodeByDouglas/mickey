//! Talking to the other PCs: finding them, the secure link and what goes
//! over it, who is trusted, and the group they form.

pub mod discovery;
pub mod group;
pub mod identity;
pub mod latency;
pub mod protocol;
pub mod secure;
pub mod transport;
pub mod trust;

/// An error for bytes from another PC that make no sense: garbage, or a
/// message where another was expected.
pub fn invalid(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}
