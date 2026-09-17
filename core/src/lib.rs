//! Everything that does not depend on the host platform.
//!
//! It owns the shared error type, logging setup, configuration, the HTTP client
//! behind a trait, the GOG OAuth2 flow, and the [`Platform`] trait that the
//! `platform-*` crates implement. The catalog and the downloader land here too.

pub mod atomic;
pub mod auth;
pub mod catalog;
pub mod config;
pub mod error;
pub mod http;
pub mod logging;
pub mod platform;
pub mod shortcuts;
pub mod steam;
pub mod tokens;
pub mod vdf;

pub use config::{Config, Paths};
pub use error::{Error, Result};
pub use platform::Platform;

/// Name used for configuration and data directories, and for the CLI binary.
pub const APP_NAME: &str = "gamestore";
