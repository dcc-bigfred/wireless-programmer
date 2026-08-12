//! Library surface for the `wireless-programmer` binary (and integration tests).

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod cli;
pub mod config;
pub mod drivers;
pub mod ipc;
pub mod jobs;
pub mod runtime;
pub mod version;
