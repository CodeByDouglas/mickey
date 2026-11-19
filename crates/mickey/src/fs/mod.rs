//! Files that cross between PCs: helpers the clipboard and file transfers
//! (PRD 0015) share to check names, open without following links, and mark
//! what arrived.

#[cfg(windows)]
pub mod mark;
pub mod names;
pub mod open;
