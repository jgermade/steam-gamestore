//! Steam library artwork for an installed game: `art`, with `art2` folded in.
//!
//! Steam reads custom artwork for a shortcut from `userdata/<user>/config/grid/`,
//! named after the shortcut's 32-bit appid: `<appid>p` is the portrait capsule,
//! `<appid>` the wide one, `<appid>_hero` the banner behind the game page, and
//! `<appid>_logo` the logo drawn over it. Checked on the mini PC (2026-10-06).
//!
//! SteamGridDB comes first, because what GOG publishes was checked image by
//! image and found wanting: the vertical cover is 342×482 at most, and the
//! "logo" is the wide capsule again, with no transparency. SteamGridDB looks a
//! game up by its GOG id, so there is no title to mismatch. GOG is the fallback
//! when there is no SteamGridDB key or no match, and only its catalog entry whose
//! id is the game's own is used: a catalog query once came back with another game
//! entirely.

use std::path::{Path, PathBuf};

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::catalog::ProductId;
use crate::http::{Fetch, HttpClient};
use crate::{Error, Paths, Result};

const STEAMGRIDDB: &str = "https://www.steamgriddb.com/api/v2";
const GOG_CATALOG: &str = "https://catalog.gog.com/v1/catalog";

/// Static images only, nothing explicit and nothing meant as a joke: this is a
/// grid the kids look at.
const FILTERS: &str = "types=static&nsfw=false&humor=false";

/// The four images Steam shows for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Portrait,
    Wide,
    Hero,
    Logo,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Portrait, Kind::Wide, Kind::Hero, Kind::Logo];

    /// The kind's name where picked images are remembered.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Portrait => "portrait",
            Kind::Wide => "wide",
            Kind::Hero => "hero",
            Kind::Logo => "logo",
        }
    }

    /// What follows the appid in the file name.
    pub fn suffix(self) -> &'static str {
        match self {
            Kind::Portrait => "p",
            Kind::Wide => "",
            Kind::Hero => "_hero",
            Kind::Logo => "_logo",
        }
    }
}

/// One image to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Art {
    pub kind: Kind,
    pub url: String,
}

