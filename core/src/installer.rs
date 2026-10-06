//! GOG offline installers: choose one, download it, and prove it is intact.
//!
//! This is the short road to a playable game that
//! `RECORD/2026-10-06.manual-install-prototype.completed.md` walked by hand. The
//! offline installer is a few plain files on GOG's CDN; each comes with a checksum
//! document holding its exact size, its MD5 and an MD5 per 10 MiB chunk. That is
//! enough to verify while downloading and to resume with a range request, with no
//! manifest protocol involved. The Galaxy content system (`man`, `dl`) can come
//! later for what it is better at, such as delta updates.
//!
//! Flow: [`windows_installer`] picks the installer, [`resolve`] turns each file's
//! `downlink` into a CDN address and a checksum address, [`Checksum::parse`] reads
//! the latter, and [`download`] fetches into the cache, verifying chunk by chunk.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::catalog::ProductId;
use crate::http::{Fetch, HttpClient};
use crate::{Error, Paths, Result};

/// The product endpoint; `expand=downloads` adds the installers to it.
pub const PRODUCT_ENDPOINT: &str = "https://api.gog.com/products";

/// Suffix of a file still being downloaded.
const PARTIAL: &str = "part";

/// One file of an installer. Large games come as a `setup_*.exe` and several
/// `setup_*-N.bin` next to it; the first file is the one to run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstallerFile {
    pub id: String,
    /// What the products API reports. It is approximate: the checksum document has
    /// the real size, and that is the one to trust.
    #[serde(default)]
    pub size: u64,
    pub downlink: String,
}

/// An installer for one operating system and language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Installer {
    pub id: String,
    pub os: String,
    pub language: String,
    #[serde(default)]
    pub version: Option<String>,
    pub files: Vec<InstallerFile>,
}

#[derive(Deserialize)]
struct Product {
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    #[serde(default)]
    installers: Vec<Installer>,
}

/// Where the installer for `id` is downloaded to.
pub fn download_dir(paths: &Paths, id: &ProductId) -> PathBuf {
    paths.cache_dir.join("downloads").join(id.as_str())
}

/// The Windows installer for `id`: in `language` when there is one, in English
/// when there is not, and otherwise in whatever GOG has. The Windows build is the
/// one that runs under Proton, which is the whole point of this project, so the
/// native Linux and Mac installers are never chosen here.
pub fn windows_installer<H: HttpClient>(
    http: &H,
    access_token: &str,
    id: &ProductId,
    language: &str,
) -> Result<Installer> {
    let url = format!("{PRODUCT_ENDPOINT}/{id}?expand=downloads");
    let response = http.get_authorized(&url, Some(access_token))?;
    if !response.is_success() {
        return Err(Error::Download(format!(
            "GOG refused the product request for {id} with status {}",
            response.status
        )));
    }
    let product: Product = serde_json::from_str(&response.body).map_err(|error| {
        Error::Download(format!(
            "the downloads of {id} could not be read ({error}); GOG may have changed \
             the shape of this response"
        ))
    })?;

    choose(product.downloads.installers, language)
        .ok_or_else(|| Error::Download(format!("{id} has no Windows installer on GOG")))
}

fn choose(installers: Vec<Installer>, language: &str) -> Option<Installer> {
    let windows: Vec<_> = installers
        .into_iter()
        .filter(|installer| installer.os == "windows" && !installer.files.is_empty())
        .collect();
    let position = windows
        .iter()
        .position(|installer| installer.language == language)
        .or_else(|| {
            windows
                .iter()
                .position(|installer| installer.language == "en")
        })
        .or(if windows.is_empty() { None } else { Some(0) })?;
    windows.into_iter().nth(position)
}

/// Where one installer file actually is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Link {
    /// The CDN address, signed and short-lived.
    #[serde(rename = "downlink")]
    pub url: String,
    /// The checksum document's address. GOG omits it for some files.
    #[serde(default)]
    pub checksum: Option<String>,
}

/// Follow a file's `downlink`, which needs the session, to its CDN address.
pub fn resolve<H: HttpClient>(http: &H, access_token: &str, file: &InstallerFile) -> Result<Link> {
    let response = http.get_authorized(&file.downlink, Some(access_token))?;
    if !response.is_success() {
        return Err(Error::Download(format!(
            "GOG refused the download link for {} with status {}",
            file.id, response.status
        )));
    }
    let link: Link = serde_json::from_str(&response.body).map_err(|error| {
        Error::Download(format!(
            "the download link for {} could not be read ({error})",
            file.id
        ))
    })?;
    Ok(Link {
        checksum: link.checksum.filter(|url| !url.is_empty()),
        ..link
    })
}

