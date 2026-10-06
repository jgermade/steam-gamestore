//! Third-party tools some games need, fetched on demand and verified.
//!
//! Nothing here is shipped with gamestore. Each tool is a pinned upstream release,
//! downloaded the first time a game needs it, checked against the SHA-256 its
//! publisher lists, and unpacked under `<cache>/tools/<name>-<version>/`.

use std::path::{Path, PathBuf};
use std::process::Command;

use gamestore_core::fixes::{self, DgVoodoo};
use gamestore_core::http::{DownloadClient, Fetch};
use gamestore_core::{Error, Paths, Result};
use sha2::{Digest, Sha256};

/// A pinned release.
#[derive(Debug, Clone, Copy)]
pub struct Release {
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// dgVoodoo2, from its author's GitHub releases; the digest is the one GitHub
/// lists for the asset.
pub const DGVOODOO: Release = Release {
    name: "dgvoodoo2",
    version: "2.87.5",
    url: "https://github.com/dege-diosg/dgVoodoo2/releases/download/v2.87.5/dgVoodoo2_87_5.zip",
    sha256: "5ffde6927f7355ca3fdd5d785b581256a8e6539fa13e395a891ade6ba1040850",
};

/// The unpacked release, downloading and verifying it the first time.
pub fn fetch(paths: &Paths, release: &Release) -> Result<PathBuf> {
    let dir = paths
        .cache_dir
        .join("tools")
        .join(format!("{}-{}", release.name, release.version));
    let done = dir.join(".verified");
    if done.is_file() {
        return Ok(dir);
    }

    std::fs::create_dir_all(&dir)
        .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;
    let (status, mut body) = DownloadClient::new().open(release.url, 0)?;
    if status != 200 {
        return Err(Error::Download(format!(
            "{} {}: the server answered {status}",
            release.name, release.version
        )));
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut bytes)
        .map_err(|error| Error::io(format!("downloading {}", release.name), error))?;
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != release.sha256 {
        return Err(Error::Download(format!(
            "{} {} does not match its published SHA-256; not using it",
            release.name, release.version
        )));
    }

    let archive = dir.join("release.zip");
    std::fs::write(&archive, &bytes)
        .map_err(|error| Error::io(format!("writing {}", archive.display()), error))?;
    let output = Command::new("unzip")
        .arg("-qq")
        .arg("-o")
        .arg(&archive)
        .arg("-d")
        .arg(&dir)
        .output()
        .map_err(|error| Error::io("running unzip (is it installed?)", error))?;
    if !output.status.success() {
        return Err(Error::Install(format!(
            "unzip could not extract {}: {}",
            release.name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let _ = std::fs::remove_file(&archive);
    std::fs::write(&done, release.sha256)
        .map_err(|error| Error::io(format!("writing {}", done.display()), error))?;
    Ok(dir)
}

/// Put dgVoodoo next to a game: its DLLs, and its configuration with `settings`.
pub fn install_dgvoodoo(paths: &Paths, target: &Path, settings: &DgVoodoo) -> Result<()> {
    let release = fetch(paths, &DGVOODOO)?;
    for file in fixes::DGVOODOO_FILES {
        let from = release.join(file);
        let name = Path::new(file).file_name().expect("a file name");
        std::fs::copy(&from, target.join(name))
            .map_err(|error| Error::io(format!("copying {}", from.display()), error))?;
    }
    let template_path = release.join("dgVoodoo.conf");
    let template = std::fs::read(&template_path)
        .map_err(|error| Error::io(format!("reading {}", template_path.display()), error))?;
    let configured =
        fixes::configure_dgvoodoo(&String::from_utf8_lossy(&template), settings.settings)
            .map_err(Error::Install)?;
    let conf = target.join("dgVoodoo.conf");
    std::fs::write(&conf, configured)
        .map_err(|error| Error::io(format!("writing {}", conf.display()), error))
}
