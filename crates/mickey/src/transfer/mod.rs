//! Big files and folders from one PC to another (PRD 0015): what is
//! offered, and how it crosses. The clipboard only triggers it.
//!
//! An **offer** lists the copied files and folders without their content;
//! the other PC then fetches each file by the offer's id and the file's
//! index, never by a path, so it can never ask for a file that was not
//! copied.

// Sending and receiving (#474) and the clipboard trigger (#478) use this.
#![cfg_attr(not(test), expect(dead_code, reason = "used from #474 on"))]

pub mod hash;
pub mod offer;
pub mod path;
pub mod receive;
pub mod send;
pub mod serve;
pub mod wire;
