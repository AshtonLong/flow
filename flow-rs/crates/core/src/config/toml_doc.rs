//! A position-preserving scan of a TOML document. The `toml` crate gives us the
//! values; this gives us where each of them sits in the source text, which is
//! what in-place patching and "which line is wrong" both need.
//!
//! The scanner assumes the text already parses (callers parse it first), so it
//! is tolerant rather than strict. Offsets are byte offsets into the source and
//! always fall on character boundaries.
//!
//! PORT: src/main/config/toml-doc.ts

use std::collections::HashMap;

use super::{ConfigPath, PathSeg};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    String,
    Bare,
    Array,
    Inline,
}

#[derive(Debug, Clone)]
pub struct ValueNode {
    pub kind: ValueKind,
    /// Offset of the first character of the value token.
    pub start: usize,
    /// Offset just past the last character of the value token.
    pub end: usize,
    /// Elements, for `Array`.
    pub items: Vec<ValueNode>,
    /// Key/value pairs, for `Inline` tables.
    pub entries: Vec<KeyValueNode>,
}

#[derive(Debug, Clone)]
pub struct KeyValueNode {
    /// Decoded key segments: `insert.paste_shortcut` → `["insert", "paste_shortcut"]`.
    pub key: Vec<String>,
    pub key_start: usize,
    pub value: ValueNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub keys: Vec<String>,
    pub array: bool,
}

#[derive(Debug, Clone)]
pub struct SectionNode {
    /// Absolute path, with list indexes: the second `[[profiles]]` is `["profiles", 1]`.
    pub path: ConfigPath,
    /// `None` for the root section (everything before the first header).
    pub header: Option<Header>,
    /// Offset of the start of the header line; 0 for the root section.
    pub start: usize,
    /// Offset just past the header line; 0 for the root section.
    pub body_start: usize,
    /// Offset of the start of the next header line, or the end of the text.
    pub end: usize,
    pub entries: Vec<KeyValueNode>,
}

/// Where a value lives, which decides how it is replaced or removed.
#[derive(Debug, Clone, Copy)]
pub enum Owner<'a> {
    Section { section: &'a SectionNode, entry: &'a KeyValueNode },
    /// `index` is the entry's position in `table.entries`.
    Inline { table: &'a ValueNode, entry: &'a KeyValueNode, index: usize },
    Array { array: &'a ValueNode, index: usize },
}

impl Owner<'_> {
    pub fn is_section(&self) -> bool {
        matches!(self, Owner::Section { .. })
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Owner::Array { .. })
    }
}

#[derive(Debug, Clone)]
pub struct Located<'a> {
    pub path: ConfigPath,
    pub node: &'a ValueNode,
    pub owner: Owner<'a>,
}

impl Located<'_> {
    /// Where the value's line is: its key, or the element itself inside an array.
    pub fn anchor(&self) -> usize {
        match self.owner {
            Owner::Array { .. } => self.node.start,
            Owner::Section { entry, .. } | Owner::Inline { entry, .. } => entry.key_start,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TomlDoc<'s> {
    pub source: &'s str,
    /// Line ending used by the document: `\r\n` if any is present, else `\n`.
    pub eol: &'static str,
    /// The root section first, then one per `[header]` in file order.
    pub sections: Vec<SectionNode>,
}

impl TomlDoc<'_> {
    /// Every value in the document with its absolute path, in file order.
    pub fn values(&self) -> Vec<Located<'_>> {
        let mut out = Vec::new();
        for section in &self.sections {
            for entry in &section.entries {
                let mut path = section.path.clone();
                path.extend(entry.key.iter().cloned().map(PathSeg::Key));
                collect_value(path, &entry.value, Owner::Section { section, entry }, &mut out);
            }
        }
        out
    }
}

const BARE_VALUE_END: &[u8] = b",]}#\n\r";
const KEY_END: &[u8] = b" \t.=]\r\n#";

struct Scanner<'s> {
    s: &'s str,
    b: &'s [u8],
    pos: usize,
}

impl<'s> Scanner<'s> {
    fn new(s: &'s str) -> Self {
        Self { s, b: s.as_bytes(), pos: 0 }
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.b.get(i).copied()
    }

    /// Offset just past the character at `i` (one step forward, never into the
    /// middle of a multi-byte character).
    fn next_char(&self, i: usize) -> usize {
        let mut j = i + 1;
        while j < self.b.len() && !self.s.is_char_boundary(j) {
            j += 1;
        }
        j
    }

