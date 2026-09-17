//! Steam's binary VDF, as used by `shortcuts.vdf`.
//!
//! The format is a tree of keyed values, each introduced by a one-byte type tag:
//!
//! | Tag    | Meaning                                    |
//! |--------|--------------------------------------------|
//! | `0x00` | nested map, key follows, ends at `0x08`     |
//! | `0x01` | string, NUL-terminated                      |
//! | `0x02` | signed 32-bit, little endian                |
//! | `0x07` | unsigned 64-bit, little endian              |
//! | `0x08` | end of the current map                      |
//!
//! The whole point of this module is **not losing anything**. `shortcuts.vdf` is
//! shared: Steam writes it, and so do Boilr, Heroic and whatever else the user has
//! installed. A parser that understood only the keys gamestore cares about and
//! rewrote the file from its own idea of a shortcut would quietly delete other
//! tools' entries and any field Valve adds in a future client. So [`Value`] keeps
//! every key, in order, with its original type, and [`write`] puts the bytes back
//! exactly as they came in — there is a test that round-trips a document byte for
//! byte, and it is the most important test in this file.

use crate::{Error, Result};

/// Tag for a nested map.
const TAG_MAP: u8 = 0x00;
/// Tag for a NUL-terminated string.
const TAG_STRING: u8 = 0x01;
/// Tag for a little-endian `i32`.
const TAG_INT32: u8 = 0x02;
/// Tag for a little-endian `u64`.
const TAG_UINT64: u8 = 0x07;
/// Tag closing the current map.
const TAG_END: u8 = 0x08;

/// A value in a binary VDF document.
///
/// Maps keep insertion order, because Steam's own files have a stable field order
/// and rewriting them in a different one produces a needlessly large diff against
/// whatever the client wrote — and makes the byte-identical round trip untestable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A nested map, in file order.
    Map(Vec<(String, Value)>),
    /// A NUL-terminated string.
    String(String),
    /// A little-endian signed 32-bit integer.
    Int32(i32),
    /// A little-endian unsigned 64-bit integer.
    UInt64(u64),
}

impl Value {
    /// An empty map.
    pub fn map() -> Self {
        Value::Map(Vec::new())
    }

    /// The entries, when this is a map.
    pub fn entries(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// The entries for modification, when this is a map.
    pub fn entries_mut(&mut self) -> Option<&mut Vec<(String, Value)>> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// Look a key up, ignoring case.
    ///
    /// Case matters here: Steam has shipped both `AppName` and `appname`, and
    /// different third-party tools picked different spellings, so a case-sensitive
    /// lookup finds nothing on somebody else's file and silently treats an existing
    /// shortcut as absent.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries()?
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }

    /// The string at `key`, ignoring case.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// The `i32` at `key`, ignoring case.
    pub fn get_i32(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            Value::Int32(number) => Some(*number),
            _ => None,
        }
    }

    /// Set `key`, keeping the spelling and position already in the file when it is
    /// there, and appending when it is not. Preserving the existing spelling is
    /// what keeps a rewrite from turning Steam's `AppName` into our `appname`.
    pub fn set(&mut self, key: &str, value: Value) {
        let Some(entries) = self.entries_mut() else {
            return;
        };

        match entries
            .iter_mut()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        {
            Some((_, existing)) => *existing = value,
            None => entries.push((key.to_string(), value)),
        }
    }
}

/// Parse a binary VDF document: the top-level key/value pairs, terminated by the
/// closing `0x08`.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    let mut reader = Reader { bytes, at: 0 };
    let entries = reader.read_map_body("the document")?;

    if reader.at != bytes.len() {
        return Err(malformed(format!(
            "{} trailing bytes after the end of the document at offset {}",
            bytes.len() - reader.at,
            reader.at
        )));
    }

    Ok(Value::Map(entries))
}

/// Serialize a document parsed by [`parse`].
pub fn write(document: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let entries = document
        .entries()
        .ok_or_else(|| malformed("the document root has to be a map"))?;

    write_map_body(entries, &mut out)?;

    Ok(out)
}

fn write_map_body(entries: &[(String, Value)], out: &mut Vec<u8>) -> Result<()> {
    for (key, value) in entries {
        match value {
            Value::Map(children) => {
                out.push(TAG_MAP);
                write_key(key, out)?;
                write_map_body(children, out)?;
            }
            Value::String(text) => {
                out.push(TAG_STRING);
                write_key(key, out)?;
                write_key(text, out)?;
            }
            Value::Int32(number) => {
                out.push(TAG_INT32);
                write_key(key, out)?;
                out.extend_from_slice(&number.to_le_bytes());
            }
            Value::UInt64(number) => {
                out.push(TAG_UINT64);
                write_key(key, out)?;
                out.extend_from_slice(&number.to_le_bytes());
            }
        }
    }
    out.push(TAG_END);

    Ok(())
}