/// One verified span of a file, inclusive at both ends as GOG writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub from: u64,
    pub to: u64,
    pub md5: String,
}

/// GOG's checksum document for one installer file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksum {
    /// The file name, which is also the name the setup expects its parts under.
    pub name: String,
    pub md5: String,
    pub total_size: u64,
    pub chunks: Vec<Chunk>,
}

impl Checksum {
    /// Fetch and parse the checksum document at `url`.
    pub fn fetch<H: HttpClient>(http: &H, url: &str) -> Result<Self> {
        let response = http.get(url)?;
        if !response.is_success() {
            return Err(Error::Download(format!(
                "the checksum document was refused with status {}",
                response.status
            )));
        }
        Self::parse(&response.body)
    }

    /// Parse the document. It is a single `<file>` with one `<chunk>` per span; a
    /// hand-written reader is enough for that, and keeps an XML library out of the
    /// build.
    pub fn parse(xml: &str) -> Result<Self> {
        let invalid = |what: &str| Error::Download(format!("the checksum document has no {what}"));

        let start = xml.find("<file").ok_or_else(|| invalid("<file> element"))?;
        let head = &xml[start..];
        let head = &head[..head.find('>').ok_or_else(|| invalid("<file> element"))?];

        let name = attribute(head, "name").ok_or_else(|| invalid("file name"))?;
        let md5 = attribute(head, "md5").ok_or_else(|| invalid("file md5"))?;
        let total_size = attribute(head, "total_size")
            .and_then(|size| size.parse().ok())
            .ok_or_else(|| invalid("file size"))?;

        let mut chunks = Vec::new();
        let mut rest = &xml[start..];
        while let Some(open) = rest.find("<chunk") {
            rest = &rest[open..];
            let tag_end = rest.find('>').ok_or_else(|| invalid("complete <chunk>"))?;
            let tag = &rest[..tag_end];
            let close = rest
                .find("</chunk>")
                .ok_or_else(|| invalid("complete <chunk>"))?;
            let digest = rest[tag_end + 1..close].trim().to_ascii_lowercase();
            let span = |key| {
                attribute(tag, key)
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or_else(|| invalid("chunk span"))
            };
            if attribute(tag, "method").is_some_and(|method| method != "md5") {
                return Err(Error::Download(
                    "the checksum document uses a method other than md5".to_string(),
                ));
            }
            chunks.push(Chunk {
                from: span("from")?,
                to: span("to")?,
                md5: digest,
            });
            rest = &rest[close..];
        }

        let checksum = Self {
            name: unescape(&name),
            md5: md5.to_ascii_lowercase(),
            total_size,
            chunks,
        };
        checksum.check_coverage()?;
        Ok(checksum)
    }

    /// The chunks have to tile the file exactly, or verifying them all would not
    /// mean the file is right.
    fn check_coverage(&self) -> Result<()> {
        let mut next = 0;
        for chunk in &self.chunks {
            if chunk.from != next || chunk.to < chunk.from {
                return Err(Error::Download(
                    "the checksum document's chunks do not cover the file in order".to_string(),
                ));
            }
            next = chunk.to + 1;
        }
        if !self.chunks.is_empty() && next != self.total_size {
            return Err(Error::Download(
                "the checksum document's chunks do not add up to the file size".to_string(),
            ));
        }
        if self.name.contains(['/', '\\']) || self.name.starts_with('.') {
            return Err(Error::Download(format!(
                "refusing a download named {:?}",
                self.name
            )));
        }
        Ok(())
    }
}

