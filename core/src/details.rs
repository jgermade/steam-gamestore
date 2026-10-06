//! What a game's detail view shows: download sizes, screenshots and videos.
//!
//! All of it comes from GOG's product endpoint (`expand=downloads,screenshots,
//! videos`), and is cached as `<cache>/details/<id>.json`, so a detail view opens
//! at once the second time and offline. Screenshots are GOG's own 1920×1080
//! renditions with a smaller one for thumbnails; videos are the YouTube pages GOG
//! links to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::{Os, ProductId};
use crate::http::HttpClient;
use crate::installer::PRODUCT_ENDPOINT;
use crate::{Error, Paths, Result};

/// One screenshot, full size and thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    pub full: String,
    pub thumbnail: String,
}

/// What the detail view needs about a game.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Details {
    /// Download size of each build, in bytes, for the English installer or the
    /// first one GOG lists.
    pub sizes: Vec<(Os, u64)>,
    pub screenshots: Vec<Screenshot>,
    /// YouTube watch addresses.
    pub videos: Vec<String>,
}

impl Details {
    pub fn size(&self, os: Os) -> Option<u64> {
        self.sizes
            .iter()
            .find(|(build, _)| *build == os)
            .map(|(_, size)| *size)
    }
}

#[derive(Deserialize)]
struct Product {
    #[serde(default)]
    downloads: Option<Downloads>,
    #[serde(default)]
    screenshots: Vec<RawScreenshot>,
    #[serde(default)]
    videos: Vec<RawVideo>,
}

#[derive(Deserialize)]
struct Downloads {
    #[serde(default)]
    installers: Vec<RawInstaller>,
}

#[derive(Deserialize)]
struct RawInstaller {
    os: String,
    language: String,
    #[serde(default)]
    total_size: u64,
}

#[derive(Deserialize)]
struct RawScreenshot {
    #[serde(default)]
    formatted_images: Vec<FormattedImage>,
}

#[derive(Deserialize)]
struct FormattedImage {
    formatter_name: String,
    image_url: String,
}

#[derive(Deserialize)]
struct RawVideo {
    video_url: String,
    #[serde(default)]
    provider: Option<String>,
}

/// Where a game's details are cached.
pub fn cache_file(paths: &Paths, id: &ProductId) -> PathBuf {
    paths.cache_dir.join("details").join(format!("{id}.json"))
}

/// The cached details, if there are any.
pub fn cached(paths: &Paths, id: &ProductId) -> Option<Details> {
    serde_json::from_slice(&std::fs::read(cache_file(paths, id)).ok()?).ok()
}

/// Fetch a game's details from GOG and cache them.
pub fn fetch<H: HttpClient>(
    http: &H,
    access_token: &str,
    paths: &Paths,
    id: &ProductId,
) -> Result<Details> {
    let url = format!("{PRODUCT_ENDPOINT}/{id}?expand=downloads,screenshots,videos");
    let response = http.get_authorized(&url, Some(access_token))?;
    if !response.is_success() {
        return Err(Error::Catalog(format!(
            "GOG refused the details of {id} with status {}",
            response.status
        )));
    }
    let details = parse(&response.body)?;
    let file = cache_file(paths, id);
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec(&details) {
        let _ = crate::atomic::write(&file, &json, crate::atomic::Access::Default);
    }
    Ok(details)
}

/// Read the product endpoint's answer.
pub fn parse(body: &str) -> Result<Details> {
    let product: Product = serde_json::from_str(body)
        .map_err(|error| Error::Catalog(format!("a game's details could not be read ({error})")))?;

    let installers = product.downloads.map(|d| d.installers).unwrap_or_default();
    let mut sizes = Vec::new();
    for (os, name) in [(Os::Linux, "linux"), (Os::Windows, "windows")] {
        let of_os: Vec<_> = installers.iter().filter(|i| i.os == name).collect();
        let chosen = of_os
            .iter()
            .find(|installer| installer.language == "en")
            .or(of_os.first());
        if let Some(installer) = chosen {
            sizes.push((os, installer.total_size));
        }
    }

    let screenshots = product
        .screenshots
        .into_iter()
        .filter_map(|shot| {
            let named = |name: &str| {
                shot.formatted_images
                    .iter()
                    .find(|image| image.formatter_name == name)
                    .map(|image| image.image_url.clone())
            };
            Some(Screenshot {
                full: named("ggvgl")?,
                thumbnail: named("ggvgm").or_else(|| named("ggvgl"))?,
            })
        })
        .collect();

    let videos = product
        .videos
        .into_iter()
        .filter(|video| video.provider.as_deref().unwrap_or("youtube") == "youtube")
        .filter_map(|video| youtube_watch(&video.video_url))
        .collect();

    Ok(Details {
        sizes,
        screenshots,
        videos,
    })
}

/// `https://www.youtube.com/embed/<id>?…` as a watch address a player can open.
fn youtube_watch(embed: &str) -> Option<String> {
    let id = embed
        .split("/embed/")
        .nth(1)?
        .split(['?', '&', '/'])
        .next()?;
    (!id.is_empty()).then(|| format!("https://www.youtube.com/watch?v={id}"))
}

