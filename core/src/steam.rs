//! Steam's own identifiers for the shortcuts we add.
//!
//! A non-Steam shortcut has no appid from Valve's side, so Steam derives one from
//! the shortcut itself: a CRC32 over the executable and the display name. Every
//! later Steam task hangs off this number being the one Steam would have computed
//! — the Proton prefix lives in `compatdata/<appid>`, and the tile artwork is
//! named after it — so a wrong CRC does not fail loudly, it silently sets up a
//! prefix nobody launches into. `03-platform-linux.md` names that as the risk to
//! test end to end early, and this module is what it tests.
//!
//! This lives in `core` rather than `platform-linux`, even though the roadmap
//! schedules `appid` in the Linux phase: the derivation is arithmetic over two
//! strings, identical on both platforms, and `wvdf` will want exactly this. Per
//! `AGENTS.md`, what both platforms do the same way belongs here.

/// Table-driven CRC-32 (IEEE 802.3, the reflected `0xEDB88320` form) — the one
/// `zlib.crc32` computes, which is what Steam uses.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

/// CRC-32 of `bytes`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc = CRC32_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }

    crc ^ 0xFFFF_FFFF
}

/// The 32-bit appid Steam uses for a non-Steam shortcut.
///
/// This is the number that names the Proton prefix (`compatdata/<appid>`) and the
/// artwork files under `userdata/<user>/config/grid/`.
///
/// `exe` must be **the exact string stored in the shortcut's `Exe` field**, quotes
/// included. Steam quotes the path, and the CRC is over what is stored, not over
/// the path as a `Path` would print it — see [`quote_exe`]. Getting this wrong is
/// the appid-mismatch risk, and it is why the two live next to each other.
pub fn shortcut_appid(exe: &str, app_name: &str) -> u32 {
    let mut input = Vec::with_capacity(exe.len() + app_name.len());
    input.extend_from_slice(exe.as_bytes());
    input.extend_from_slice(app_name.as_bytes());

    // The high bit is what marks the id as a shortcut rather than a real appid.
    crc32(&input) | 0x8000_0000
}

/// The 64-bit form of the same identifier, as it appears in `steam://rungameid/`
/// links and in some of Steam's own files.
pub fn shortcut_id(exe: &str, app_name: &str) -> u64 {
    appid_to_shortcut_id(shortcut_appid(exe, app_name))
}

/// Widen a 32-bit shortcut appid into its 64-bit form.
pub fn appid_to_shortcut_id(appid: u32) -> u64 {
    (appid as u64) << 32 | 0x0200_0000
}

/// Narrow a 64-bit shortcut id back to the appid the prefix and artwork use.
pub fn shortcut_id_to_appid(shortcut_id: u64) -> u32 {
    (shortcut_id >> 32) as u32
}

