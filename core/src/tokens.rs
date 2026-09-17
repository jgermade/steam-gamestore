//! Storing the GOG session, and keeping its access token fresh.
//!
//! The store is a trait: the keyring is the real backend, and tests use an
//! in-memory one. What to do on a machine with no Secret Service — which is the
//! likely case in console mode — is still undecided, so there is deliberately no
//! file-backed store here yet.

use std::time::SystemTime;

use crate::auth::{self, Credentials, TokenSet};
use crate::http::HttpClient;
use crate::{Error, Result};

/// Service name the tokens are stored under.
pub const KEYRING_SERVICE: &str = "gamestore";

/// Somewhere to keep a GOG session between runs.
pub trait TokenStore {
    /// The stored session, or `None` when there is none.
    fn load(&self) -> Result<Option<TokenSet>>;
    /// Replace the stored session.
    fn save(&self, tokens: &TokenSet) -> Result<()>;
    /// Forget the stored session. Clearing an empty store is not an error.
    fn clear(&self) -> Result<()>;
    /// Where this store keeps things, for `gamestore info`.
    fn describe(&self) -> String;
}

/// The platform keyring: Secret Service on Linux, the credential manager on
/// Windows.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    account: String,
}

impl KeyringStore {
    /// One entry per account name, so two Steam users on the same machine keep
    /// separate GOG sessions.
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account).map_err(keyring_error)
    }
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Result<Option<TokenSet>> {
        match self.entry()?.get_password() {
            Ok(stored) => Ok(Some(serde_json::from_str(&stored).map_err(|error| {
                Error::TokenStore(format!(
                    "the stored session could not be read ({error}); log in again"
                ))
            })?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn save(&self, tokens: &TokenSet) -> Result<()> {
        let serialized = serde_json::to_string(tokens).map_err(|error| {
            Error::TokenStore(format!("the session could not be written ({error})"))
        })?;

        self.entry()?
            .set_password(&serialized)
            .map_err(keyring_error)
    }

    fn clear(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn describe(&self) -> String {
        format!("platform keyring ({KEYRING_SERVICE}/{})", self.account)
    }
}

fn keyring_error(error: keyring::Error) -> Error {
    // `NoDefaultStore` only says the store could not be initialized; the reason it
    // could not — usually no Secret Service running, which is the normal state of a
    // machine booted straight into gamescope — is behind `store_status`.
    let reason = match (&error, keyring::Entry::store_status()) {
        (keyring::Error::NoDefaultStore, Err(cause)) => format!("{error} ({cause})"),
        _ => error.to_string(),
    };

    Error::TokenStore(format!("the platform keyring is not usable: {reason}"))
}

/// A store that keeps the session for the lifetime of the process. For tests, and
/// for a run that deliberately keeps nothing.
#[derive(Debug, Default)]
pub struct MemoryStore {
    tokens: std::sync::Mutex<Option<TokenSet>>,
}

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<TokenSet>> {
        Ok(self.tokens.lock().expect("token mutex").clone())
    }

    fn save(&self, tokens: &TokenSet) -> Result<()> {
        *self.tokens.lock().expect("token mutex") = Some(tokens.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.tokens.lock().expect("token mutex") = None;
        Ok(())
    }

    fn describe(&self) -> String {
        "memory (this run only)".to_string()
    }
}

/// An authenticated GOG session: the tokens, where they live, and the refresh that
/// keeps them usable.
pub struct Session<H: HttpClient, S: TokenStore> {
    credentials: Credentials,
    http: H,
    store: S,
    tokens: Option<TokenSet>,
}

impl<H: HttpClient, S: TokenStore> Session<H, S> {
    /// A session that has not looked at the store yet.
    pub fn new(credentials: Credentials, http: H, store: S) -> Self {
        Self {
            credentials,
            http,
            store,
            tokens: None,
        }
    }

    /// Load the stored session. Returns whether there was one.
    pub fn restore(&mut self) -> Result<bool> {
        self.tokens = self.store.load()?;
        Ok(self.tokens.is_some())
    }

    /// Complete a login from what the user pasted, and store the result.
    pub fn log_in(&mut self, pasted: &str) -> Result<()> {
        let code = auth::extract_code(pasted)?;
        let tokens = auth::exchange_code(&self.http, &self.credentials, &code)?;
        self.store.save(&tokens)?;
        self.tokens = Some(tokens);

        Ok(())
    }

    /// Forget the session, here and in the store.
    pub fn log_out(&mut self) -> Result<()> {
        self.tokens = None;
        self.store.clear()
    }

    /// The credentials this session authenticates with.
    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// Where this session's tokens are kept.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The GOG user id, when there is a session.
    pub fn user_id(&self) -> Option<&str> {
        self.tokens.as_ref().map(|tokens| tokens.user_id.as_str())
    }

    /// When the current access token stops being accepted.
    pub fn expires_at(&self) -> Option<SystemTime> {
        self.tokens.as_ref().map(|tokens| tokens.expires_at)
    }

    /// A usable access token, refreshing and re-storing it if it is due.
    pub fn access_token(&mut self) -> Result<String> {
        self.access_token_at(SystemTime::now())
    }

    /// [`Session::access_token`] with an explicit clock.
    pub fn access_token_at(&mut self, now: SystemTime) -> Result<String> {
        let tokens = self.tokens.as_ref().ok_or(Error::NotLoggedIn)?;
        if !tokens.is_expired(now) {
            return Ok(tokens.access_token.clone());
        }

        // GOG rotates the refresh token, so what comes back replaces what went in.
        match auth::refresh(&self.http, &self.credentials, &tokens.refresh_token) {
            Ok(refreshed) => {
                self.store.save(&refreshed)?;
                let access_token = refreshed.access_token.clone();
                self.tokens = Some(refreshed);
                Ok(access_token)
            }
            Err(Error::Auth(reason)) => {
                // The refresh token is gone or revoked: the stored session is
                // useless, and keeping it only makes the next run fail the same way.
                self.tokens = None;
                self.store.clear()?;
                Err(Error::Auth(format!(
                    "{reason}; the stored session was discarded, run `gamestore login` again"
                )))
            }
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;
    use std::cell::RefCell;
    use std::time::Duration;

    struct ScriptedHttp {
        responses: RefCell<Vec<Response>>,
        calls: RefCell<Vec<String>>,
    }

    impl ScriptedHttp {
        fn new(bodies: &[(u16, &str)]) -> Self {
            Self {
                responses: RefCell::new(
                    bodies
                        .iter()
                        .rev()
                        .map(|(status, body)| Response {
                            status: *status,
                            body: body.to_string(),
                        })
                        .collect(),
                ),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl HttpClient for ScriptedHttp {
        fn get(&self, url: &str) -> Result<Response> {
            self.calls.borrow_mut().push(url.to_string());
            self.responses
                .borrow_mut()
                .pop()
                .ok_or_else(|| Error::Http {
                    url: crate::http::redact(url),
                    reason: "the test ran out of scripted responses".to_string(),
                })
        }
    }

    fn token_body(access: &str, refresh: &str, expires_in: u64) -> String {
        format!(
            r#"{{"access_token":"{access}","refresh_token":"{refresh}",
                "user_id":"51000000000000000","session_id":"a-session",
                "expires_in":{expires_in},"token_type":"bearer"}}"#
        )
    }

    fn credentials() -> Credentials {
        Credentials::new("an-id", "a-secret")
    }

    const T0: SystemTime = SystemTime::UNIX_EPOCH;

    fn tokens_expiring_at(seconds: u64, refresh_token: &str) -> TokenSet {
        TokenSet {
            access_token: "stored".to_string(),
            refresh_token: refresh_token.to_string(),
            user_id: "51000000000000000".to_string(),
            session_id: None,
            expires_at: T0 + Duration::from_secs(seconds),
        }
    }

    /// A session whose store already holds `tokens`, restored and ready. Seeding
    /// the store instead of logging in keeps the test on the T0 clock: a login
    /// computes its expiry from the real one.
    fn restored_session(
        tokens: TokenSet,
        http: ScriptedHttp,
    ) -> Session<ScriptedHttp, MemoryStore> {
        let store = MemoryStore::default();
        store.save(&tokens).unwrap();
        let mut session = Session::new(credentials(), http, store);
        assert!(session.restore().unwrap());
        session
    }

    #[test]
    fn logging_in_stores_the_session() {
        let http = ScriptedHttp::new(&[(200, &token_body("first", "refresh-1", 3600))]);
        let mut session = Session::new(credentials(), http, MemoryStore::default());

        session
            .log_in("https://embed.gog.com/on_login_success?code=abcdef1234567890")
            .unwrap();

        assert_eq!(session.user_id(), Some("51000000000000000"));
        let stored = session.store.load().unwrap().unwrap();
        assert_eq!(stored.access_token, "first");
    }

    #[test]
    fn a_stored_session_comes_back_and_a_valid_token_is_not_refreshed() {
        let mut session = restored_session(
            tokens_expiring_at(3600, "refresh-1"),
            ScriptedHttp::new(&[]),
        );

        assert_eq!(session.access_token_at(T0).unwrap(), "stored");
        assert_eq!(session.http.calls(), 0);
    }

    #[test]
    fn an_empty_store_restores_to_nothing_and_refuses_to_pretend() {
        let mut session = Session::new(
            credentials(),
            ScriptedHttp::new(&[]),
            MemoryStore::default(),
        );

        assert!(!session.restore().unwrap());
        assert!(matches!(
            session.access_token_at(T0).unwrap_err(),
            Error::NotLoggedIn
        ));
    }

    #[test]
    fn an_expiring_token_is_refreshed_and_the_rotation_is_stored() {
        let mut session = restored_session(
            tokens_expiring_at(3600, "refresh-1"),
            ScriptedHttp::new(&[(200, &token_body("second", "refresh-2", 3600))]),
        );

        // Inside the one minute margin, so it counts as due.
        let token = session
            .access_token_at(T0 + Duration::from_secs(3590))
            .unwrap();

        assert_eq!(token, "second");
        assert_eq!(session.http.calls(), 1);
        assert!(
            session.http.calls.borrow()[0].contains("grant_type=refresh_token"),
            "the refresh has to use the refresh grant"
        );
        let stored = session.store.load().unwrap().unwrap();
        assert_eq!(stored.access_token, "second");
        assert_eq!(
            stored.refresh_token, "refresh-2",
            "the rotated refresh token has to replace the old one, or the next \
             refresh fails"
        );
    }

    #[test]
    fn a_revoked_refresh_token_discards_the_stored_session() {
        let mut session = restored_session(
            tokens_expiring_at(3600, "refresh-1"),
            ScriptedHttp::new(&[(
                400,
                r#"{"error":"invalid_grant","error_description":"Token revoked"}"#,
            )]),
        );

        let error = session
            .access_token_at(T0 + Duration::from_secs(4000))
            .unwrap_err();

        assert!(error.to_string().contains("Token revoked"), "{error}");
        assert!(error.to_string().contains("gamestore login"), "{error}");
        assert!(session.store.load().unwrap().is_none());
        assert_eq!(session.user_id(), None);
    }

    #[test]
    fn a_network_failure_during_refresh_keeps_the_session() {
        // No scripted response at all, so the refresh fails as a transport error.
        let mut session = restored_session(
            tokens_expiring_at(3600, "refresh-1"),
            ScriptedHttp::new(&[]),
        );
        let error = session
            .access_token_at(T0 + Duration::from_secs(4000))
            .unwrap_err();

        assert!(matches!(error, Error::Http { .. }), "{error}");
        assert!(
            session.store.load().unwrap().is_some(),
            "a flaky network must not log the user out"
        );
    }

    #[test]
    fn logging_out_clears_both_the_session_and_the_store() {
        let http = ScriptedHttp::new(&[(200, &token_body("first", "refresh-1", 3600))]);
        let mut session = Session::new(credentials(), http, MemoryStore::default());
        session.log_in("abcdef1234567890").unwrap();

        session.log_out().unwrap();

        assert!(session.store.load().unwrap().is_none());
        assert_eq!(session.user_id(), None);
    }

    #[test]
    fn a_round_trip_through_the_store_keeps_every_field() {
        let tokens = TokenSet {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            user_id: "51000000000000000".to_string(),
            session_id: Some("s".to_string()),
            expires_at: T0 + Duration::from_secs(1234),
        };

        let json = serde_json::to_string(&tokens).unwrap();
        let back: TokenSet = serde_json::from_str(&json).unwrap();

        assert_eq!(back, tokens);
    }
}
