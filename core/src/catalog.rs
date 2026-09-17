//! The GOG library: what the account owns, cached so browsing works offline.
//!
//! # The response shapes here are assumptions
//!
//! GOG publishes no specification for these endpoints. The field names below are
//! taken from what other open-source clients read, and **nothing in this file has
//! been checked against GOG**, because the session that wrote it cannot reach
//! `embed.gog.com`. So the parsing is written to be forgiving in the ways that cost
//! nothing — unknown fields are ignored, a missing optional field is `None`, and a
//! product that cannot be read is skipped rather than failing the whole page — and
//! strict where being wrong would be silent. When the mini PC run happens, the
//! fixtures in the tests are what should be replaced with real bodies.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::http::HttpClient;
use crate::{Error, Paths, Result};

/// Where the owned-products list comes from.
pub const PRODUCTS_ENDPOINT: &str = "https://embed.gog.com/account/getFilteredProducts";

/// The identifier every other part of gamestore keys a game on.
///
/// GOG's product id, kept as a string: it is numeric today, and a newtype around a
/// string costs nothing while keeping it from being confused with the Steam appid,
/// which is a different number for the same game and is derived, not owned.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProductId(String);

impl ProductId {
    /// Wrap an id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProductId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProductId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// The operating systems a product ships builds for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

/// One owned game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// The key `state`, `art` and the Ludusavi mapping all use.
    pub id: ProductId,
    /// Title as GOG spells it, which is what the tile shows.
    pub title: String,
    /// URL-safe name, useful for the Ludusavi mapping and for support links.
    #[serde(default)]
    pub slug: String,
    /// Cover image URL as GOG gives it, if any.
    #[serde(default)]
    pub image: Option<String>,
    /// Which platforms GOG has builds for, sorted.
    #[serde(default)]
    pub runs_on: Vec<Os>,
}

impl Game {
    /// Whether GOG ships a Windows build, which is the one installed under Proton.
    pub fn has_windows_build(&self) -> bool {
        self.runs_on.contains(&Os::Windows)
    }
}

/// The owned library, as last fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// When this was fetched, so the UI can say how stale it is.
    pub fetched_at: SystemTime,
    /// Owned games, sorted by title so the grid order does not depend on GOG's.
    pub games: Vec<Game>,
}

impl Catalog {
    /// An empty catalog, as if it had just been fetched.
    pub fn empty(now: SystemTime) -> Self {
        Self {
            fetched_at: now,
            games: Vec::new(),
        }
    }

    /// How many games are owned.
    pub fn len(&self) -> usize {
        self.games.len()
    }

    /// Whether the library is empty.
    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    /// Look a game up by the id everything else keys on.
    pub fn get(&self, id: &ProductId) -> Option<&Game> {
        self.games.iter().find(|game| &game.id == id)
    }

    /// Games whose title contains `needle`, ignoring case and accents-as-written.
    ///
    /// This is what the grid's search box calls; it is deliberately a plain
    /// substring match, since the on-screen keyboard makes anything more elaborate
    /// slower to drive rather than faster.
    pub fn search(&self, needle: &str) -> Vec<&Game> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return self.games.iter().collect();
        }

        self.games
            .iter()
            .filter(|game| game.title.to_lowercase().contains(&needle))
            .collect()
    }

    /// Where the cached copy lives.
    pub fn cache_file(paths: &Paths) -> std::path::PathBuf {
        paths.cache_dir.join("catalog.json")
    }

    /// Read the cached catalog, or `None` when there is none.
    ///
    /// A cache that cannot be parsed is treated as absent rather than as an error:
    /// it is derived data, and refusing to start because a throwaway file is stale
    /// would be the wrong trade.
    pub fn load(paths: &Paths) -> Result<Option<Self>> {
        let path = Self::cache_file(paths);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(serde_json::from_str(&text).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::io(format!("reading {}", path.display()), error)),
        }
    }

    /// Replace the cached catalog.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|error| {
            Error::Catalog(format!("the catalog could not be written ({error})"))
        })?;

        crate::atomic::write(
            &Self::cache_file(paths),
            text.as_bytes(),
            crate::atomic::Access::Default,
        )
    }
}

/// Fetch the whole owned library, following GOG's pagination.
///
/// `access_token` is a GOG access token; [`crate::tokens::Session::access_token`]
/// is what produces one, refreshing it first if it is due.
pub fn fetch<H: HttpClient>(http: &H, access_token: &str, now: SystemTime) -> Result<Catalog> {
    let mut games: BTreeMap<ProductId, Game> = BTreeMap::new();
    let mut page = 1_u32;

    loop {
        // `mediaType=1` is GOG's filter for games, as opposed to films.
        let url = format!("{PRODUCTS_ENDPOINT}?mediaType=1&page={page}");
        let response = http.get_authorized(&url, Some(access_token))?;

        if !response.is_success() {
            return Err(Error::Catalog(format!(
                "GOG refused the library request with status {} ({})",
                response.status,
                first_line(&response.body)
            )));
        }

        let parsed: ProductsPage = serde_json::from_str(&response.body).map_err(|error| {
            Error::Catalog(format!(
                "the library page could not be read ({error}); GOG may have changed \
                 the shape of this response"
            ))
        })?;

        for product in parsed.products {
            if let Some(game) = product.into_game() {
                games.insert(game.id.clone(), game);
            }
        }

        let total = parsed.total_pages.unwrap_or(1).max(1);
        if page >= total {
            break;
        }
        page += 1;
    }

    let mut games: Vec<Game> = games.into_values().collect();
    games.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(Catalog {
        fetched_at: now,
        games,
    })
}