    fn scan(mut self) -> Vec<SectionNode> {
        let len = self.b.len();
        let mut sections = vec![SectionNode {
            path: Vec::new(),
            header: None,
            start: 0,
            body_start: 0,
            end: 0,
            entries: Vec::new(),
        }];
        // Number of items seen so far for each array of tables, keyed by its path.
        let mut counts: HashMap<ConfigPath, usize> = HashMap::new();

        while self.pos < len {
            self.skip_blank();
            let Some(c) = self.at(self.pos) else { break };
            if c == b'\n' || c == b'\r' {
                self.pos += 1;
            } else if c == b'#' {
                self.pos = self.line_end(self.pos);
            } else if c == b'[' {
                let line_start = line_start_of(self.s, self.pos);
                let array = self.at(self.pos + 1) == Some(b'[');
                self.pos += if array { 2 } else { 1 };
                let keys = self.key();
                let line_end = self.line_end(self.pos);
                self.pos = if line_end < len { line_end + 1 } else { line_end };
                if let Some(current) = sections.last_mut() {
                    current.end = line_start;
                }
                sections.push(SectionNode {
                    path: resolve_header(&keys, array, &mut counts),
                    header: Some(Header { keys, array }),
                    start: line_start,
                    body_start: self.pos,
                    end: len,
                    entries: Vec::new(),
                });
            } else {
                match self.key_value() {
                    Some(entry) => {
                        if let Some(current) = sections.last_mut() {
                            current.entries.push(entry);
                        }
                    }
                    None => self.pos = self.line_end(self.pos),
                }
            }
        }
        if let Some(current) = sections.last_mut() {
            current.end = len;
        }
        sections
    }

