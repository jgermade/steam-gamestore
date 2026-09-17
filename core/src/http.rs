//! The HTTP client, behind a trait.
//!
//! Every request gamestore makes goes through [`HttpClient`], so the auth flow and
//! (later) the catalog and the downloader are testable without a network, and so
//! the transport can be swapped without touching them.

use std::time::Duration;

use crate::{Error, Result};

/// What a request came back with. Non-2xx statuses are returned, not raised: the
/// GOG endpoints put their error details in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Minimal HTTP surface: enough for OAuth2 and the GOG JSON APIs.
pub trait HttpClient {
    /// GET `url` and read the whole body.
    fn get(&self, url: &str) -> Result<Response>;
}

/// Blocking HTTP client. Concurrency is a bounded worker pool over this, which is
/// what the downloader needs; nothing here requires an async runtime.
pub struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    /// Client with a 30 second deadline per request.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Client with an explicit per-request deadline.
    pub fn with_timeout(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();

        Self {
            agent: config.into(),
        }
    }
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for UreqClient {
    fn get(&self, url: &str) -> Result<Response> {
        let mut response = self.agent.get(url).call().map_err(|error| Error::Http {
            url: redact(url),
            reason: error.to_string(),
        })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| Error::Http {
                url: redact(url),
                reason: error.to_string(),
            })?;

        Ok(Response { status, body })
    }
}

/// Strip the query string, so an authorization code or a token never reaches a log
/// or an error message.
pub fn redact(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<redacted>"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_string_is_never_kept() {
        assert_eq!(
            redact("https://auth.gog.com/token?client_secret=shhh&code=abc"),
            "https://auth.gog.com/token?<redacted>"
        );
        assert_eq!(
            redact("https://auth.gog.com/token"),
            "https://auth.gog.com/token"
        );
    }

    #[test]
    fn success_is_the_2xx_range() {
        let response = |status| Response {
            status,
            body: String::new(),
        };

        assert!(response(200).is_success());
        assert!(response(204).is_success());
        assert!(!response(302).is_success());
        assert!(!response(400).is_success());
    }
}