/// Write a NUL-terminated string. An interior NUL would silently truncate the
/// field when Steam read it back, so it is refused instead.
fn write_key(text: &str, out: &mut Vec<u8>) -> Result<()> {
    if text.as_bytes().contains(&0) {
        return Err(malformed(format!(
            "{text:?} contains a NUL byte and cannot be stored in a VDF field"
        )));
    }
    out.extend_from_slice(text.as_bytes());
    out.push(0);

    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    /// Read key/value pairs until the closing `0x08`.
    fn read_map_body(&mut self, context: &str) -> Result<Vec<(String, Value)>> {
        let mut entries = Vec::new();

        loop {
            let tag = self.take_byte(context)?;
            if tag == TAG_END {
                return Ok(entries);
            }

            let key = self.take_string(context)?;
            let value = match tag {
                TAG_MAP => Value::Map(self.read_map_body(&key)?),
                TAG_STRING => Value::String(self.take_string(&key)?),
                TAG_INT32 => Value::Int32(i32::from_le_bytes(self.take_array(&key)?)),
                TAG_UINT64 => Value::UInt64(u64::from_le_bytes(self.take_array(&key)?)),
                other => {
                    return Err(malformed(format!(
                        "unsupported value type {other:#04x} for key {key:?} at offset {}; \
                         refusing to touch the file rather than guessing at its length",
                        self.at
                    )));
                }
            };

            entries.push((key, value));
        }
    }

    fn take_byte(&mut self, context: &str) -> Result<u8> {
        let byte = self.bytes.get(self.at).copied().ok_or_else(|| {
            malformed(format!(
                "the file ends inside {context}, with no closing 0x08"
            ))
        })?;
        self.at += 1;

        Ok(byte)
    }

    fn take_string(&mut self, context: &str) -> Result<String> {
        let end = self.bytes[self.at..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| {
                malformed(format!(
                    "the file ends inside a string in {context}, with no NUL terminator"
                ))
            })?;

        let raw = &self.bytes[self.at..self.at + end];
        self.at += end + 1;

        // Refusing beats a lossy conversion: replacing an unreadable byte would
        // write the replacement back and corrupt somebody else's entry.
        String::from_utf8(raw.to_vec()).map_err(|_| {
            malformed(format!(
                "a string in {context} is not valid UTF-8; gamestore will not \
                 rewrite this file rather than risk corrupting it"
            ))
        })
    }

    fn take_array<const N: usize>(&mut self, context: &str) -> Result<[u8; N]> {
        let slice = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or_else(|| malformed(format!("the file ends inside the value of {context:?}")))?;
        self.at += N;

        Ok(slice.try_into().expect("slice length checked above"))
    }
}