    fn skip_blank(&mut self) {
        while matches!(self.at(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    /// Skips whitespace, newlines and comments (inside arrays and inline tables).
    fn skip_trivia(&mut self) {
        loop {
            match self.at(self.pos) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.pos += 1,
                Some(b'#') => self.pos = self.line_end(self.pos),
                _ => return,
            }
        }
    }

    /// Offset of the newline that ends the line containing `from`, or the text length.
    fn line_end(&self, from: usize) -> usize {
        let from = from.min(self.b.len());
        self.b[from..].iter().position(|&b| b == b'\n').map_or(self.b.len(), |i| from + i)
    }

    fn key_value(&mut self) -> Option<KeyValueNode> {
        let key_start = self.pos;
        let key = self.key();
        self.skip_blank();
        if key.is_empty() || self.at(self.pos) != Some(b'=') {
            return None;
        }
        self.pos += 1;
        self.skip_blank();
        Some(KeyValueNode { key, key_start, value: self.value() })
    }

    /// Reads a possibly dotted, possibly quoted key and leaves `pos` after it.
    fn key(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        loop {
            self.skip_blank();
            let c = self.at(self.pos);
            let start = self.pos;
            if matches!(c, Some(b'"' | b'\'')) {
                self.string();
                parts.push(decode_string(&self.s[start..self.pos]));
            } else {
                while self.pos < self.b.len() && !KEY_END.contains(&self.b[self.pos]) {
                    self.pos += 1;
                }
                if self.pos == start {
                    break;
                }
                parts.push(self.s[start..self.pos].to_string());
            }
            self.skip_blank();
            if self.at(self.pos) != Some(b'.') {
                break;
            }
            self.pos += 1;
        }
        parts
    }

    fn value(&mut self) -> ValueNode {
        let start = self.pos;
        let c = self.at(self.pos);
        if matches!(c, Some(b'"' | b'\'')) {
            self.string();
            return ValueNode { kind: ValueKind::String, start, end: self.pos, items: Vec::new(), entries: Vec::new() };
        }
        if c == Some(b'[') {
            self.pos += 1;
            let mut items = Vec::new();
            loop {
                self.skip_trivia();
                if self.pos >= self.b.len() {
                    break;
                }
                if self.b[self.pos] == b']' {
                    self.pos += 1;
                    break;
                }
                if self.b[self.pos] == b',' {
                    self.pos += 1;
                    continue;
                }
                let before = self.pos;
                items.push(self.value());
                if self.pos == before {
                    // Malformed input: always make progress.
                    self.pos = self.next_char(self.pos);
                }
            }
            return ValueNode { kind: ValueKind::Array, start, end: self.pos, items, entries: Vec::new() };
        }
        if c == Some(b'{') {
            self.pos += 1;
            let mut entries = Vec::new();
            loop {
                self.skip_trivia();
                if self.pos >= self.b.len() {
                    break;
                }
                if self.b[self.pos] == b'}' {
                    self.pos += 1;
                    break;
                }
                if self.b[self.pos] == b',' {
                    self.pos += 1;
                    continue;
                }
                let before = self.pos;
                if let Some(entry) = self.key_value() {
                    entries.push(entry);
                }
                if self.pos == before {
                    self.pos = self.next_char(self.pos);
                }
            }
            return ValueNode { kind: ValueKind::Inline, start, end: self.pos, items: Vec::new(), entries };
        }
        while self.pos < self.b.len() && !BARE_VALUE_END.contains(&self.b[self.pos]) {
            self.pos += 1;
        }
        // Dates may contain a space, so trim instead of stopping at whitespace.
        let mut end = self.pos;
        while end > start && matches!(self.b[end - 1], b' ' | b'\t') {
            end -= 1;
        }
        ValueNode { kind: ValueKind::Bare, start, end, items: Vec::new(), entries: Vec::new() }
    }

    /// Moves `pos` past the string that starts at `pos`.
    fn string(&mut self) {
        let len = self.b.len();
        let quote = self.b[self.pos];
        let basic = quote == b'"';
        let triple = [quote; 3];
        if self.b[self.pos..].starts_with(&triple) {
            self.pos += 3;
            while self.pos < len {
                if basic && self.b[self.pos] == b'\\' {
                    self.pos = self.next_char(self.pos + 1);
                } else if self.b[self.pos..].starts_with(&triple) {
                    self.pos += 3;
                    // Up to two quotes may sit directly before the closing delimiter.
                    let mut extra = 0;
                    while extra < 2 && self.at(self.pos) == Some(quote) {
                        self.pos += 1;
                        extra += 1;
                    }
                    return;
                } else {
                    self.pos = self.next_char(self.pos);
                }
            }
            self.pos = self.pos.min(len);
            return;
        }
        self.pos += 1;
        while self.pos < len {
            let c = self.b[self.pos];
            if basic && c == b'\\' {
                self.pos = self.next_char(self.pos + 1);
            } else if c == quote {
                self.pos += 1;
                return;
            } else if c == b'\n' {
                return;
            } else {
                self.pos = self.next_char(self.pos);
            }
        }
        self.pos = self.pos.min(len);
    }
}

/// Decodes a quoted key by letting the real parser handle the escapes.
fn decode_string(raw: &str) -> String {
    if let Ok(table) = format!("k = {raw}").parse::<toml::Table>()
        && let Some(toml::Value::String(value)) = table.get("k")
    {
        return value.clone();
    }
    // Drop the first and the last character, like `raw.slice(1, -1)`.
    let mut chars = raw.chars();
    chars.next();
    chars.next_back();
    chars.as_str().to_string()
}

/// Turns header keys into an absolute path. A header that passes through an
/// array of tables refers to its most recent item, so `[profiles.cleanup]`
/// after the second `[[profiles]]` is `["profiles", 1, "cleanup"]`.
fn resolve_header(keys: &[String], array: bool, counts: &mut HashMap<ConfigPath, usize>) -> ConfigPath {
    let mut path: ConfigPath = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        path.push(PathSeg::Key(key.clone()));
        let last = i == keys.len() - 1;
        if last && array {
            let index = counts.get(&path).copied().unwrap_or(0);
            counts.insert(path.clone(), index + 1);
            path.push(PathSeg::Index(index));
        } else if !last && let Some(&seen) = counts.get(&path) {
            path.push(PathSeg::Index(seen.saturating_sub(1)));
        }
    }
    path
}

fn collect_value<'a>(path: ConfigPath, node: &'a ValueNode, owner: Owner<'a>, out: &mut Vec<Located<'a>>) {
    out.push(Located { path: path.clone(), node, owner });
    match node.kind {
        ValueKind::Inline => {
            for (index, entry) in node.entries.iter().enumerate() {
                let mut child = path.clone();
                child.extend(entry.key.iter().cloned().map(PathSeg::Key));
                collect_value(child, &entry.value, Owner::Inline { table: node, entry, index }, out);
            }
        }
        ValueKind::Array => {
            for (index, item) in node.items.iter().enumerate() {
                let mut child = path.clone();
                child.push(PathSeg::Index(index));
                collect_value(child, item, Owner::Array { array: node, index }, out);
            }
        }
        _ => {}
    }
}

/// Scans TOML source into sections and located values.
pub fn scan_toml(source: &str) -> TomlDoc<'_> {
    let sections = Scanner::new(source).scan();
    TomlDoc { source, eol: if source.contains("\r\n") { "\r\n" } else { "\n" }, sections }
}

pub fn path_equals(a: &[PathSeg], b: &[PathSeg]) -> bool {
    a == b
}

/// True when `prefix` is the start of `path` (or equal to it).
pub fn is_path_prefix(prefix: &[PathSeg], path: &[PathSeg]) -> bool {
    path.starts_with(prefix)
}

