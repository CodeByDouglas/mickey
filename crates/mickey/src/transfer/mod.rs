//! Big files and folders from one PC to another (PRD 0015): what is
//! offered, and how it crosses. The clipboard only triggers it.
//!
//! An **offer** lists the copied files and folders without their content;
//! the other PC then fetches each file by the offer's id and the file's
//! index, never by a path, so it can never ask for a file that was not
//! copied.

pub mod ahead;
pub mod bring;
pub mod cleanup;
pub mod fetches;
pub mod file_offer;
pub mod hash;
pub mod listen;
pub mod offer;
pub mod offers;
pub mod path;
pub mod progress;
pub mod receive;
pub mod room;
pub mod send;
pub mod show;
pub mod trigger;
pub mod wire;