fn attribute(tag: &str, key: &str) -> Option<String> {
    let needle = format!(" {key}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_string())
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Download one installer file into `dir`, verified against `checksum`, and
/// return its path.
///
/// A complete, verified file is not fetched again. A `.part` left by an
/// interrupted run is checked chunk by chunk, cut back to its last good chunk,
/// and continued from there with a range request; a server that ignores the range
/// gets the file from the start instead. Every chunk is checked as it arrives, so
/// a bad byte stops the download at the chunk it is in.
///
/// `progress` is called with the bytes on disk and the total.
pub fn download<F: Fetch>(
    fetch: &F,
    url: &str,
    checksum: &Checksum,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;
    let target = dir.join(&checksum.name);
    let partial = dir.join(format!("{}.{PARTIAL}", checksum.name));

    if target.exists() {
        if verified_prefix(&target, checksum)? == checksum.total_size {
            progress(checksum.total_size, checksum.total_size);
            return Ok(target);
        }
        std::fs::rename(&target, &partial)
            .map_err(|error| Error::io(format!("moving aside {}", target.display()), error))?;
    }

    let mut from = if partial.exists() {
        verified_prefix(&partial, checksum)?
    } else {
        0
    };

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&partial)
        .map_err(|error| Error::io(format!("opening {}", partial.display()), error))?;

    loop {
        file.set_len(from)
            .and_then(|()| file.seek(SeekFrom::Start(from)).map(|_| ()))
            .map_err(|error| Error::io(format!("preparing {}", partial.display()), error))?;

        if from == checksum.total_size {
            break;
        }

        let (status, mut body) = fetch.open(url, from)?;
        let start = match status {
            // The range was honoured, or there was none to honour.
            206 => from,
            200 if from == 0 => 0,
            // A full body in answer to a range: keep nothing and take it from the top.
            200 => {
                file.set_len(0)
                    .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
                    .map_err(|error| {
                        Error::io(format!("preparing {}", partial.display()), error)
                    })?;
                0
            }
            status => {
                return Err(Error::Download(format!(
                    "the CDN answered {status} for {}",
                    checksum.name
                )));
            }
        };

        let mut sink = Verifier::new(&mut file, checksum, start, progress);
        let copied = std::io::copy(&mut body, &mut sink);
        let (written, good) = (sink.position, sink.good);

        // Whatever happened, only verified bytes stay on disk for the next run.
        file.flush()
            .and_then(|()| file.set_len(good))
            .map_err(|error| Error::io(format!("writing {}", partial.display()), error))?;
        copied.map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                Error::Download(format!("{}: {error}", checksum.name))
            } else {
                Error::io(format!("downloading {}", checksum.name), error)
            }
        })?;

        if written != checksum.total_size {
            return Err(Error::Download(format!(
                "{} ended at {written} bytes of {}; run the install again to resume",
                checksum.name, checksum.total_size
            )));
        }
        from = written;
    }

    drop(file);
    if checksum.chunks.is_empty() && md5_of(&partial)? != checksum.md5 {
        return Err(Error::Download(format!(
            "{} does not match its checksum",
            checksum.name
        )));
    }
    std::fs::rename(&partial, &target)
        .map_err(|error| Error::io(format!("finishing {}", target.display()), error))?;
    Ok(target)
}

/// How many leading bytes of `path` are whole, verified chunks. Without chunks the
/// whole file is checked against the file MD5, and it is either all or nothing.
fn verified_prefix(path: &Path, checksum: &Checksum) -> Result<u64> {
    let length = std::fs::metadata(path)
        .map_err(|error| Error::io(format!("reading {}", path.display()), error))?
        .len();

    if checksum.chunks.is_empty() {
        let whole = length == checksum.total_size && md5_of(path)? == checksum.md5;
        return Ok(if whole { length } else { 0 });
    }

    let mut file = File::open(path)
        .map_err(|error| Error::io(format!("reading {}", path.display()), error))?;
    let mut good = 0;
    let mut buffer = vec![0; 1 << 20];
    for chunk in &checksum.chunks {
        if chunk.to >= length {
            break;
        }
        let mut hasher = Md5::new();
        let mut left = chunk.to - chunk.from + 1;
        while left > 0 {
            let take = left.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..take])
                .map_err(|error| Error::io(format!("reading {}", path.display()), error))?;
            hasher.update(&buffer[..take]);
            left -= take as u64;
        }
        if hex(&hasher.finalize()) != chunk.md5 {
            break;
        }
        good = chunk.to + 1;
    }
    Ok(good)
}

fn md5_of(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .map_err(|error| Error::io(format!("reading {}", path.display()), error))?;
    let mut hasher = Md5::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|error| Error::io(format!("reading {}", path.display()), error))?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A writer that checks each chunk as its last byte goes through, and refuses to