/// The value defined at exactly `path`, if it is written as a single token.
pub fn find_value<'v, 'a>(values: &'v [Located<'a>], path: &[PathSeg]) -> Option<&'v Located<'a>> {
    values.iter().find(|located| located.path == path)
}

/// Offset of the start of the line containing `offset`.
pub fn line_start_of(source: &str, offset: usize) -> usize {
    let end = offset.min(source.len());
    source.as_bytes()[..end].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1)
}

/// Offset just past the newline that ends the line containing `offset`.
pub fn line_end_after(source: &str, offset: usize) -> usize {
    let from = offset.min(source.len());
    source.as_bytes()[from..].iter().position(|&b| b == b'\n').map_or(source.len(), |i| from + i + 1)
}

fn line_number_at(source: &str, offset: usize) -> u32 {
    let end = offset.min(source.len());
    let newlines = source.as_bytes()[..end].iter().filter(|&&b| b == b'\n').count();
    u32::try_from(newlines + 1).unwrap_or(u32::MAX)
}

/// Maps a config path to its 1-based line in a source text. A path that is not
/// written out (a missing required key, say) resolves to the nearest enclosing
/// key or table header.
pub struct LineLocator<'s> {
    source: &'s str,
    /// Every located value's path and the offset of its line, in file order.
    values: Vec<(ConfigPath, usize)>,
    /// Every `[header]`'s path and start, in file order.
    headers: Vec<(ConfigPath, usize)>,
}

impl LineLocator<'_> {
    pub fn locate(&self, path: &[PathSeg]) -> Option<u32> {
        for length in (1..=path.len()).rev() {
            let prefix = &path[..length];
            if let Some((_, offset)) = self.values.iter().find(|(p, _)| p.as_slice() == prefix) {
                return Some(line_number_at(self.source, *offset));
            }
            if let Some((_, start)) = self.headers.iter().find(|(p, _)| p.as_slice() == prefix) {
                return Some(line_number_at(self.source, *start));
            }
            // A table spelled with dotted keys has no line of its own: use its first key.
            if let Some((_, offset)) = self.values.iter().find(|(p, _)| is_path_prefix(prefix, p)) {
                return Some(line_number_at(self.source, *offset));
            }
            if let Some((_, start)) = self.headers.iter().find(|(p, _)| is_path_prefix(prefix, p)) {
                return Some(line_number_at(self.source, *start));
            }
        }
        None
    }
}

/// Returns a locator that maps a config path to its 1-based line in `source`.
pub fn create_line_locator(source: &str) -> LineLocator<'_> {
    let doc = scan_toml(source);
    let values = doc.values().iter().map(|located| (located.path.clone(), located.anchor())).collect();
    let headers = doc
        .sections
        .iter()
        .filter(|section| section.header.is_some())
        .map(|section| (section.path.clone(), section.start))
        .collect();
    LineLocator { source, values, headers }
}

/// The 1-based line of `path` in `source`, when it can be located.
pub fn locate_line(source: &str, path: &[PathSeg]) -> Option<u32> {
    create_line_locator(source).locate(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn scans_sections_and_values_with_list_indexes() {
        let source = "a = 1\n[[p]]\nname = \"x\"\n[p.sub]\nk = [1, { q = 2 }]\n\n[[p]]\nname = 'y'\n";
        let doc = scan_toml(source);
        let paths: Vec<ConfigPath> = doc.sections.iter().map(|s| s.path.clone()).collect();
        assert_eq!(paths, vec![path![], path!["p", 0usize], path!["p", 0usize, "sub"], path!["p", 1usize]]);
        let values = doc.values();
        let located: Vec<String> =
            values.iter().map(|v| format!("{:?}={}", v.path, &source[v.node.start..v.node.end])).collect();
        assert!(located.contains(&format!("{:?}=2", path!["p", 0usize, "sub", "k", 1usize, "q"])));
        assert!(located.contains(&format!("{:?}='y'", path!["p", 1usize, "name"])));
    }

    #[test]
    fn locates_lines() {
        let source = "audio.sounds = 5\ninsert = { method = \"paste\",\n  paste_shortcut = 7 }\n\n[[dictionary]]\nheard = [\"a\"]\n";
        let line = |path: ConfigPath| locate_line(source, &path);
        assert_eq!(line(path!["audio", "sounds"]), Some(1));
        assert_eq!(line(path!["insert", "paste_shortcut"]), Some(3));
        assert_eq!(line(path!["dictionary", 0usize, "write"]), Some(5));
        assert_eq!(line(path!["nothing"]), None);
    }

    #[test]
    fn decodes_quoted_keys() {
        let doc = scan_toml("\"a b\".'c' = 1\n");
        assert_eq!(doc.sections[0].entries[0].key, vec!["a b".to_string(), "c".to_string()]);
    }
}
