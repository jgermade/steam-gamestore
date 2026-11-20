//! Steam's text KeyValues format: `config.vdf`, `libraryfolders.vdf`,
//! `compatibilitytool.vdf`.
//!
//! The binary format of `shortcuts.vdf` is [`crate::vdf`]; this is the other one.
//! It matters for the `compat` task: the Proton mapping of a shortcut lives in
//! `config/config.vdf`, a file holding a great deal of Steam's own state, so
//! writing it back must not disturb anything gamestore did not mean to change.
//!
//! Values are kept exactly as they are spelled between the quotes, escapes
//! included, and [`write`] reproduces the layout Steam writes (a tab per level,
//! two tabs between a key and its value). A file Steam wrote therefore comes back
//! byte for byte, which a test asserts against a real one. Files Steam did not
//! write, with spaces or comments, are read but would be normalised on writing;
//! gamestore never writes those.

use crate::{Error, Result};

/// A node: a quoted value, or a block of further pairs in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// The raw text between the quotes. Backslash escapes are left as written.
    Value(String),
    Map(Vec<(String, Node)>),
}

impl Node {
    /// The pairs of a block.
    pub fn entries(&self) -> Option<&[(String, Node)]> {
        match self {
            Node::Map(entries) => Some(entries),
            Node::Value(_) => None,
        }
    }

    /// The pairs of a block, for changing.
    pub fn entries_mut(&mut self) -> Option<&mut Vec<(String, Node)>> {
        match self {
            Node::Map(entries) => Some(entries),
            Node::Value(_) => None,
        }
    }

    /// The child under `key`. Steam treats keys case-insensitively.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries()?
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, node)| node)
    }

    /// The child under `key`, for changing.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        self.entries_mut()?
            .iter_mut()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, node)| node)
    }

    /// Follow a path of keys.
    pub fn path(&self, keys: &[&str]) -> Option<&Node> {
        keys.iter().try_fold(self, |node, key| node.get(key))
    }

    /// Follow a path of keys, for changing.
    pub fn path_mut(&mut self, keys: &[&str]) -> Option<&mut Node> {
        keys.iter().try_fold(self, |node, key| node.get_mut(key))
    }

    /// The value of a leaf, unescaped.
    pub fn as_str(&self) -> Option<String> {
        match self {
            Node::Value(raw) => Some(unescape(raw)),
            Node::Map(_) => None,
        }
    }

    /// Replace the child under `key`, or append it when there is none.
    pub fn set(&mut self, key: &str, node: Node) {
        if let Some(entries) = self.entries_mut() {
            match entries
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
            {
                Some((_, existing)) => *existing = node,
                None => entries.push((key.to_string(), node)),
            }
        }
    }

    /// A leaf holding `text`, escaped the way Steam escapes it.
    pub fn value(text: &str) -> Node {
        Node::Value(escape(text))
    }
}

/// Parse a document: one or more top-level pairs, returned as a block.
pub fn parse(text: &str) -> Result<Node> {
    let mut tokens = Tokens {
        text: text.trim_start_matches('\u{feff}'),
        at: 0,
    };
    let entries = parse_entries(&mut tokens, false)?;
    Ok(Node::Map(entries))
}

/// Serialize a block the way Steam writes it.
pub fn write(document: &Node) -> String {
    let mut out = String::new();
    if let Node::Map(entries) = document {
        write_entries(&mut out, entries, 0);
    }
    out
}

fn write_entries(out: &mut String, entries: &[(String, Node)], depth: usize) {
    let indent = "\t".repeat(depth);
    for (key, node) in entries {
        match node {
            Node::Value(raw) => {
                out.push_str(&format!("{indent}\"{key}\"\t\t\"{raw}\"\n"));
            }
            Node::Map(children) => {
                out.push_str(&format!("{indent}\"{key}\"\n{indent}{{\n"));
                write_entries(out, children, depth + 1);
                out.push_str(&format!("{indent}}}\n"));
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Text(String),
    Open,
    Close,
}

struct Tokens<'a> {
    text: &'a str,
    at: usize,
}

impl Tokens<'_> {
    fn error(&self, problem: &str) -> Error {
        let line = self.text[..self.at.min(self.text.len())]
            .lines()
            .count()
            .max(1);
        Error::Vdf(format!("{problem} at line {line}"))
    }

    fn next(&mut self) -> Result<Option<Token>> {
        let bytes = self.text.as_bytes();
        loop {
            while self.at < bytes.len() && bytes[self.at].is_ascii_whitespace() {
                self.at += 1;
            }
            if self.text[self.at..].starts_with("//") {
                self.at += self.text[self.at..]
                    .find('\n')
                    .unwrap_or(bytes.len() - self.at);
                continue;
            }
            break;
        }
        let Some(&byte) = bytes.get(self.at) else {
            return Ok(None);
        };
        match byte {
            b'{' => {
                self.at += 1;
                Ok(Some(Token::Open))
            }
            b'}' => {
                self.at += 1;
                Ok(Some(Token::Close))
            }
            b'"' => {
                let start = self.at + 1;
                let mut index = start;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            self.at = index + 1;
                            return Ok(Some(Token::Text(self.text[start..index].to_string())));
                        }
                        _ => index += 1,
                    }
                }
                Err(self.error("unterminated string"))
            }
            _ => {
                let start = self.at;
                while self.at < bytes.len()
                    && !bytes[self.at].is_ascii_whitespace()
                    && !matches!(bytes[self.at], b'{' | b'}' | b'"')
                {
                    self.at += 1;
                }
                Ok(Some(Token::Text(self.text[start..self.at].to_string())))
            }
        }
    }
}