/// How much space a directory takes, following nothing.
pub fn disk_usage(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => disk_usage(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// How the disk gamestore installs to is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Storage {
    pub total: u64,
    pub free: u64,
    /// Steam's libraries on that disk: its games, and the Proton prefixes,
    /// gamestore's included.
    pub steam: u64,
    /// gamestore's games and the installers it keeps.
    pub gog: u64,
    /// The system's own filesystem, `(total, free)`, when it is another one than
    /// the games': then the system's share is there, not in [`Storage::other`].
    pub system: Option<(u64, u64)>,
}

impl Storage {
    /// What is neither Steam nor GOG: the system, and everything else.
    pub fn other(&self) -> u64 {
        self.total
            .saturating_sub(self.free)
            .saturating_sub(self.steam)
            .saturating_sub(self.gog)
    }
}

/// Size and free space of the filesystem holding `path`, from `df`.
pub fn filesystem(path: &Path) -> Option<(u64, u64)> {
    let output = std::process::Command::new("df")
        .args(["-B1", "--output=size,avail"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut numbers = text.lines().nth(1)?.split_whitespace();
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

/// Measure the disk `install_root` is on. Only what is on that same disk
/// counts: a Steam library elsewhere is not part of this picture. Walking big
/// libraries takes seconds, so this belongs off the UI thread.
pub fn storage(
    install_root: &Path,
    cache_dir: &Path,
    steam_libraries: &[PathBuf],
) -> Option<Storage> {
    use std::os::unix::fs::MetadataExt;
    let anchor = std::iter::successors(Some(install_root), |path| path.parent())
        .find(|path| path.exists())?;
    let device = std::fs::metadata(anchor).ok()?.dev();
    let same_disk = |path: &Path| std::fs::metadata(path).is_ok_and(|m| m.dev() == device);
    let (total, free) = filesystem(anchor)?;

    let mut seen: Vec<PathBuf> = Vec::new();
    let mut steam = 0;
    for library in steam_libraries {
        let apps = library.join("steamapps");
        let real = apps.canonicalize().unwrap_or(apps.clone());
        if same_disk(&real) && !seen.contains(&real) {
            steam += disk_usage(&real);
            seen.push(real);
        }
    }
    let system = std::fs::metadata("/")
        .ok()
        .filter(|root| root.dev() != device)
        .and_then(|_| filesystem(Path::new("/")));
    let gog = [install_root.to_path_buf(), cache_dir.join("downloads")]
        .iter()
        .filter(|path| same_disk(path))
        .map(|path| disk_usage(path))
        .sum();
    Some(Storage {
        total,
        free,
        steam,
        gog,
        system,
    })
}

/// A size as people read it: `161 MB`, `4.3 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 || unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real answer for Xeno Crisis (2026-10-06), cut down to one screenshot.
    const XENO_CRISIS: &str = r#"{"id":1955095293,"title":"Xeno Crisis",
        "downloads":{"installers":[
            {"os":"windows","language":"en","total_size":168820736},
            {"os":"mac","language":"en","total_size":179306496},
            {"os":"linux","language":"en","total_size":178257920}]},
        "screenshots":[{"image_id":"7033","formatter_template_url":"https://images-3.gog-statics.com/7033_{formatter}.png",
            "formatted_images":[
                {"formatter_name":"ggvgt","image_url":"https://images-3.gog-statics.com/7033_ggvgt.jpg"},
                {"formatter_name":"ggvgm","image_url":"https://images-3.gog-statics.com/7033_ggvgm.jpg"},
                {"formatter_name":"ggvgl","image_url":"https://images-3.gog-statics.com/7033_ggvgl.jpg"}]}],
        "videos":[{"video_url":"https://www.youtube.com/embed/X8YV_wzhadk?wmode=opaque&rel=0",
            "thumbnail_url":"https://img.youtube.com/vi/X8YV_wzhadk/hqdefault.jpg","provider":"youtube"}]}"#;

    #[test]
    fn the_real_answer_gives_sizes_screenshots_and_videos() {
        let details = parse(XENO_CRISIS).unwrap();

        assert_eq!(details.size(Os::Linux), Some(178_257_920));
        assert_eq!(details.size(Os::Windows), Some(168_820_736));
        assert_eq!(details.screenshots.len(), 1);
        assert!(details.screenshots[0].full.ends_with("_ggvgl.jpg"));
        assert!(details.screenshots[0].thumbnail.ends_with("_ggvgm.jpg"));
        assert_eq!(
            details.videos,
            ["https://www.youtube.com/watch?v=X8YV_wzhadk"]
        );
    }

    #[test]
    fn sizes_read_like_people_write_them() {
        assert_eq!(human_size(168_820_736), "161 MB");
        assert_eq!(human_size(8_388_608_000), "7.8 GB");
        assert_eq!(human_size(2048), "2 KB");
    }

    #[test]
    fn storage_splits_the_disk_and_leaves_the_rest_to_the_system() {
        let temp = tempfile::tempdir().unwrap();
        let games = temp.path().join("games");
        let library = temp.path().join("steam");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(library.join("steamapps/common")).unwrap();
        std::fs::write(games.join("game.bin"), [0; 300]).unwrap();
        std::fs::write(library.join("steamapps/common/x"), [0; 200]).unwrap();

        let storage = storage(
            &games,
            &temp.path().join("cache"),
            &[library.clone(), library],
        )
        .unwrap();

        assert_eq!(storage.gog, 300);
        assert_eq!(storage.steam, 200, "a library listed twice counts once");
        assert!(storage.total > 0 && storage.free <= storage.total);
        assert_eq!(
            storage.other() + storage.steam + storage.gog + storage.free,
            storage.total
        );
    }

    #[test]
    fn disk_usage_adds_up_files_in_subdirectories() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("a/b")).unwrap();
        std::fs::write(temp.path().join("one"), [0; 10]).unwrap();
        std::fs::write(temp.path().join("a/b/two"), [0; 5]).unwrap();
        assert_eq!(disk_usage(temp.path()), 15);
    }
}
