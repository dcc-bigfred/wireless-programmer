//! Transport traits implemented by `wp-link` adapters.
//!
//! Defining these in the core crate keeps the dependency graph acyclic:
//! `wp-core` <- `wp-link` <- `wp-drivers`. Drivers depend only on `wp-core`.

use std::io;

/// A minimal HTTP client for device config pages.
///
/// Implementations are expected to be bounded: a deadline, a maximum response
/// body size, and a bounded retry count.
pub trait HttpClient {
    /// Issue a `GET` to `path` (path begins with `/`) and return the body.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] on transport failure or when the response exceeds
    /// the configured bounds.
    fn get(&mut self, path: &str) -> io::Result<Vec<u8>>;
}

/// A bidirectional byte stream for serial devices.
pub trait ByteStream {
    /// Read up to `buf.len()` bytes into `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] on transport failure.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Write all of `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] on transport failure.
    fn write(&mut self, buf: &[u8]) -> io::Result<()>;
}
