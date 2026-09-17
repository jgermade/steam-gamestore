//! GOG OAuth2 authorization code flow.
//!
//! GOG publishes no device authorization grant, and the client credentials in use
//! have their `redirect_uri` fixed on GOG's side, so nothing can hand the
//! authorization code back to this machine on its own: the user logs in in a
//! browser and pastes what they land on. See
//! `RECORD/2026-09-16.gog-login-flow-decision.completed.md` for the reasoning and
//! for the two checks that would allow a better flow.
//!
//! Storing the tokens is the `tok` task; this module only obtains and refreshes
//! them.

use std::fmt;
use std::time::{Duration, SystemTime};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::http::{HttpClient, Response};
use crate::{Error, Result};

/// Where the user logs in.
pub const AUTH_ENDPOINT: &str = "https://auth.gog.com/auth";
/// Where a code or a refresh token is exchanged for an access token.
pub const TOKEN_ENDPOINT: &str = "https://auth.gog.com/token";
/// The redirect the GOG Galaxy client credentials are registered with. The login
/// lands here with `?code=…` in the address, which is what the user pastes back.
pub const GALAXY_REDIRECT_URI: &str = "https://embed.gog.com/on_login_success?origin=client";

/// Environment variable holding the OAuth2 client id.
pub const CLIENT_ID_ENV: &str = "GOG_CLIENT_ID";
/// Environment variable holding the OAuth2 client secret.
pub const CLIENT_SECRET_ENV: &str = "GOG_CLIENT_SECRET";

/// OAuth2 client credentials.
///
/// Which credentials to use is still open (`08-open-questions.md`, question 3):
/// ship the well-known GOG Galaxy ones as other clients do, or make the user
/// supply their own. Nothing is baked in here yet, so both answers stay available.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl Credentials {
    /// Credentials with the redirect the Galaxy client is registered with.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_uri: GALAXY_REDIRECT_URI.to_string(),
        }
    }

    /// Use a different redirect than [`GALAXY_REDIRECT_URI`].
    pub fn with_redirect_uri(mut self, redirect_uri: impl Into<String>) -> Self {
        self.redirect_uri = redirect_uri.into();
        self
    }

    /// The URL to open in a browser to log in.
    pub fn authorization_url(&self) -> String {
        format!(
            "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&layout=client2",
            encode(&self.client_id),
            encode(&self.redirect_uri),
        )
    }
}

/// Never print the secret, not even through `{:?}`.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// Pull the authorization code out of what the user pasted: either the whole
/// address they landed on, or the bare code.
pub fn extract_code(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::Auth("nothing was pasted".to_string()));
    }

    let Some((_, query)) = input.split_once('?') else {
        return validate_code(input);
    };

    let mut code = None;
    let mut error = None;
    let mut description = None;
    for (key, value) in query_pairs(query) {
        match key.as_str() {
            "code" => code = Some(value),
            "error" => error = Some(value),
            "error_description" => description = Some(value),
            _ => {}
        }
    }

    if let Some(error) = error {
        return Err(Error::Auth(match description {
            Some(description) => format!("{error}: {description}"),
            None => error,
        }));
    }

    match code {
        Some(code) => validate_code(&code),
        None => Err(Error::Auth(
            "that address has no `code` in it; log in again and paste the address you land on"
                .to_string(),
        )),
    }
}

/// Exchange an authorization code for tokens.
pub fn exchange_code(
    http: &impl HttpClient,
    credentials: &Credentials,
    code: &str,
) -> Result<TokenSet> {
    let code = validate_code(code)?;
    let url = format!(
        "{TOKEN_ENDPOINT}?client_id={}&client_secret={}&grant_type=authorization_code&code={}&redirect_uri={}",
        encode(&credentials.client_id),
        encode(&credentials.client_secret),
        encode(&code),
        encode(&credentials.redirect_uri),
    );

    token_request(http, &url, SystemTime::now())
}

/// Trade a refresh token for a fresh access token. GOG rotates the refresh token,
/// so the one that comes back replaces the one that went in.
pub fn refresh(
    http: &impl HttpClient,
    credentials: &Credentials,
    refresh_token: &str,
) -> Result<TokenSet> {
    let url = format!(
        "{TOKEN_ENDPOINT}?client_id={}&client_secret={}&grant_type=refresh_token&refresh_token={}",
        encode(&credentials.client_id),
        encode(&credentials.client_secret),
        encode(refresh_token),
    );

    token_request(http, &url, SystemTime::now())
}