/// write past the end of the file or past a bad chunk.
struct Verifier<'a, W: Write> {
    inner: W,
    checksum: &'a Checksum,
    position: u64,
    /// End of the last verified chunk; everything when there are no chunks.
    good: u64,
    chunk: usize,
    hasher: Md5,
    progress: &'a mut dyn FnMut(u64, u64),
}

impl<'a, W: Write> Verifier<'a, W> {
    fn new(
        inner: W,
        checksum: &'a Checksum,
        from: u64,
        progress: &'a mut dyn FnMut(u64, u64),
    ) -> Self {
        // `from` is always a chunk boundary: it comes from `verified_prefix`.
        let chunk = checksum
            .chunks
            .iter()
            .position(|chunk| chunk.from == from)
            .unwrap_or(checksum.chunks.len());
        progress(from, checksum.total_size);
        Self {
            inner,
            checksum,
            position: from,
            good: from,
            chunk,
            hasher: Md5::new(),
            progress,
        }
    }
}

impl<W: Write> Write for Verifier<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = self.checksum.total_size - self.position;
        if room == 0 && !data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the server sent more bytes than the file has",
            ));
        }
        let mut data = &data[..data.len().min(room as usize)];

        // Feed the hasher up to the end of the current chunk at most, so the
        // check happens exactly at the boundary.
        if let Some(chunk) = self.checksum.chunks.get(self.chunk) {
            let to_boundary = (chunk.to + 1 - self.position) as usize;
            data = &data[..data.len().min(to_boundary)];
        }

        let written = self.inner.write(data)?;
        if let Some(chunk) = self.checksum.chunks.get(self.chunk) {
            self.hasher.update(&data[..written]);
            if self.position + written as u64 == chunk.to + 1 {
                let digest = hex(&std::mem::take(&mut self.hasher).finalize());
                if digest != chunk.md5 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "bytes {}..={} do not match their checksum; run the install again \
                             to fetch them anew",
                            chunk.from, chunk.to
                        ),
                    ));
                }
                self.chunk += 1;
                self.good = chunk.to + 1;
            }
        }
        self.position += written as u64;
        if self.checksum.chunks.is_empty() {
            self.good = self.position;
        }
        (self.progress)(self.position, self.checksum.total_size);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// How the installed game is started, from the `goggame-<id>.info` GOG's
/// installers leave in the install directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayTask {
    /// The executable, inside the install directory.
    pub exe: PathBuf,
    /// Arguments GOG passes it, often none.
    pub arguments: String,
    /// Where it expects to start; the install directory when GOG says nothing.
    pub working_dir: PathBuf,
}

#[derive(Deserialize)]
struct GameInfo {
    #[serde(default, rename = "playTasks")]
    play_tasks: Vec<RawPlayTask>,
}

#[derive(Deserialize)]
struct RawPlayTask {
    #[serde(default, rename = "isPrimary")]
    is_primary: bool,
    #[serde(default)]
    category: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default, rename = "workingDir")]
    working_dir: Option<String>,
}