/// The executable as Steam stores it in `shortcuts.vdf`: quoted.
///
/// Whatever this returns is what must be fed to [`shortcut_appid`] *and* written
/// into the shortcut, or the prefix the wrapper prepares is not the prefix Steam
/// launches into.
pub fn quote_exe(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// The `compatdata` directory name for a shortcut's Proton prefix.
pub fn compatdata_name(appid: u32) -> String {
    appid.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors for the plain CRC-32, checked against Python's `zlib.crc32`, which is
    // the same function Steam's own tooling and every reference implementation use.
    // These pin the checksum half of the derivation; what they cannot pin is the
    // Steam-specific half — which string goes in, and the high-bit mask — and that
    // is settled by the end-to-end run on the mini PC, not here.
    const CRC32_VECTORS: &[(&str, u32)] = &[
        ("", 0x00000000),
        ("a", 0xe8b7be43),
        ("abc", 0x352441c2),
        ("message digest", 0x20159d7f),
        ("abcdefghijklmnopqrstuvwxyz", 0x4c2750bd),
        ("123456789", 0xcbf43926),
        ("The quick brown fox jumps over the lazy dog", 0x414fa339),
        ("Brütal Legend", 0x55284eae),
        ("Beneath a Steel Sky", 0xd4348ea3),
        ("Baldur's Gate II", 0x05e4ad9c),
        ("Grim Fandango Remastered", 0x33d3c92b),
        ("FTL: Faster Than Light", 0x616048fd),
        ("\"/home/deck/Games/Bastion/Bastion.exe\"", 0x7c0219bf),
        (
            "\"/home/deck/Games/Brütal Legend/BrutalLegend.exe\"Brütal Legend",
            0x47743451,
        ),
        ("\"C:\\Games\\Gothic II\\Gothic2.exe\"Gothic II", 0xfb454b77),
        ("日本語のタイトル", 0x27be6e0b),
    ];

    #[test]
    fn crc32_matches_the_standard_vectors() {
        for (input, expected) in CRC32_VECTORS {
            assert_eq!(
                crc32(input.as_bytes()),
                *expected,
                "crc32({input:?}) should be {expected:#010x}"
            );
        }
    }

    #[test]
    fn crc32_is_computed_over_bytes_not_characters() {
        // A non-ASCII title is ordinary in a GOG library, and the CRC has to be over
        // its UTF-8 bytes; anything else silently diverges from Steam for exactly
        // the games whose names are not plain ASCII.
        let text = "Brütal Legend";
        assert_eq!(crc32(text.as_bytes()), crc32(text.to_string().as_bytes()));
        assert_ne!(crc32("Brutal Legend".as_bytes()), crc32(text.as_bytes()));
    }

    #[test]
    fn an_appid_always_has_the_high_bit_set() {
        for name in ["A", "Beneath a Steel Sky", "Brütal Legend", ""] {
            let appid = shortcut_appid("\"/games/game.exe\"", name);
            assert_ne!(appid & 0x8000_0000, 0, "{name:?} produced {appid:#010x}");
        }
    }

    #[test]
    fn the_appid_is_the_crc_of_the_exe_followed_by_the_name() {
        let exe = "\"/games/Bastion/Bastion.exe\"";
        let name = "Bastion";

        let expected = crc32(format!("{exe}{name}").as_bytes()) | 0x8000_0000;

        assert_eq!(shortcut_appid(exe, name), expected);
    }

    #[test]
    fn the_same_shortcut_always_derives_the_same_appid() {
        let first = shortcut_appid("\"/games/game.exe\"", "A Game");
        let second = shortcut_appid("\"/games/game.exe\"", "A Game");

        assert_eq!(first, second, "the derivation has to be stable across runs");
    }

    #[test]
    fn a_different_exe_or_name_derives_a_different_appid() {
        let base = shortcut_appid("\"/games/game.exe\"", "A Game");

        assert_ne!(base, shortcut_appid("\"/games/other.exe\"", "A Game"));
        assert_ne!(base, shortcut_appid("\"/games/game.exe\"", "Other Game"));
    }

    #[test]
    fn quoting_changes_the_appid_which_is_why_quote_exe_exists() {
        // The trap this module is guarding: the CRC is over the stored string, so
        // the quoted and unquoted spellings are different shortcuts to Steam.
        let path = "/games/game.exe";

        assert_ne!(
            shortcut_appid(path, "A Game"),
            shortcut_appid(&quote_exe(path), "A Game")
        );
        assert_eq!(quote_exe(path), "\"/games/game.exe\"");
    }

    #[test]
    fn the_two_id_widths_convert_both_ways() {
        let exe = quote_exe("/games/game.exe");
        let appid = shortcut_appid(&exe, "A Game");

        let wide = shortcut_id(&exe, "A Game");

        assert_eq!(wide, appid_to_shortcut_id(appid));
        assert_eq!(shortcut_id_to_appid(wide), appid);
        assert_eq!(
            wide & 0xFFFF_FFFF,
            0x0200_0000,
            "the low half is the marker"
        );
    }

    #[test]
    fn the_prefix_directory_is_named_after_the_appid() {
        let appid = shortcut_appid("\"/games/game.exe\"", "A Game");

        assert_eq!(compatdata_name(appid), appid.to_string());
        assert!(
            appid > 2_147_483_647,
            "the high bit makes it a large unsigned number, and it must not be \
             formatted as a negative one"
        );
    }
}
