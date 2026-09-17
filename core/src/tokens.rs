//! Storing the GOG session, and keeping its access token fresh.
//!
//! The store is a trait with three backends: the platform keyring, a file, and an
//! in-memory one for tests. [`open_store`] picks between the first two — the
//! keyring when it works, the file when it does not, which on a machine booted
//! straight into gamescope is the normal case rather than the exception.
//!
//! The file is **not encrypted**, and nothing here pretends otherwise. Encrypting
//! it was considered and rejected: the key would have to live where the same
//! machine can read it unattended, so it would be obfuscation sold as secrecy. A
//! `0600` file gives the same real protection against everything short of another
//! local user reading it, and says so plainly in `gamestore info`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::auth::{self, Credentials, TokenSet};
use crate::http::HttpClient;
use crate::{Error, Paths, Result};

/// Service name the tokens are stored under.
pub const KEYRING_SERVICE: &str = "gamestore";

/// Environment variable naming the stored session, so two Steam users sharing a
/// machine keep separate GOG logins. The data directory override separates their
/// files; this separates their keyring entries, which are per-machine.
pub const ACCOUNT_ENV: &str = "GAMESTORE_ACCOUNT";

/// Account name used when [`ACCOUNT_ENV`] says nothing.
pub const DEFAULT_ACCOUNT: &str = "gog";

/// What the file store's permissions actually amount to, for `describe`.
#[cfg(unix)]
const PERMISSIONS_NOTE: &str = "unencrypted, 0600";
/// What the file store's permissions actually amount to, for `describe`.
#[cfg(not(unix))]
const PERMISSIONS_NOTE: &str = "unencrypted, readable by this user account";

/// The account the session is stored under, from [`ACCOUNT_ENV`] or the default.
pub fn account_from_env() -> String {
    std::env::var(ACCOUNT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT.to_string())
}

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

/// So the backend can be chosen at run time without making every caller generic.
impl TokenStore for Box<dyn TokenStore> {
    fn load(&self) -> Result<Option<TokenSet>> {
        (**self).load()
    }

    fn save(&self, tokens: &TokenSet) -> Result<()> {
        (**self).save(tokens)
    }

    fn clear(&self) -> Result<()> {
        (**self).clear()
    }

    fn describe(&self) -> String {
        (**self).describe()
    }
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

    /// Whether this machine has a keyring that actually answers.
    ///
    /// `Entry::new` succeeding is not enough: it can build an entry and then fail
    /// on the first read. A read that finds nothing is the healthy empty case, so
    /// this asks for the entry and treats `None` as success.
    pub fn is_usable(&self) -> Result<()> {
        self.load().map(|_| ())
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

/// A plain JSON file, `0600`, for machines with no usable keyring.
///
/// Deliberately not encrypted; see the module documentation for why. Whoever can
/// read this file can use the session, which is why [`FileStore::describe`] says
/// so and `gamestore info` prints it.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// A store at an exact path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The per-account file under the data directory, which the directory
    /// overrides already separate per Steam user.
    pub fn for_account(paths: &Paths, account: &str) -> Self {
        Self::new(paths.data_dir.join("sessions").join(file_name(account)))
    }

    /// Where the session is kept.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An account name as a file name: anything that is not obviously safe in a path
/// becomes `_`, because the account can come from the environment.
fn file_name(account: &str) -> String {
    let safe: String = account
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();

    format!(
        "{}.json",
        if safe.is_empty() {
            DEFAULT_ACCOUNT
        } else {
            &safe
        }
    )
}

impl TokenStore for FileStore {
    fn load(&self) -> Result<Option<TokenSet>> {
        match std::fs::read_to_string(&self.path) {
            Ok(stored) => Ok(Some(serde_json::from_str(&stored).map_err(|error| {
                Error::TokenStore(format!(
                    "the session stored in {} could not be read ({error}); log in again",
                    self.path.display()
                ))
            })?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::io(format!("reading {}", self.path.display()), error)),
        }
    }

    fn save(&self, tokens: &TokenSet) -> Result<()> {
        let serialized = serde_json::to_string(tokens).map_err(|error| {
            Error::TokenStore(format!("the session could not be written ({error})"))
        })?;

        write_private(&self.path, &serialized)
    }

    fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::io(
                format!("removing {}", self.path.display()),
                error,
            )),
        }
    }

    fn describe(&self) -> String {
        format!("file ({}, {PERMISSIONS_NOTE})", self.path.display())
    }
}

/// Write `contents` so that only this user can read it, and so that a machine
/// switched off at the wall never leaves a half-written session behind: the bytes
/// land in a temporary file that is renamed over the target.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| Error::TokenStore(format!("{} has no parent directory", path.display())))?;
    create_dir_private(parent)?;

    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temporary)
        .map_err(|error| Error::io(format!("creating {}", temporary.display()), error))?;

    // `mode` above only applies when the file is created, so a temporary left by a
    // previous crash would keep whatever permissions it had. Set them either way.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                Error::io(
                    format!("setting permissions on {}", temporary.display()),
                    error,
                )
            })?;
    }

    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::io(format!("writing {}", temporary.display()), error));
    }
    drop(file);

    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::io(
            format!("replacing {} with {}", path.display(), temporary.display()),
            error,
        )
    })
}

/// Create a directory only this user can enter.
fn create_dir_private(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
            Error::io(format!("setting permissions on {}", dir.display()), error)
        })?;
    }

    Ok(())
}

/// A chosen token store, and why it is the one that was chosen.
pub struct OpenedStore {
    /// Where the session will be kept.
    pub store: Box<dyn TokenStore>,
    /// Why the keyring is not being used, when it is not. Worth printing: it is
    /// the difference between tokens in a secret service and tokens in a file.
    pub keyring_unavailable: Option<String>,
}