fn parse_entries(tokens: &mut Tokens, nested: bool) -> Result<Vec<(String, Node)>> {
    let mut entries = Vec::new();
    loop {
        let key = match tokens.next()? {
            Some(Token::Text(key)) => key,
            Some(Token::Close) if nested => return Ok(entries),
            None if !nested => return Ok(entries),
            None => return Err(tokens.error("missing `}`")),
            Some(_) => return Err(tokens.error("expected a key")),
        };
        let node = match tokens.next()? {
            Some(Token::Text(value)) => Node::Value(value),
            Some(Token::Open) => Node::Map(parse_entries(tokens, true)?),
            _ => return Err(tokens.error(&format!("key {key:?} has no value"))),
        };
        entries.push((key, node));
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a real `config/config.vdf` as Steam wrote it on the mini PC,
    /// cut down to the parts gamestore reads and writes, with a Windows-style path
    /// to exercise escapes.
    const CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"AutoUpdateWindowEnabled\"\t\t\"0\"\n\t\t\t\t\"BaseInstallFolder_1\"\t\t\"D:\\\\Games\\\\Steam\"\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_11\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"75\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"system\"\n\t{\n\t\t\"Bluetooth\"\n\t\t{\n\t\t\t\"Enabled\"\t\t\"0\"\n\t\t}\n\t}\n}\n";

    #[test]
    fn a_file_steam_wrote_comes_back_byte_for_byte() {
        assert_eq!(write(&parse(CONFIG).unwrap()), CONFIG);
    }

    #[test]
    fn values_are_found_case_insensitively_and_unescaped() {
        let document = parse(CONFIG).unwrap();
        let steam = document
            .path(&["InstallConfigStore", "software", "valve", "steam"])
            .unwrap();
        assert_eq!(
            steam.get("BaseInstallFolder_1").unwrap().as_str().unwrap(),
            "D:\\Games\\Steam"
        );
        assert_eq!(
            steam
                .path(&["CompatToolMapping", "0", "name"])
                .unwrap()
                .as_str()
                .unwrap(),
            "proton_11"
        );
    }

    #[test]
    fn set_replaces_in_place_and_appends_what_is_new() {
        let mut document = parse(CONFIG).unwrap();
        let mapping = document
            .path_mut(&[
                "InstallConfigStore",
                "Software",
                "Valve",
                "Steam",
                "CompatToolMapping",
            ])
            .unwrap();
        mapping.set(
            "2907567421",
            Node::Map(vec![("name".to_string(), Node::value("proton_11"))]),
        );
        let written = write(&document);

        assert!(written.contains(
            "\t\t\t\t\t}\n\t\t\t\t\t\"2907567421\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_11\"\n\t\t\t\t\t}\n"
        ), "{written}");
        assert_eq!(parse(&written).unwrap(), document);
    }

    #[test]
    fn hand_written_files_with_spaces_and_comments_are_read() {
        let manifest = "// a comment\n\"manifest\"\n{\n  \"version\" \"2\"\n  \"commandline\" \"/proton %verb%\"\n}\n";
        let document = parse(manifest).unwrap();
        assert_eq!(
            document
                .path(&["manifest", "commandline"])
                .unwrap()
                .as_str()
                .unwrap(),
            "/proton %verb%"
        );
    }

    #[test]
    fn escapes_survive_a_set() {
        let mut document = Node::Map(Vec::new());
        document.set("path", Node::value("C:\\a \"b\""));
        let written = write(&document);
        assert_eq!(written, "\"path\"\t\t\"C:\\\\a \\\"b\\\"\"\n");
        assert_eq!(
            parse(&written)
                .unwrap()
                .get("path")
                .unwrap()
                .as_str()
                .unwrap(),
            "C:\\a \"b\""
        );
    }

    #[test]
    fn truncated_documents_are_errors_not_panics() {
        assert!(parse("\"a\"\n{\n\t\"b\"\t\t\"c\"\n").is_err());
        assert!(parse("\"a\"\t\t\"unterminated").is_err());
        assert!(parse("\"a\"").is_err());
    }
}