/// Read the cache, fetching when there is none.
pub fn load_or_fetch<H: HttpClient>(
    paths: &Paths,
    http: &H,
    access_token: &str,
    now: SystemTime,
) -> Result<Catalog> {
    if let Some(cached) = Catalog::load(paths)? {
        return Ok(cached);
    }

    let catalog = fetch(http, access_token, now)?;
    catalog.save(paths)?;

    Ok(catalog)
}

/// One page of `getFilteredProducts`.
#[derive(Debug, Deserialize)]
struct ProductsPage {
    #[serde(default)]
    products: Vec<Product>,
    #[serde(rename = "totalPages")]
    total_pages: Option<u32>,
}

/// A product as that endpoint spells it. Unknown fields are ignored on purpose:
/// GOG adds them, and a `deny_unknown_fields` here would turn every addition on
/// their side into a broken library on ours.
#[derive(Debug, Deserialize)]
struct Product {
    id: Option<serde_json::Value>,
    title: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(rename = "worksOn", default)]
    works_on: BTreeMap<String, bool>,
}

impl Product {
    /// Convert, or skip: a product with no id or no title is not something the grid
    /// could draw or the installer could act on, and one unusable row should not
    /// cost the user the rest of their library.
    fn into_game(self) -> Option<Game> {
        let id = match self.id? {
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::String(text) => text,
            _ => return None,
        };
        let title = self.title.filter(|title| !title.trim().is_empty())?;

        let mut runs_on: Vec<Os> = self
            .works_on
            .iter()
            .filter(|(_, supported)| **supported)
            .filter_map(|(name, _)| match name.to_lowercase().as_str() {
                "windows" => Some(Os::Windows),
                "mac" => Some(Os::Mac),
                "linux" => Some(Os::Linux),
                _ => None,
            })
            .collect();
        runs_on.sort();

        Some(Game {
            id: ProductId::new(id),
            title,
            slug: self.slug.unwrap_or_default(),
            image: self.image.filter(|image| !image.is_empty()),
            runs_on,
        })
    }
}

