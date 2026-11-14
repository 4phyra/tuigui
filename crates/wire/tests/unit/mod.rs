//! Pieces every test of this crate shares.
//!
//! One module rather than a copy per file: the transports are several and the claims about them
//! are one set, so the rules live here and each transport's file only says how to reach it.

#[path = "pending_read.rs"]
pub mod pending_read;
