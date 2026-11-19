//! Files from another PC: helpers every feature that writes them shares
//! (the clipboard today, file transfers next, PRD 0015).

#[cfg(windows)]
pub mod mark;
pub mod names;