/// The primary play task of the game installed in `install_dir`.
///
/// The file is named after the product id, but a game can carry the info of its
/// root product instead, so any `goggame-*.info` is accepted when the exact one is
/// not there. Paths in it use Windows separators.
pub fn primary_task(install_dir: &Path, id: &ProductId) -> Result<PlayTask> {
    let exact = install_dir.join(format!("goggame-{id}.info"));
    let info = if exact.is_file() {
        exact
    } else {
        std::fs::read_dir(install_dir)
            .map_err(|error| Error::io(format!("reading {}", install_dir.display()), error))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("goggame-") && name.ends_with(".info"))
            })
            .ok_or_else(|| {
                Error::Install(format!(
                    "no goggame-*.info in {}, so the game's executable is unknown",
                    install_dir.display()
                ))
            })?
    };

    let text = std::fs::read_to_string(&info)
        .map_err(|error| Error::io(format!("reading {}", info.display()), error))?;
    let parsed: GameInfo = serde_json::from_str(&text).map_err(|error| {
        Error::Install(format!("{} could not be read ({error})", info.display()))
    })?;

    let task = parsed
        .play_tasks
        .into_iter()
        .filter(|task| {
            task.kind.as_deref().unwrap_or("FileTask") == "FileTask" && task.path.is_some()
        })
        .filter(|task| task.category.as_deref().unwrap_or("game") == "game")
        .max_by_key(|task| task.is_primary)
        .ok_or_else(|| Error::Install(format!("{} names no executable to play", info.display())))?;

    let local = |windows: &str| install_dir.join(windows.replace('\\', "/"));
    Ok(PlayTask {
        exe: local(task.path.as_deref().unwrap_or_default()),
        arguments: task.arguments.unwrap_or_default(),
        working_dir: task
            .working_dir
            .filter(|dir| !dir.is_empty())
            .map(|dir| local(&dir))
            .unwrap_or_else(|| install_dir.to_path_buf()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;
    use std::cell::RefCell;

    /// The real installer list for Xeno Crisis (GOG id 1955095293), as
    /// `products/1955095293?expand=downloads` returned it on 2026-10-06, cut down to
    /// the installers.
    const XENO_CRISIS_DOWNLOADS: &str = r#"{"id":1955095293,"title":"Xeno Crisis","downloads":{"installers":[
        {"id":"installer_windows_en","name":"Xeno Crisis","os":"windows","language":"en","language_full":"English","version":"1.0.4","total_size":168820736,
         "files":[{"id":"en1installer0","size":168820736,"downlink":"https://api.gog.com/products/1955095293/downlink/installer/en1installer0"}]},
        {"id":"installer_mac_en","name":"Xeno Crisis","os":"mac","language":"en","language_full":"English","version":"1.0.4","total_size":179306496,
         "files":[{"id":"en2installer0","size":179306496,"downlink":"https://api.gog.com/products/1955095293/downlink/installer/en2installer0"}]},
        {"id":"installer_linux_en","name":"Xeno Crisis","os":"linux","language":"en","language_full":"English","version":"1.0.4","total_size":178257920,
         "files":[{"id":"en3installer0","size":178257920,"downlink":"https://api.gog.com/products/1955095293/downlink/installer/en3installer0"}]}
        ],"patches":[],"language_packs":[],"bonus_content":[]}}"#;

    /// The head and the last two chunks of the real checksum document for the file
    /// above. Its 17 chunks are not all here, so it is used for parsing only.
    const XENO_CRISIS_CHECKSUM_HEAD: &str = r#"<file name="setup_xeno_crisis_1.0.4_(64bit)_(36061).exe" available="1" notavailablemsg="" md5="52e53c80b1b9e096a73cf736b0d63a76" chunks="17" timestamp="2020-08-11 15:20:15" total_size="169744608">
	<chunk id="0" from="0" to="10485759" method="md5">008588d36dc1eaea48b7a5fe97595321</chunk>
	<chunk id="1" from="10485760" to="20971519" method="md5">4d7615684b99b0129b62f3ca816a167c</chunk>