/// Tokens for an authenticated session.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub session_id: Option<String>,
    /// When the access token stops being accepted.
    pub expires_at: SystemTime,
}

impl TokenSet {
    /// Whether the access token needs refreshing at `now`, with a minute of margin
    /// so a request does not start on a token that expires mid-flight.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.expires_at
            .duration_since(now)
            .map(|left| left < Duration::from_secs(60))
            .unwrap_or(true)
    }

    fn from_response(response: TokenResponse, now: SystemTime) -> Self {
        Self {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            user_id: response.user_id,
            session_id: response.session_id,
            expires_at: now + Duration::from_secs(response.expires_in),
        }
    }
}

/// Never print the tokens, not even through `{:?}`.
impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("user_id", &self.user_id)
            .field(
                "session_id",
                &self.session_id.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    user_id: String,
    session_id: Option<String>,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

fn token_request(http: &impl HttpClient, url: &str, now: SystemTime) -> Result<TokenSet> {
    let response = http.get(url)?;

    if !response.is_success() {
        return Err(Error::Auth(describe_failure(&response)));
    }

    let parsed: TokenResponse = serde_json::from_str(&response.body).map_err(|source| {
        Error::Auth(format!(
            "GOG returned a token response that could not be read ({source}); \
             the flow may have changed"
        ))
    })?;

    Ok(TokenSet::from_response(parsed, now))
}

fn describe_failure(response: &Response) -> String {
    let status = response.status;
    match serde_json::from_str::<ErrorResponse>(&response.body) {
        Ok(ErrorResponse {
            error: Some(error),
            error_description: Some(description),
        }) => format!("GOG rejected the request ({status}): {error}: {description}"),
        Ok(ErrorResponse {
            error: Some(error), ..
        }) => format!("GOG rejected the request ({status}): {error}"),
        _ => format!("GOG rejected the request ({status})"),
    }
}

fn validate_code(code: &str) -> Result<String> {
    let code = code.trim();
    let plausible = code.len() >= 8
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));

    if plausible {
        Ok(code.to_string())
    } else {
        Err(Error::Auth(
            "that does not look like an authorization code; paste the whole address \
             you landed on, or just the value of `code`"
                .to_string(),
        ))
    }
}

fn query_pairs(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query.split('&').filter_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        Some((key.to_string(), decode(value)))
    })
}

/// Everything but the RFC 3986 unreserved characters. Encoding `-._~` as well
/// would be legal, but GOG compares the `redirect_uri` against the one the client
/// is registered with, so the value it sees should be the obvious spelling.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn encode(value: &str) -> String {
    utf8_percent_encode(value, QUERY_VALUE).to_string()
}