fn malformed(detail: impl Into<String>) -> Error {
    Error::Vdf(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document shaped like a real `shortcuts.vdf`, including a key type we do
    /// not manage (`UInt64`) and a nested `tags` map.
    fn sample() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(TAG_MAP);
        bytes.extend_from_slice(b"shortcuts\0");

        bytes.push(TAG_MAP);
        bytes.extend_from_slice(b"0\0");
        bytes.push(TAG_INT32);
        bytes.extend_from_slice(b"appid\0");
        bytes.extend_from_slice(&(-1234_i32).to_le_bytes());
        bytes.push(TAG_STRING);
        bytes.extend_from_slice("AppName\0Br\u{fc}tal Legend\0".as_bytes());
        bytes.push(TAG_STRING);
        bytes.extend_from_slice(b"Exe\0\"/games/bl/bl.exe\"\0");
        bytes.push(TAG_UINT64);
        bytes.extend_from_slice(b"SomethingValveAddedLater\0");
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        bytes.push(TAG_MAP);
        bytes.extend_from_slice(b"tags\0");
        bytes.push(TAG_STRING);
        bytes.extend_from_slice(b"0\0favourite\0");
        bytes.push(TAG_END); // tags
        bytes.push(TAG_END); // shortcut 0

        bytes.push(TAG_END); // shortcuts
        bytes.push(TAG_END); // document

        bytes
    }

    #[test]
    fn a_document_round_trips_byte_for_byte() {
        // The one that matters: whatever else changes, reading somebody else's file
        // and writing it back unmodified must not alter a single byte.
        let original = sample();

        let reparsed = write(&parse(&original).unwrap()).unwrap();

        assert_eq!(reparsed, original);
    }

    #[test]
    fn every_key_and_type_survives_the_trip() {
        let document = parse(&sample()).unwrap();
        let shortcut = document.get("shortcuts").unwrap().get("0").unwrap();

        assert_eq!(shortcut.get_i32("appid"), Some(-1234));
        assert_eq!(shortcut.get_str("AppName"), Some("Brütal Legend"));
        assert_eq!(shortcut.get_str("Exe"), Some("\"/games/bl/bl.exe\""));
        assert_eq!(
            shortcut.get("SomethingValveAddedLater"),
            Some(&Value::UInt64(u64::MAX)),
            "a type we do not manage still has to survive"
        );
        assert_eq!(
            shortcut.get("tags").unwrap().get_str("0"),
            Some("favourite")
        );
    }

    #[test]
    fn lookups_ignore_case_because_steam_has_shipped_both_spellings() {
        let document = parse(&sample()).unwrap();
        let shortcut = document.get("SHORTCUTS").unwrap().get("0").unwrap();

        assert_eq!(shortcut.get_str("appname"), shortcut.get_str("AppName"));
        assert_eq!(shortcut.get_str("EXE"), Some("\"/games/bl/bl.exe\""));
    }

    #[test]
    fn setting_a_key_keeps_the_spelling_and_the_position_already_in_the_file() {
        let mut document = parse(&sample()).unwrap();
        let shortcuts = document.entries_mut().unwrap();
        let shortcut = &mut shortcuts[0].1.entries_mut().unwrap()[0].1;

        shortcut.set("appname", Value::String("Renamed".to_string()));

        let keys: Vec<&str> = shortcut
            .entries()
            .unwrap()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "appid",
                "AppName",
                "Exe",
                "SomethingValveAddedLater",
                "tags"
            ],
            "the original spelling and order have to survive a set"
        );
        assert_eq!(shortcut.get_str("AppName"), Some("Renamed"));
    }

    #[test]
    fn setting_an_absent_key_appends_it() {
        let mut document = parse(&sample()).unwrap();
        let shortcut = &mut document.entries_mut().unwrap()[0].1.entries_mut().unwrap()[0].1;

        shortcut.set("LaunchOptions", Value::String("-windowed".to_string()));

        assert_eq!(shortcut.get_str("LaunchOptions"), Some("-windowed"));
        assert_eq!(
            shortcut.entries().unwrap().last().unwrap().0,
            "LaunchOptions"
        );
    }

    #[test]
    fn an_empty_document_is_just_the_end_marker() {
        let document = parse(&[TAG_END]).unwrap();

        assert_eq!(document, Value::Map(Vec::new()));
        assert_eq!(write(&document).unwrap(), vec![TAG_END]);
    }

    #[test]
    fn a_truncated_file_is_an_error_rather_than_a_panic() {
        let full = sample();

        for cut in 1..full.len() {
            let result = parse(&full[..cut]);
            if let Ok(parsed) = result {
                // A prefix can only be valid if it happens to be a whole document,
                // which for this sample means nothing short of the full thing.
                assert_eq!(
                    write(&parsed).unwrap().len(),
                    cut,
                    "a {cut}-byte prefix parsed as something that is not itself"
                );
            }
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = sample();
        bytes.push(TAG_END);

        let error = parse(&bytes).unwrap_err();

        assert!(error.to_string().contains("trailing"), "{error}");
    }

    #[test]
    fn an_unsupported_type_is_refused_by_name_rather_than_guessed_at() {
        // 0x03 is a float in binary VDF. Nothing writes one into shortcuts.vdf
        // today, but guessing its width would desynchronise the whole parse and
        // corrupt everything after it.
        let bytes = vec![0x03, b'f', b'p', 0, 0, 0, 0, 0, TAG_END];

        let error = parse(&bytes).unwrap_err();

        assert!(error.to_string().contains("0x03"), "{error}");
        assert!(error.to_string().contains("refusing"), "{error}");
    }

    #[test]
    fn invalid_utf8_is_refused_rather_than_replaced() {
        let bytes = vec![TAG_STRING, b'k', 0, 0xFF, 0xFE, 0, TAG_END];

        let error = parse(&bytes).unwrap_err();

        assert!(error.to_string().contains("UTF-8"), "{error}");
    }

    #[test]
    fn a_nul_inside_a_value_is_refused_on_write() {
        let document = Value::Map(vec![("k".to_string(), Value::String("a\0b".to_string()))]);

        let error = write(&document).unwrap_err();

        assert!(error.to_string().contains("NUL"), "{error}");
    }
}