/// Keep the session in the platform keyring, falling back to a `0600` file.
///
/// The fallback is not a corner case on the target machine: a box booted straight
/// into gamescope has no D-Bus session and therefore no Secret Service. It is
/// taken silently by the code and loudly by the caller — [`OpenedStore`] carries
/// the reason so `login` and `info` can state where the tokens ended up.
pub fn open_store(paths: &Paths, account: &str) -> OpenedStore {
    let keyring = KeyringStore::new(account);

    match keyring.is_usable() {
        Ok(()) => OpenedStore {
            store: Box::new(keyring),
            keyring_unavailable: None,
        },
        Err(error) => OpenedStore {
            store: Box::new(FileStore::for_account(paths, account)),
            keyring_unavailable: Some(error.to_string()),
        },
    }
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

    // --- The file store ---------------------------------------------------

    fn paths_in(dir: &std::path::Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        }
    }

    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_file_store_round_trips_a_session_through_a_directory_that_did_not_exist() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");
        let tokens = tokens_expiring_at(3600, "refresh-1");

        assert!(store.load().unwrap().is_none(), "nothing stored yet");
        store.save(&tokens).unwrap();

        assert_eq!(store.load().unwrap(), Some(tokens));
        assert!(store.path().exists());
    }

    #[test]
    fn clearing_a_file_store_removes_it_and_clearing_an_empty_one_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");
        store.save(&tokens_expiring_at(3600, "refresh-1")).unwrap();

        store.clear().unwrap();

        assert!(!store.path().exists());
        assert!(store.load().unwrap().is_none());
        store.clear().unwrap();
    }

    #[test]
    fn an_unreadable_session_file_says_to_log_in_again_rather_than_panicking() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");
        store.save(&tokens_expiring_at(3600, "refresh-1")).unwrap();
        std::fs::write(store.path(), "not json at all").unwrap();

        let error = store.load().unwrap_err();

        assert!(error.to_string().contains("log in again"), "{error}");
    }

    #[test]
    fn the_file_store_says_out_loud_that_it_is_not_encrypted() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");

        let description = store.describe();

        assert!(description.contains("unencrypted"), "{description}");
        assert!(
            description.contains(&store.path().display().to_string()),
            "the description has to name the file: {description}"
        );
    }

    #[test]
    fn an_account_name_cannot_escape_the_sessions_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let sessions = paths.data_dir.join("sessions");

        for hostile in ["../../etc/passwd", "a/b", "..", ""] {
            let store = FileStore::for_account(&paths, hostile);
            assert_eq!(
                store.path().parent(),
                Some(sessions.as_path()),
                "{hostile:?} escaped to {}",
                store.path().display()
            );
        }
    }

    #[test]
    fn two_accounts_keep_separate_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let one = FileStore::for_account(&paths, "player-one");
        let two = FileStore::for_account(&paths, "player-two");

        one.save(&tokens_expiring_at(3600, "refresh-one")).unwrap();

        assert_ne!(one.path(), two.path());
        assert!(two.load().unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_session_file_and_its_directory_are_readable_only_by_this_user() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");

        store.save(&tokens_expiring_at(3600, "refresh-1")).unwrap();

        assert_eq!(mode_of(store.path()), 0o600);
        assert_eq!(mode_of(store.path().parent().unwrap()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_leftover_does_not_survive_the_next_save() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let store = FileStore::for_account(&paths_in(temp.path()), "gog");
        store.save(&tokens_expiring_at(3600, "refresh-1")).unwrap();

        // What a crash between the write and the rename leaves behind, with the
        // permissions an unlucky umask would have given it.
        let leftover = store.path().with_extension("json.tmp");
        std::fs::write(&leftover, "stale").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o644)).unwrap();

        store.save(&tokens_expiring_at(7200, "refresh-2")).unwrap();

        assert_eq!(
            mode_of(store.path()),
            0o600,
            "the rename must not carry 0644 over"
        );
        assert!(
            !leftover.exists(),
            "the temporary file must not be left behind"
        );
        assert_eq!(store.load().unwrap().unwrap().refresh_token, "refresh-2");
    }

    #[test]
    fn a_session_refreshes_through_a_boxed_store_like_any_other() {
        let temp = tempfile::tempdir().unwrap();
        let store: Box<dyn TokenStore> =
            Box::new(FileStore::for_account(&paths_in(temp.path()), "gog"));
        store.save(&tokens_expiring_at(3600, "refresh-1")).unwrap();

        let http = ScriptedHttp::new(&[(200, &token_body("second", "refresh-2", 3600))]);
        let mut session = Session::new(credentials(), http, store);
        assert!(session.restore().unwrap());

        let token = session
            .access_token_at(T0 + Duration::from_secs(3590))
            .unwrap();

        assert_eq!(token, "second");
        assert_eq!(
            session.store().load().unwrap().unwrap().refresh_token,
            "refresh-2",
            "the rotation has to reach the file through the box"
        );
    }

    #[test]
    fn open_store_falls_back_to_a_file_exactly_when_it_says_it_did() {
        // Which branch is taken depends on the machine: a desktop has a Secret
        // Service, a CI container and a gamescope console do not. What must hold
        // either way is that the description and the reason agree, because that
        // pair is what `gamestore info` shows the user.
        let temp = tempfile::tempdir().unwrap();
        let opened = open_store(&paths_in(temp.path()), "gog");
        let description = opened.store.describe();

        match &opened.keyring_unavailable {
            Some(reason) => {
                assert!(description.starts_with("file ("), "{description}");
                assert!(description.contains("unencrypted"), "{description}");
                assert!(reason.contains("keyring"), "{reason}");
            }
            None => assert!(description.starts_with("platform keyring"), "{description}"),
        }
    }
}
