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
///
/// The authorized form is the one implementors provide, and the plain one
/// delegates to it. That way round on purpose: a trait where `get` is required and
/// the authorized call defaults to it would let a client silently drop the
/// `Authorization` header and look like an expired session.
pub trait HttpClient {
    /// GET `url`, sending `bearer` as an OAuth2 bearer token when there is one.
    fn get_authorized(&self, url: &str, bearer: Option<&str>) -> Result<Response>;

    /// GET `url` with no credentials.
    fn get(&self, url: &str) -> Result<Response> {
        self.get_authorized(url, None)
    }
}

/// Binary downloads, which [`HttpClient`] cannot carry: its bodies are text.
pub trait Fetch {
    /// GET `url` from byte `from` on. Returns the status before any of the body is
    /// read, so the caller can tell a honoured range (206) from a server that sent
    /// the whole file anyway (200) before it decides where the bytes go.
    fn open(&self, url: &str, from: u64) -> Result<(u16, Box<dyn std::io::Read>)>;
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
    fn get_authorized(&self, url: &str, bearer: Option<&str>) -> Result<Response> {
        let mut request = self.agent.get(url);
        if let Some(token) = bearer {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }

        let mut response = request.call().map_err(|error| Error::Http {
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

/// Client for large downloads. There is no whole-request deadline, which a
/// multi-gigabyte file would always hit; connecting and the response headers still
/// have one, so a dead CDN fails rather than hangs.
pub struct DownloadClient {
    agent: ureq::Agent,
}

impl DownloadClient {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for DownloadClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for DownloadClient {
    fn open(&self, url: &str, from: u64) -> Result<(u16, Box<dyn std::io::Read>)> {
        let mut request = self.agent.get(url);
        if from > 0 {
            request = request.header("Range", &format!("bytes={from}-"));
        }
        let response = request.call().map_err(|error| Error::Http {
            url: redact(url),
            reason: error.to_string(),
        })?;
        let status = response.status().as_u16();
        Ok((status, Box::new(response.into_body().into_reader())))
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
