//! Everything that does not depend on the host platform.
//!
//! Later phases add authentication, the catalog and the downloader here; for now
//! this crate owns the shared error type, logging setup, configuration and the
//! [`Platform`] trait that the `platform-*` crates implement.

pub mod config;
pub mod error;
pub mod logging;
pub mod platform;

pub use config::{Config, Paths};
pub use error::{Error, Result};
pub use platform::Platform;

/// Name used for configuration and data directories, and for the CLI binary.
pub const APP_NAME: &str = "gamestore";