fn decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeHttp {
        response: Response,
        seen: RefCell<Vec<String>>,
    }

    impl FakeHttp {
        fn new(status: u16, body: &str) -> Self {
            Self {
                response: Response {
                    status,
                    body: body.to_string(),
                },
                seen: RefCell::new(Vec::new()),
            }
        }

        fn last_url(&self) -> String {
            self.seen.borrow().last().cloned().unwrap_or_default()
        }
    }

    impl HttpClient for FakeHttp {
        fn get(&self, url: &str) -> Result<Response> {
            self.seen.borrow_mut().push(url.to_string());
            Ok(self.response.clone())
        }
    }

    fn credentials() -> Credentials {
        Credentials::new("46899977096215655", "secret-value")
    }

    const TOKEN_BODY: &str = r#"{
        "access_token": "an-access-token",
        "refresh_token": "a-refresh-token",
        "user_id": "51000000000000000",
        "session_id": "a-session",
        "expires_in": 3600,
        "token_type": "bearer",
        "scope": ""
    }"#;

    #[test]
    fn the_authorization_url_carries_the_encoded_redirect() {
        let url = credentials().authorization_url();

        assert!(url.starts_with(AUTH_ENDPOINT), "{url}");
        assert!(url.contains("client_id=46899977096215655"), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient"
            ),
            "{url}"
        );
    }

    #[test]
    fn the_code_comes_out_of_a_pasted_address() {
        let code = extract_code(
            "https://embed.gog.com/on_login_success?origin=client&code=abcdef1234567890 ",
        )
        .unwrap();

        assert_eq!(code, "abcdef1234567890");
    }

    #[test]
    fn a_bare_code_is_accepted_too() {
        assert_eq!(
            extract_code(" abcdef1234567890\n").unwrap(),
            "abcdef1234567890"
        );
    }

    #[test]
    fn a_denied_login_reports_what_gog_said() {
        let error = extract_code(
            "https://embed.gog.com/on_login_success?error=access_denied&error_description=User%20said%20no",
        )
        .unwrap_err();

        assert!(error.to_string().contains("access_denied"), "{error}");
        assert!(error.to_string().contains("User said no"), "{error}");
    }

    #[test]
    fn an_address_without_a_code_says_what_to_do() {
        let error =
            extract_code("https://embed.gog.com/on_login_success?origin=client").unwrap_err();

        assert!(error.to_string().contains("`code`"), "{error}");
    }

    #[test]
    fn garbage_is_not_mistaken_for_a_code() {
        for input in ["", "   ", "nope", "not a code at all"] {
            assert!(extract_code(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn exchanging_a_code_asks_for_the_authorization_code_grant() {
        let http = FakeHttp::new(200, TOKEN_BODY);

        let tokens = exchange_code(&http, &credentials(), "abcdef1234567890").unwrap();

        let url = http.last_url();
        assert!(url.starts_with(TOKEN_ENDPOINT), "{url}");
        assert!(url.contains("grant_type=authorization_code"), "{url}");
        assert!(url.contains("code=abcdef1234567890"), "{url}");
        assert!(url.contains("client_secret=secret-value"), "{url}");
        assert_eq!(tokens.access_token, "an-access-token");
        assert_eq!(tokens.refresh_token, "a-refresh-token");
        assert_eq!(tokens.user_id, "51000000000000000");
        assert_eq!(tokens.session_id.as_deref(), Some("a-session"));
    }

    #[test]
    fn refreshing_asks_for_the_refresh_token_grant() {
        let http = FakeHttp::new(200, TOKEN_BODY);

        refresh(&http, &credentials(), "a-refresh-token").unwrap();

        let url = http.last_url();
        assert!(url.contains("grant_type=refresh_token"), "{url}");
        assert!(url.contains("refresh_token=a-refresh-token"), "{url}");
    }

    #[test]
    fn expiry_is_computed_from_expires_in_with_a_minute_of_margin() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let response: TokenResponse = serde_json::from_str(TOKEN_BODY).unwrap();

        let tokens = TokenSet::from_response(response, now);

        assert_eq!(tokens.expires_at, now + Duration::from_secs(3600));
        assert!(!tokens.is_expired(now));
        assert!(!tokens.is_expired(now + Duration::from_secs(3500)));
        assert!(tokens.is_expired(now + Duration::from_secs(3550)));
        assert!(tokens.is_expired(now + Duration::from_secs(7200)));
    }

    #[test]
    fn a_rejected_exchange_reports_gogs_own_message() {
        let http = FakeHttp::new(
            400,
            r#"{"error":"invalid_grant","error_description":"Code has expired"}"#,
        );

        let error = exchange_code(&http, &credentials(), "abcdef1234567890").unwrap_err();

        assert!(error.to_string().contains("invalid_grant"), "{error}");
        assert!(error.to_string().contains("Code has expired"), "{error}");
    }

    #[test]
    fn an_unreadable_token_response_is_an_error_not_a_panic() {
        let http = FakeHttp::new(200, "<html>maintenance</html>");

        let error = exchange_code(&http, &credentials(), "abcdef1234567890").unwrap_err();

        assert!(error.to_string().contains("could not be read"), "{error}");
    }

    #[test]
    fn neither_secrets_nor_tokens_show_up_in_debug_output() {
        let http = FakeHttp::new(200, TOKEN_BODY);
        let tokens = exchange_code(&http, &credentials(), "abcdef1234567890").unwrap();

        let credentials = format!("{:?}", credentials());
        let tokens = format!("{tokens:?}");

        assert!(!credentials.contains("secret-value"), "{credentials}");
        assert!(!tokens.contains("an-access-token"), "{tokens}");
        assert!(!tokens.contains("a-refresh-token"), "{tokens}");
        assert!(!tokens.contains("a-session"), "{tokens}");
        assert!(tokens.contains("51000000000000000"), "{tokens}");
    }
}