fn first_line(body: &str) -> String {
    body.lines().next().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;
    use std::cell::RefCell;

    struct FakePages {
        pages: RefCell<Vec<Response>>,
        seen: RefCell<Vec<(String, Option<String>)>>,
    }

    impl FakePages {
        fn new(bodies: &[(u16, &str)]) -> Self {
            Self {
                pages: RefCell::new(
                    bodies
                        .iter()
                        .rev()
                        .map(|(status, body)| Response {
                            status: *status,
                            body: (*body).to_string(),
                        })
                        .collect(),
                ),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl HttpClient for FakePages {
        fn get_authorized(&self, url: &str, bearer: Option<&str>) -> Result<Response> {
            self.seen
                .borrow_mut()
                .push((url.to_string(), bearer.map(str::to_string)));

            self.pages.borrow_mut().pop().ok_or_else(|| Error::Http {
                url: url.to_string(),
                reason: "the test ran out of pages".to_string(),
            })
        }
    }

    const T0: SystemTime = SystemTime::UNIX_EPOCH;

    fn page(products: &str, total: u32) -> String {
        format!(r#"{{"products":[{products}],"totalPages":{total}}}"#)
    }

    const BASTION: &str = r#"{"id":1207658930,"title":"Bastion","slug":"bastion",
        "image":"//images.gog.com/abc","worksOn":{"Windows":true,"Mac":true,"Linux":false}}"#;
    const GOTHIC: &str = r#"{"id":1207658697,"title":"Gothic II","slug":"gothic_ii",
        "worksOn":{"Windows":true,"Mac":false,"Linux":false}}"#;

    fn paths_in(dir: &std::path::Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        }
    }

    #[test]
    fn a_single_page_library_is_read() {
        let http = FakePages::new(&[(200, &page(BASTION, 1))]);

        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(catalog.len(), 1);
        let game = &catalog.games[0];
        assert_eq!(game.id, ProductId::new("1207658930"));
        assert_eq!(game.title, "Bastion");
        assert_eq!(game.slug, "bastion");
        assert_eq!(game.image.as_deref(), Some("//images.gog.com/abc"));
        assert_eq!(game.runs_on, vec![Os::Windows, Os::Mac]);
        assert!(game.has_windows_build());
    }

    #[test]
    fn the_access_token_is_sent_as_a_bearer_on_every_page() {
        let http = FakePages::new(&[(200, &page(BASTION, 2)), (200, &page(GOTHIC, 2))]);

        fetch(&http, "a-token", T0).unwrap();

        let seen = http.seen.borrow();
        assert_eq!(seen.len(), 2);
        for (url, bearer) in seen.iter() {
            assert_eq!(
                bearer.as_deref(),
                Some("a-token"),
                "an unauthenticated page request would look like an empty library"
            );
            assert!(url.contains("mediaType=1"), "{url}");
        }
        assert!(seen[0].0.contains("page=1"));
        assert!(seen[1].0.contains("page=2"));
    }

    #[test]
    fn every_page_is_followed_and_the_result_is_sorted_by_title() {
        let http = FakePages::new(&[(200, &page(GOTHIC, 2)), (200, &page(BASTION, 2))]);

        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(
            catalog
                .games
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Bastion", "Gothic II"],
            "the grid order must not depend on the order GOG paginates in"
        );
    }

    #[test]
    fn a_game_repeated_across_pages_is_only_listed_once() {
        let http = FakePages::new(&[(200, &page(BASTION, 2)), (200, &page(BASTION, 2))]);

        assert_eq!(fetch(&http, "a-token", T0).unwrap().len(), 1);
    }

    #[test]
    fn a_product_that_cannot_be_read_is_skipped_not_fatal() {
        // One unusable row must not cost the user the rest of the library.
        let broken = r#"{"title":"No Id At All"}"#;
        let http = FakePages::new(&[(200, &page(&format!("{broken},{BASTION}"), 1))]);

        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.games[0].title, "Bastion");
    }

    #[test]
    fn unknown_fields_do_not_break_the_parse() {
        let with_extra = r#"{"id":1,"title":"Future Game","somethingGogAddedLater":{"a":1},
            "worksOn":{"Windows":true,"Xbox":true}}"#;
        let http = FakePages::new(&[(200, &page(with_extra, 1))]);

        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog.games[0].runs_on,
            vec![Os::Windows],
            "a platform we do not know about is ignored, not an error"
        );
    }

    #[test]
    fn a_string_id_is_accepted_as_well_as_a_number() {
        let http = FakePages::new(&[(200, &page(r#"{"id":"1207658930","title":"Bastion"}"#, 1))]);

        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(catalog.games[0].id, ProductId::new("1207658930"));
    }

    #[test]
    fn a_refused_request_says_what_gog_answered() {
        let http = FakePages::new(&[(401, r#"{"error":"invalid_token"}"#)]);

        let error = fetch(&http, "stale", T0).unwrap_err();

        assert!(error.to_string().contains("401"), "{error}");
        assert!(error.to_string().contains("invalid_token"), "{error}");
    }

    #[test]
    fn a_response_that_is_not_the_expected_shape_says_so() {
        let http = FakePages::new(&[(200, "<html>maintenance</html>")]);

        let error = fetch(&http, "a-token", T0).unwrap_err();

        assert!(error.to_string().contains("changed the shape"), "{error}");
    }

    #[test]
    fn the_catalog_round_trips_through_the_cache() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let http = FakePages::new(&[(200, &page(BASTION, 1))]);
        let catalog = fetch(&http, "a-token", T0).unwrap();

        catalog.save(&paths).unwrap();

        assert_eq!(Catalog::load(&paths).unwrap(), Some(catalog));
    }

    #[test]
    fn no_cache_is_none_and_a_corrupt_cache_is_treated_as_none() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());

        assert_eq!(Catalog::load(&paths).unwrap(), None);

        std::fs::create_dir_all(&paths.cache_dir).unwrap();
        std::fs::write(Catalog::cache_file(&paths), "not json").unwrap();
        assert_eq!(
            Catalog::load(&paths).unwrap(),
            None,
            "derived data that cannot be read is refetched, not fatal"
        );
    }

    #[test]
    fn load_or_fetch_uses_the_cache_and_does_not_call_gog_twice() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let http = FakePages::new(&[(200, &page(BASTION, 1))]);

        let first = load_or_fetch(&paths, &http, "a-token", T0).unwrap();
        let second = load_or_fetch(&paths, &http, "a-token", T0).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            http.seen.borrow().len(),
            1,
            "the second call has to come from the cache; the fake has no second page"
        );
    }

    #[test]
    fn search_is_a_case_insensitive_substring_and_empty_matches_everything() {
        let http = FakePages::new(&[(200, &page(&format!("{BASTION},{GOTHIC}"), 1))]);
        let catalog = fetch(&http, "a-token", T0).unwrap();

        assert_eq!(catalog.search("got").len(), 1);
        assert_eq!(catalog.search("GOTHIC").len(), 1);
        assert_eq!(catalog.search("  ").len(), 2);
        assert_eq!(catalog.search("nothing").len(), 0);
    }

    #[test]
    fn a_game_can_be_found_by_the_id_everything_else_keys_on() {
        let http = FakePages::new(&[(200, &page(BASTION, 1))]);
        let catalog = fetch(&http, "a-token", T0).unwrap();

        let found = catalog.get(&ProductId::new("1207658930")).unwrap();

        assert_eq!(found.title, "Bastion");
        assert_eq!(catalog.get(&ProductId::new("nope")), None);
    }
}