impl Art {
    /// The name Steam looks for, keeping the image's own extension.
    pub fn file_name(&self, appid: u32) -> String {
        let path = self.url.split(['?', '#']).next().unwrap_or_default();
        let extension = path
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .filter(|extension| matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp"))
            .unwrap_or_else(|| "png".to_string());
        format!("{appid}{}.{extension}", self.kind.suffix())
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    data: Option<T>,
}

#[derive(Deserialize)]
struct SgdbGame {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct SgdbImage {
    url: String,
    #[serde(default)]
    thumb: String,
}

fn sgdb<H: HttpClient, T: serde::de::DeserializeOwned>(
    http: &H,
    key: &str,
    path: &str,
) -> Result<Option<T>> {
    let response = http.get_authorized(&format!("{STEAMGRIDDB}{path}"), Some(key))?;
    if response.status == 404 {
        return Ok(None);
    }
    if response.status == 401 || response.status == 403 {
        return Err(Error::Artwork(
            "SteamGridDB refused the API key; check [steamgriddb] api_key in config.toml"
                .to_string(),
        ));
    }
    if !response.is_success() {
        return Err(Error::Artwork(format!(
            "SteamGridDB answered {} for {path}",
            response.status
        )));
    }
    let envelope: Envelope<T> = serde_json::from_str(&response.body).map_err(|error| {
        Error::Artwork(format!("SteamGridDB's answer could not be read ({error})"))
    })?;
    Ok(envelope.data.filter(|_| envelope.success))
}

/// The SteamGridDB game for a GOG product: by GOG id, else by exact title, else
/// the first title match.
fn sgdb_game<H: HttpClient>(
    http: &H,
    key: &str,
    id: &ProductId,
    title: &str,
) -> Result<Option<u64>> {
    if let Some(game) = sgdb::<_, SgdbGame>(http, key, &format!("/games/gog/{id}"))? {
        return Ok(Some(game.id));
    }
    let search = utf8_percent_encode(title, NON_ALPHANUMERIC).to_string();
    let found: Vec<SgdbGame> =
        sgdb(http, key, &format!("/search/autocomplete/{search}"))?.unwrap_or_default();
    Ok(found
        .iter()
        .find(|game| game.name.eq_ignore_ascii_case(title))
        .or(found.first())
        .map(|game| game.id))
}

/// The SteamGridDB query for one kind, in the sizes Steam uses.
fn kind_path(game: u64, kind: Kind) -> String {
    match kind {
        Kind::Portrait => format!("/grids/game/{game}?dimensions=600x900&{FILTERS}"),
        Kind::Wide => format!("/grids/game/{game}?dimensions=920x430,460x215&{FILTERS}"),
        Kind::Hero => format!("/heroes/game/{game}?{FILTERS}"),
        Kind::Logo => format!("/logos/game/{game}?{FILTERS}"),
    }
}

/// One image to choose from: the image itself and a small preview of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub url: String,
    pub thumb: String,
}

/// Every image of `kind` SteamGridDB has for the game, best rated first, at most
/// `limit` of them.
pub fn choices<H: HttpClient>(
    http: &H,
    key: &str,
    id: &ProductId,
    title: &str,
    kind: Kind,
    limit: usize,
) -> Result<Vec<Choice>> {
    let Some(game) = sgdb_game(http, key, id, title)? else {
        return Ok(Vec::new());
    };
    let images: Vec<SgdbImage> = sgdb(http, key, &kind_path(game, kind))?.unwrap_or_default();
    Ok(images
        .into_iter()
        .take(limit)
        .map(|image| Choice {
            thumb: if image.thumb.is_empty() {
                image.url.clone()
            } else {
                image.thumb
            },
            url: image.url,
        })
        .collect())
}

/// The best-rated image of each kind on SteamGridDB, in the sizes Steam uses.
pub fn steamgriddb<H: HttpClient>(
    http: &H,
    key: &str,
    id: &ProductId,
    title: &str,
) -> Result<Vec<Art>> {
    let Some(game) = sgdb_game(http, key, id, title)? else {
        return Ok(Vec::new());
    };
    let mut arts = Vec::new();
    for kind in Kind::ALL {
        let images: Vec<SgdbImage> = sgdb(http, key, &kind_path(game, kind))?.unwrap_or_default();
        if let Some(image) = images.into_iter().next() {
            arts.push(Art {
                kind,
                url: image.url,
            });
        }
    }
    Ok(arts)
}

#[derive(Deserialize)]
struct CatalogPage {
    #[serde(default)]
    products: Vec<CatalogProduct>,
}

#[derive(Deserialize)]
struct CatalogProduct {
    id: serde_json::Value,
    #[serde(default, rename = "coverHorizontal")]
    cover_horizontal: Option<String>,
    #[serde(default, rename = "coverVertical")]
    cover_vertical: Option<String>,
}

/// What GOG's store catalog has: a wide capsule that is the right shape, and a
/// small portrait. Only the entry whose id is `id` counts.
pub fn gog<H: HttpClient>(http: &H, id: &ProductId, title: &str) -> Result<Vec<Art>> {
    let query = utf8_percent_encode(title, NON_ALPHANUMERIC).to_string();
    let response = http.get(&format!("{GOG_CATALOG}?query=like:{query}&limit=48"))?;
    if !response.is_success() {
        return Ok(Vec::new());
    }
    let page: CatalogPage = serde_json::from_str(&response.body)
        .map_err(|error| Error::Artwork(format!("GOG's catalog could not be read ({error})")))?;
    let Some(product) = page.products.into_iter().find(|product| {
        let found = match &product.id {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        found == id.as_str()
    }) else {
        return Ok(Vec::new());
    };
    Ok([
        (Kind::Wide, product.cover_horizontal),
        (Kind::Portrait, product.cover_vertical),
    ]
    .into_iter()
    .filter_map(|(kind, url)| {
        url.filter(|url| !url.is_empty())
            .map(|url| Art { kind, url })
    })
    .collect())
}

/// Where the images someone picked for a game are remembered, so reinstalling it
/// keeps them instead of going back to SteamGridDB's favourites.
fn chosen_file(paths: &Paths, id: &ProductId) -> PathBuf {
    paths.data_dir.join("artwork").join(format!("{id}.json"))
}

/// The images picked for a game, one per kind at most.
pub fn chosen(paths: &Paths, id: &ProductId) -> Vec<Art> {
    let Ok(text) = std::fs::read_to_string(chosen_file(paths, id)) else {
        return Vec::new();
    };
    let picked: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&text).unwrap_or_default();
    Kind::ALL
        .into_iter()
        .filter_map(|kind| {
            picked.get(kind.name()).map(|url| Art {
                kind,
                url: url.clone(),
            })
        })
        .collect()
}

/// Remember `art` as the game's image of its kind.
pub fn remember(paths: &Paths, id: &ProductId, art: &Art) -> Result<()> {
    let mut picked: std::collections::BTreeMap<String, String> = chosen(paths, id)
        .into_iter()
        .map(|art| (art.kind.name().to_string(), art.url))
        .collect();
    picked.insert(art.kind.name().to_string(), art.url.clone());
    let file = chosen_file(paths, id);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;
    }
    let text = serde_json::to_string_pretty(&picked).expect("a map of strings serialises");
    crate::atomic::write(&file, text.as_bytes(), crate::atomic::Access::Default)
}

/// Download each image once and write it into every grid directory, replacing
/// any earlier image of the same kind whatever its extension. Directories that are
/// the same one through a symlink are written once. Returns the files written.
pub fn save<F: Fetch>(
    fetch: &F,
    arts: &[Art],
    appid: u32,
    grid_dirs: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in grid_dirs {
        std::fs::create_dir_all(dir)
            .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;
        let real = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !dirs.contains(&real) {
            dirs.push(real);
        }
    }

    let mut written = Vec::new();
    for art in arts {
        let (status, mut body) = fetch.open(&art.url, 0)?;
        if status != 200 {
            return Err(Error::Artwork(format!(
                "the image server answered {status}"
            )));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut body, &mut bytes)
            .map_err(|error| Error::io("downloading artwork", error))?;

        let name = art.file_name(appid);
        for dir in &dirs {
            remove_kind(dir, appid, art.kind);
            let path = dir.join(&name);
            crate::atomic::write(&path, &bytes, crate::atomic::Access::Default)?;
            written.push(path);
        }
    }
    Ok(written)
}

/// Remove the images of one kind for `appid`, in any format. A `.jpg` left next
/// to a new `.png` would leave Steam to pick between them.
pub fn remove_kind(dir: &Path, appid: u32, kind: Kind) {
    for extension in ["png", "jpg", "jpeg", "webp"] {
        let _ = std::fs::remove_file(dir.join(format!("{appid}{}.{extension}", kind.suffix())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;
    use std::collections::HashMap;

    /// Real SteamGridDB answers for Xeno Crisis (2026-10-06), cut down to one item.
    const BY_GOG_ID: &str = r#"{"success": true, "data": {"id": 5249018, "name": "Xeno Crisis", "release_date": 1572256150, "types": ["steam", "gog"], "verified": true}}"#;
    const PORTRAITS: &str = r#"{"success": true, "page": 0, "total": 26, "limit": 50, "data": [{"id": 132555, "score": 0, "style": "alternate", "width": 600, "height": 900, "nsfw": false, "humor": false, "notes": null, "mime": "image/png", "language": "en", "url": "https://cdn2.steamgriddb.com/grid/3d7c240be8fba05e7e1c40d52c4eb4be.png", "thumb": "https://cdn2.steamgriddb.com/thumb/3d7c240be8fba05e7e1c40d52c4eb4be.jpg", "lock": false, "epilepsy": false, "upvotes": 0, "downvotes": 0, "author": {"name": "Julgamesh"}}]}"#;
    const SEARCH: &str = r#"{"success": true, "data": [{"id": 5448357, "name": "Xeno", "verified": true, "types": ["steam"]}, {"id": 5249018, "name": "Xeno Crisis", "verified": true, "types": ["steam", "gog"]}]}"#;
    const EMPTY: &str = r#"{"success": true, "data": []}"#;

    /// Answers by URL prefix; anything unlisted is a 404.
    struct Fake(HashMap<&'static str, (u16, &'static str)>);

    impl HttpClient for Fake {
        fn get_authorized(&self, url: &str, _: Option<&str>) -> Result<Response> {
            let found = self
                .0
                .iter()
                .filter(|(prefix, _)| url.contains(*prefix))
                .max_by_key(|(prefix, _)| prefix.len());
            let (status, body) = found.map(|(_, answer)| *answer).unwrap_or((404, ""));
            Ok(Response {
                status,
                body: body.to_string(),
            })
        }
    }

    #[test]
    fn the_game_is_found_by_its_gog_id_and_each_kind_is_asked_for() {
        let http = Fake(HashMap::from([
            ("/games/gog/1955095293", (200, BY_GOG_ID)),
            ("/grids/game/5249018?dimensions=600x900", (200, PORTRAITS)),
            ("/grids/game/5249018?dimensions=920x430", (200, EMPTY)),
            ("/heroes/game/5249018", (200, EMPTY)),
            ("/logos/game/5249018", (200, EMPTY)),
        ]));

        let arts = steamgriddb(&http, "key", &"1955095293".into(), "Xeno Crisis").unwrap();

        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].kind, Kind::Portrait);
        assert_eq!(arts[0].file_name(2907567421), "2907567421p.png");
    }

    #[test]
    fn without_a_gog_match_the_exact_title_beats_the_first_result() {
        let http = Fake(HashMap::from([
            ("/search/autocomplete/Xeno%20Crisis", (200, SEARCH)),
            ("/grids/game/5249018?dimensions=600x900", (200, PORTRAITS)),
        ]));

        let arts = steamgriddb(&http, "key", &"1".into(), "Xeno Crisis").unwrap();

        assert_eq!(
            arts.len(),
            1,
            "only the exact title's id has a portrait here"
        );
    }

    #[test]
    fn a_refused_key_says_where_the_key_lives() {
        let http = Fake(HashMap::from([("/games/gog/", (401, ""))]));

        let error = steamgriddb(&http, "bad", &"1".into(), "x").unwrap_err();

        assert!(error.to_string().contains("api_key"), "{error}");
    }

    #[test]
    fn gogs_catalog_is_used_only_for_the_entry_with_the_right_id() {
        // What the catalog really returned once for a filtered query: another game.
        let wrong = r#"{"products":[{"id":"1207659222","title":"The Saboteur","coverHorizontal":"https://images.gog-statics.com/9a75.png","coverVertical":"https://images.gog-statics.com/73e1.jpg"}]}"#;
        let right = r#"{"products":[{"id":"1955095293","title":"Xeno Crisis","coverHorizontal":"https://images.gog-statics.com/3c4b.png","coverVertical":"https://images.gog-statics.com/eaa7.jpg"}]}"#;

        let http = Fake(HashMap::from([("catalog.gog.com", (200, wrong))]));
        assert!(
            gog(&http, &"1955095293".into(), "Xeno Crisis")
                .unwrap()
                .is_empty()
        );

        let http = Fake(HashMap::from([("catalog.gog.com", (200, right))]));
        let arts = gog(&http, &"1955095293".into(), "Xeno Crisis").unwrap();
        assert_eq!(arts[0].file_name(7), "7.png");
        assert_eq!(arts[1].file_name(7), "7p.jpg");
    }

    #[test]
    fn choices_come_with_their_previews_and_picks_are_remembered() {
        let http = Fake(HashMap::from([
            ("/games/gog/1955095293", (200, BY_GOG_ID)),
            ("/grids/game/5249018?dimensions=600x900", (200, PORTRAITS)),
        ]));
        let found = choices(
            &http,
            "key",
            &"1955095293".into(),
            "Xeno Crisis",
            Kind::Portrait,
            10,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].thumb.contains("/thumb/"));

        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: temp.path().join("c"),
            data_dir: temp.path().join("d"),
            cache_dir: temp.path().join("k"),
        };
        let id: ProductId = "7".into();
        assert!(chosen(&paths, &id).is_empty());
        for url in ["https://a/1.png", "https://a/2.png"] {
            let art = Art {
                kind: Kind::Portrait,
                url: url.to_string(),
            };
            remember(&paths, &id, &art).unwrap();
        }
        let hero = Art {
            kind: Kind::Hero,
            url: "https://a/h.jpg".to_string(),
        };
        remember(&paths, &id, &hero).unwrap();
        let picked = chosen(&paths, &id);
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].url, "https://a/2.png");
        assert_eq!(picked[1], hero);
    }

    struct Images;

    impl Fetch for Images {
        fn open(&self, url: &str, _: u64) -> Result<(u16, Box<dyn std::io::Read>)> {
            Ok((200, Box::new(std::io::Cursor::new(url.as_bytes().to_vec()))))
        }
    }

    #[test]
    fn saving_replaces_other_formats_and_writes_a_shared_dir_once() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("a/grid");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(temp.path().join("b")).unwrap();
        std::os::unix::fs::symlink(&shared, temp.path().join("b/grid")).unwrap();
        std::fs::write(shared.join("7p.jpg"), "old").unwrap();

        let arts = [Art {
            kind: Kind::Portrait,
            url: "https://cdn/x.png".to_string(),
        }];
        let written = save(
            &Images,
            &arts,
            7,
            &[shared.clone(), temp.path().join("b/grid")],
        )
        .unwrap();

        assert_eq!(written.len(), 1);
        assert!(!shared.join("7p.jpg").exists());
        assert_eq!(
            std::fs::read_to_string(shared.join("7p.png")).unwrap(),
            "https://cdn/x.png"
        );
    }
}
