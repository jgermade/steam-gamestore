//! Logging setup.
//!
//! The mini PC runs in console mode, so anything the user is meant to read goes
//! to stderr through `tracing`; per-game launch logs are files under
//! [`Paths::log_dir`](crate::Paths::log_dir) and belong to the launch wrapper.

use tracing_subscriber::EnvFilter;

/// Environment variable holding the log filter, e.g. `gamestore=debug`.
pub const LOG_ENV: &str = "GAMESTORE_LOG";

/// Install the global subscriber. Calling it more than once is harmless.
pub fn init() {
    init_with_default("info");
}

/// Install the global subscriber with `default` as the filter when [`LOG_ENV`] is
/// unset or unparseable.
pub fn init_with_default(default: &str) {
    let filter = EnvFilter::builder()
        .with_env_var(LOG_ENV)
        .try_from_env()
        .unwrap_or_else(|_| EnvFilter::new(default));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}
