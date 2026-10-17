//! SLMP framing, mailbox state and Linux process execution for sm0sh.
#![cfg(target_os = "linux")]

pub mod command;
pub mod config;
pub mod executor;
pub mod memory;
pub mod protocol;
pub mod server;
