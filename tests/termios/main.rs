//! What the pty is left in when the session is over.
//!
//! Raw mode belongs to the terminal device rather than to the process: a session
//! that takes it and does not give it back leaves the user's shell unusable. The
//! test process cannot read that descriptor — termlens owns it — so the shell
//! inside the pty reports it with `stty -g`, once before the session and once
//! after it. The two strings are the assertion.
//!
//! They are compared with each other rather than against a stored value: what
//! the pty starts as is the harness's business, and what it must be afterwards
//! is what it was before.

#![cfg(target_os = "linux")]

#[path = "../common/mod.rs"]
mod common;

mod after_foot_exit;
mod error_during_stream;
mod modes;