</file>"#;

    struct Api(&'static str);

    impl HttpClient for Api {
        fn get_authorized(&self, _: &str, bearer: Option<&str>) -> Result<Response> {
            assert_eq!(bearer, Some("token"));
            Ok(Response {
                status: 200,
                body: self.0.to_string(),
            })
        }
    }

    #[test]
    fn the_windows_installer_is_picked_out_of_the_real_response() {
        let installer = windows_installer(
            &Api(XENO_CRISIS_DOWNLOADS),
            "token",
            &"1955095293".into(),
            "es",
        )
        .unwrap();

        assert_eq!(installer.os, "windows");
        assert_eq!(installer.language, "en");
        assert_eq!(installer.version.as_deref(), Some("1.0.4"));
        assert_eq!(installer.files.len(), 1);
        assert_eq!(installer.files[0].id, "en1installer0");
    }

    fn installer(language: &str, os: &str) -> Installer {
        Installer {
            id: format!("installer_{os}_{language}"),
            os: os.to_string(),
            language: language.to_string(),
            version: None,
            files: vec![InstallerFile {
                id: "f".to_string(),
                size: 1,
                downlink: "d".to_string(),
            }],
        }
    }

    #[test]
    fn the_language_falls_back_to_english_and_then_to_anything() {
        let all = vec![
            installer("de", "windows"),
            installer("en", "windows"),
            installer("es", "linux"),
        ];
        assert_eq!(choose(all.clone(), "de").unwrap().language, "de");
        assert_eq!(choose(all.clone(), "es").unwrap().language, "en");
        assert_eq!(
            choose(vec![installer("fr", "windows")], "es")
                .unwrap()
                .language,
            "fr"
        );
        assert_eq!(choose(vec![installer("es", "linux")], "es"), None);
    }

    #[test]
    fn the_real_checksum_head_parses() {
        let error = Checksum::parse(XENO_CRISIS_CHECKSUM_HEAD).unwrap_err();
        // Two of seventeen chunks: parsed, and then rejected for not covering the
        // file, which is the check that matters.
        assert!(error.to_string().contains("add up"), "{error}");

        let whole = XENO_CRISIS_CHECKSUM_HEAD
            .replace("chunks=\"17\"", "chunks=\"0\"")
            .lines()
            .filter(|line| !line.contains("<chunk"))
            .collect::<Vec<_>>()
            .join("\n");
        let checksum = Checksum::parse(&whole).unwrap();
        assert_eq!(checksum.name, "setup_xeno_crisis_1.0.4_(64bit)_(36061).exe");
        assert_eq!(checksum.md5, "52e53c80b1b9e096a73cf736b0d63a76");
        assert_eq!(checksum.total_size, 169_744_608);
    }

    #[test]
    fn a_name_that_leaves_the_download_directory_is_refused() {
        let xml = r#"<file name="../evil.exe" md5="00" total_size="0"></file>"#;
        assert!(Checksum::parse(xml).is_err());
    }

    /// A file, its checksum document in 4-byte chunks, and a fake CDN serving it.
    fn content() -> Vec<u8> {
        (0u8..18).collect()
    }

    fn checksum_for(data: &[u8], chunk: usize) -> Checksum {
        let chunks = data
            .chunks(chunk)
            .enumerate()
            .map(|(index, part)| Chunk {
                from: (index * chunk) as u64,
                to: (index * chunk + part.len() - 1) as u64,
                md5: hex(&Md5::digest(part)),
            })
            .collect();
        Checksum {
            name: "setup.exe".to_string(),
            md5: hex(&Md5::digest(data)),
            total_size: data.len() as u64,
            chunks,
        }
    }

    struct Cdn {
        data: Vec<u8>,
        honours_ranges: bool,
        /// Stop after this many bytes, as a dropped connection would.
        cut_after: Option<usize>,
        requests: RefCell<Vec<u64>>,
    }

    impl Cdn {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                honours_ranges: true,
                cut_after: None,
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl Fetch for Cdn {
        fn open(&self, _: &str, from: u64) -> Result<(u16, Box<dyn Read>)> {
            self.requests.borrow_mut().push(from);
            let (status, body) = if from > 0 && self.honours_ranges {
                (206, &self.data[from as usize..])
            } else {
                (200, &self.data[..])
            };
            let body = body[..body.len().min(self.cut_after.unwrap_or(usize::MAX))].to_vec();
            Ok((status, Box::new(std::io::Cursor::new(body))))
        }
    }

    fn run(cdn: &Cdn, checksum: &Checksum, dir: &Path) -> Result<PathBuf> {
        download(cdn, "https://cdn/setup.exe", checksum, dir, &mut |_, _| {})
    }

    #[test]
    fn a_clean_download_is_verified_and_named_after_the_checksum() {
        let temp = tempfile::tempdir().unwrap();
        let cdn = Cdn::new(content());
        let checksum = checksum_for(&cdn.data, 4);

        let path = run(&cdn, &checksum, temp.path()).unwrap();

        assert_eq!(path, temp.path().join("setup.exe"));
        assert_eq!(std::fs::read(&path).unwrap(), cdn.data);
        assert!(!temp.path().join("setup.exe.part").exists());
    }

    #[test]
    fn a_verified_file_is_not_fetched_again() {
        let temp = tempfile::tempdir().unwrap();
        let cdn = Cdn::new(content());
        let checksum = checksum_for(&cdn.data, 4);
        run(&cdn, &checksum, temp.path()).unwrap();

        run(&cdn, &checksum, temp.path()).unwrap();

        assert_eq!(*cdn.requests.borrow(), [0]);
    }

    #[test]
    fn an_interrupted_download_resumes_from_its_last_good_chunk() {
        let temp = tempfile::tempdir().unwrap();
        let mut cdn = Cdn::new(content());
        let checksum = checksum_for(&cdn.data, 4);
        cdn.cut_after = Some(10);

        let error = run(&cdn, &checksum, temp.path()).unwrap_err();
        assert!(error.to_string().contains("resume"), "{error}");

        cdn.cut_after = None;
        let path = run(&cdn, &checksum, temp.path()).unwrap();

        // Ten bytes arrived; two whole chunks (8 bytes) were kept.
        assert_eq!(*cdn.requests.borrow(), [0, 8]);
        assert_eq!(std::fs::read(path).unwrap(), cdn.data);
    }

    #[test]
    fn a_corrupt_partial_is_cut_back_to_the_chunk_before_the_damage() {
        let temp = tempfile::tempdir().unwrap();
        let cdn = Cdn::new(content());
        let checksum = checksum_for(&cdn.data, 4);
        let mut damaged = cdn.data[..12].to_vec();
        damaged[5] ^= 0xff;
        std::fs::write(temp.path().join("setup.exe.part"), damaged).unwrap();

        let path = run(&cdn, &checksum, temp.path()).unwrap();

        assert_eq!(*cdn.requests.borrow(), [4]);
        assert_eq!(std::fs::read(path).unwrap(), cdn.data);
    }

    #[test]
    fn a_server_that_ignores_the_range_gets_a_fresh_start() {
        let temp = tempfile::tempdir().unwrap();
        let mut cdn = Cdn::new(content());
        cdn.honours_ranges = false;
        let checksum = checksum_for(&cdn.data, 4);
        std::fs::write(temp.path().join("setup.exe.part"), &cdn.data[..8]).unwrap();

        let path = run(&cdn, &checksum, temp.path()).unwrap();

        assert_eq!(*cdn.requests.borrow(), [8]);
        assert_eq!(std::fs::read(path).unwrap(), cdn.data);
    }

    #[test]
    fn a_bad_chunk_on_the_wire_stops_the_download_there() {
        let temp = tempfile::tempdir().unwrap();
        let mut cdn = Cdn::new(content());
        let checksum = checksum_for(&cdn.data, 4);
        cdn.data[9] ^= 0xff;

        let error = run(&cdn, &checksum, temp.path()).unwrap_err();

        assert!(error.to_string().contains("bytes 8..=11"), "{error}");
        let kept = std::fs::read(temp.path().join("setup.exe.part")).unwrap();
        assert_eq!(kept.len(), 8, "nothing past the last good chunk is kept");
    }

    /// The real `goggame-1955095293.info` Xeno Crisis's installer wrote.
    const XENO_CRISIS_INFO: &str = r#"{"buildId":"53063895835721891","clientId":"52046146912113724","gameId":"1955095293","language":"English","languages":["en-US"],"name":"Xeno Crisis","osBitness":["64"],"playTasks":[{"category":"game","isPrimary":true,"languages":["en-US"],"name":"Xeno Crisis","path":"Xeno Crisis.exe","type":"FileTask"}],"rootGameId":"1955095293","version":1}"#;

    #[test]
    fn the_primary_task_of_the_real_info_file_is_found() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("goggame-1955095293.info"),
            XENO_CRISIS_INFO,
        )
        .unwrap();

        let task = primary_task(temp.path(), &"1955095293".into()).unwrap();

        assert_eq!(task.exe, temp.path().join("Xeno Crisis.exe"));
        assert_eq!(task.working_dir, temp.path());
        assert_eq!(task.arguments, "");
    }

    #[test]
    fn links_and_secondary_tasks_lose_to_the_primary_game_exe() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("goggame-1.info"),
            r#"{"playTasks":[
                {"category":"document","path":"manual.pdf","type":"FileTask"},
                {"category":"game","path":"bin\\launcher.exe","type":"FileTask"},
                {"category":"game","isPrimary":true,"path":"bin\\game.exe","arguments":"-dx11","workingDir":"bin","type":"FileTask"},
                {"category":"tool","link":"https://example.com","type":"URLTask"}]}"#,
        )
        .unwrap();

        let task = primary_task(temp.path(), &"1".into()).unwrap();

        assert_eq!(task.exe, temp.path().join("bin/game.exe"));
        assert_eq!(task.arguments, "-dx11");
        assert_eq!(task.working_dir, temp.path().join("bin"));
    }

    #[test]
    fn without_chunks_the_whole_file_md5_decides() {
        let temp = tempfile::tempdir().unwrap();
        let cdn = Cdn::new(content());
        let mut checksum = checksum_for(&cdn.data, 4);
        checksum.chunks.clear();

        run(&cdn, &checksum, temp.path()).unwrap();

        checksum.md5 = "0".repeat(32);
        std::fs::remove_file(temp.path().join("setup.exe")).unwrap();
        let error = run(&cdn, &checksum, temp.path()).unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error}");
    }
}
