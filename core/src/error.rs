//! The error type shared by every crate in the workspace.

use std::path::PathBuf;

/// Result with [`Error`] as its error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Anything that can go wrong while running gamestore.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation} failed: {source}")]
    Io {
        operation: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not determine the {0} directory for this platform")]
    MissingDirectory(&'static str),

    #[error("could not parse the configuration file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not serialize the configuration: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error(
        "no GOG client credentials configured; put them under [auth] in {path} \
or set GOG_CLIENT_ID and GOG_CLIENT_SECRET"
    )]
    MissingCredentials { path: PathBuf },

    #[error("request to {url} failed: {reason}")]
    Http { url: String, reason: String },

    #[error("GOG login failed: {0}")]
    Auth(String),

    #[error("{0}")]
    TokenStore(String),

    #[error("{0}")]
    Vdf(String),

    #[error("the GOG library could not be read: {0}")]
    Catalog(String),

    #[error("{0}")]
    Registry(String),

    #[error("not logged in to GOG; run `gamestore login`")]
    NotLoggedIn,

    #[error("no Steam installation found; set STEAM_ROOT to point at one")]
    SteamNotFound,

    #[error("{0} is not supported on this platform")]
    Unsupported(&'static str),
}

impl Error {
    /// An error about a path that is not an [`std::io::Error`].
    pub fn io_path(path: &std::path::Path, problem: &str) -> Self {
        Self::Io {
            operation: format!("{} {problem}", path.display()),
            source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
        }
    }

    /// Attach a human-readable operation to an [`std::io::Error`].
    pub fn io(operation: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            source,
        }
    }
}
