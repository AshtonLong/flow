//! In-place edits of `config.toml`. The settings window changes values through
//! here so that hand-written comments, ordering and formatting survive: only the
//! tokens that have to change are touched.
//!
//! PORT: src/main/config/patch.ts

use toml::Value;

use super::toml_doc::{
    Located, Owner, SectionNode, TomlDoc, ValueKind, find_value, is_path_prefix, line_end_after, line_start_of,
    scan_toml,
};
use super::toml_parse::{TomlSyntaxError, parse_toml};
use super::toml_write::{
    Table, deep_equal_opt, format_key, format_key_path, format_table_lines, format_value, get_at, normalise_value,
    remove_at, set_at, set_in_table, tables_equal,
};
use super::{ConfigPatch, PathSeg};

/// Top-level lists written as `[[name]]` blocks. An empty one is simply absent from the file.
const LIST_KEYS: [&str; 4] = ["dictionary", "snippets", "profiles", "local_models"];

/// Why a patch could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TomlPatchError {
    /// The text being patched is not valid TOML.
    #[error(transparent)]
    Syntax(#[from] TomlSyntaxError),
    /// A patch that cannot be applied, such as one addressing a list item that
    /// does not exist (the TypeScript `TomlPatchError`).
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, TomlPatchError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    start: usize,
    end: usize,
}

/// `JSON.stringify(path)`, for error messages.
fn path_json(path: &[PathSeg]) -> String {
    serde_json::to_string(path).unwrap_or_default()
}

fn no_such_item(path: &[PathSeg]) -> TomlPatchError {
    TomlPatchError::Invalid(format!("No such list item: {}", path_json(path)))
}

fn key_of(seg: &PathSeg) -> Option<&str> {
    match seg {
        PathSeg::Key(key) => Some(key),
        PathSeg::Index(_) => None,
    }
}

/// The keys of a path made only of keys.
fn keys_of(path: &[PathSeg]) -> Option<Vec<&str>> {
    path.iter().map(key_of).collect()
}

fn splice(source: &str, start: usize, end: usize, text: &str) -> String {
    let mut out = String::with_capacity(source.len() + text.len());
    out.push_str(&source[..start]);
    out.push_str(text);
    out.push_str(&source[end..]);
    out
}

/// Whitespace as JavaScript's `String.prototype.trim` sees it.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn is_blank(text: &str) -> bool {
    text.chars().all(is_js_whitespace)
}

/// A non-empty array of tables, the shape of `[[dictionary]]`, `[[profiles]]` and friends.
fn table_list(value: &Value) -> Option<Vec<&Table>> {
    match value {
        Value::Array(items) if !items.is_empty() => items.iter().map(|item| item.as_table()).collect(),
        _ => None,
    }
}

/// Removes ranges from `source`. A range that reaches the end of the file also
/// takes the blank lines before it (back to `floor` at most), so removals never
/// leave a ragged tail. Returns the text and the merged ranges, for [`map_offset`].
fn remove_ranges(source: &str, ranges: &[Range], floor: usize) -> (String, Vec<Range>) {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|range| range.start);
    let mut merged: Vec<Range> = Vec::new();
    for range in sorted {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    if let Some(mut tail) = merged.last().copied()
        && tail.end >= source.len()
    {
        while tail.start > floor {
            let previous = line_start_of(source, tail.start - 1);
            if !is_blank(&source[previous..tail.start]) {
                break;
            }
            tail.start = previous;
        }
        let count = merged.len();
        merged[count - 1] = tail;
        // The blank lines may have run back into the range before this one.
        while merged.len() > 1 && merged[merged.len() - 2].end >= tail.start {
            let before = merged.remove(merged.len() - 2);
            tail.start = tail.start.min(before.start);
            let count = merged.len();
            merged[count - 1] = tail;
        }
    }
    let mut text = source.to_string();
    for range in merged.iter().rev() {
        text = splice(&text, range.start, range.end, "");
    }
    (text, merged)
}

/// Translates an offset in the text before [`remove_ranges`] to the text after it.
fn map_offset(merged: &[Range], offset: usize) -> usize {
    let mut out = offset;
    for range in merged {
        if offset >= range.end {
            out -= range.end - range.start;
        } else if offset > range.start {
            out -= offset - range.start;
        }
    }
    out
}

/// The lines of one `key = value` entry, including a value that spans several lines.
fn entry_range(source: &str, located: &Located<'_>) -> Range {
    Range { start: line_start_of(source, located.anchor()), end: line_end_after(source, located.node.end) }
}

/// Offset just past the last `key = value` line of a section (or past its header).
fn body_end(source: &str, section: &SectionNode) -> usize {
    match section.entries.last() {
        Some(last) => line_end_after(source, last.value.end),
        None => section.body_start,
    }
}

/// A section from its header to its last key, plus the blank lines that follow.
/// Comments after the last key are left alone: they usually introduce whatever
/// comes next.
fn section_range(source: &str, section: &SectionNode) -> Range {
    let mut end = body_end(source, section);
    while end < source.len() {
        let next = line_end_after(source, end);
        if !is_blank(&source[end..next]) {
            break;
        }
        end = next;
    }
    Range { start: section.start, end }
}

/// `/(^|\n)[ \t]*\r?\n$/`: the text ends with a blank line.
fn ends_with_blank_line(text: &str) -> bool {
    let Some(rest) = text.strip_suffix('\n') else { return false };
    let rest = rest.strip_suffix('\r').unwrap_or(rest);
    let rest = rest.trim_end_matches([' ', '\t']);
    rest.is_empty() || rest.ends_with('\n')
}

/// `/^[ \t]*\r?\n/`: the text starts with a blank line.
fn starts_with_blank_line(text: &str) -> bool {
    let rest = text.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    rest.starts_with('\n')
}

/// Inserts a block of whole lines at `offset`, separated from its neighbours by a blank line.
fn insert_block(source: &str, offset: usize, block: &str, eol: &str, space_before: bool) -> String {
    let mut before = source[..offset].to_string();
    let after = &source[offset..];
    if !before.is_empty() && !before.ends_with('\n') {
        before.push_str(eol);
    }
    let spaced = is_blank(&before) || ends_with_blank_line(&before);
    let lead = if spaced || !space_before { "" } else { eol };
    let trail = if is_blank(after) || starts_with_blank_line(after) { "" } else { eol };
    format!("{before}{lead}{block}{trail}{after}")
}

fn format_block(name: &str, item: &Table, eol: &str) -> String {
    let mut lines = vec![format!("[[{}]]", format_key(name))];
    lines.extend(format_table_lines(item, eol == "\n", &[]));
    lines.join(eol) + eol
}

/// Sections that belong to the list `name`: its `[[name]]` blocks and their sub-tables.
fn list_sections<'d>(doc: &'d TomlDoc<'_>, name: &str) -> Vec<&'d SectionNode> {
    doc.sections
        .iter()
        .filter(|section| section.header.is_some() && section.path.first().and_then(key_of) == Some(name))
        .collect()
}

/// Replaces every `[[name]]` block with fresh ones, at the position of the first.
fn replace_list(doc: &TomlDoc<'_>, name: &str, items: &[&Table]) -> String {
    let (source, eol) = (doc.source, doc.eol);
    let related = list_sections(doc, name);
    let mut ranges: Vec<Range> = related.iter().map(|section| section_range(source, section)).collect();
    // Anything else that defines `name` (dotted keys before the first header) has to go too.
    for located in doc.values() {
        if located.owner.is_section() && located.path.first().and_then(key_of) == Some(name) {
            ranges.push(entry_range(source, &located));
        }
    }
    let first = related.first();
    let (text, merged) = remove_ranges(source, &ranges, first.map_or(0, |section| section.start));
    let blocks = items.iter().map(|item| format_block(name, item, eol)).collect::<Vec<_>>().join(eol);
    match first {
        None => insert_block(&text, text.len(), &blocks, eol, true),
        // The new blocks take the place of the first old one, under whatever sat above it.
        Some(first) => insert_block(&text, map_offset(&merged, first.start), &blocks, eol, false),
    }
}

/// Adds one `[[name]]` block after the existing ones.
fn append_list_item(doc: &TomlDoc<'_>, name: &str, item: &Table) -> String {
    let (source, eol) = (doc.source, doc.eol);
    let related = list_sections(doc, name);
    let offset = related.last().map_or(source.len(), |last| body_end(source, last));
    insert_block(source, offset, &format_block(name, item, eol), eol, true)
}

/// Removes one entry of an inline table or one element of an array, with its comma.
fn remove_nested(source: &str, located: &Located<'_>) -> String {
    match located.owner {
        Owner::Inline { table, entry, index } => {
            let entries = &table.entries;
            if entries.len() <= 1 {
                return splice(source, table.start, table.end, "{}");
            }
            if index < entries.len() - 1 {
                splice(source, entry.key_start, entries[index + 1].key_start, "")
            } else {
                splice(source, entries[index - 1].value.end, entry.value.end, "")
            }
        }
        Owner::Array { array, index } => {
            let items = &array.items;
            if items.len() <= 1 {
                return splice(source, array.start, array.end, "[]");
            }
            if index < items.len() - 1 {
                splice(source, items[index].start, items[index + 1].start, "")
            } else {
                splice(source, items[index - 1].end, items[index].end, "")
            }
        }
        Owner::Section { .. } => source.to_string(),
    }
}

/// Removes everything that defines `path`: key lines, dotted keys and whole table blocks.
fn remove_path(text: &str, path: &[PathSeg]) -> String {
    enum Step {
        Nested { text: String, array: bool },
        Done(String),
    }
    let mut source = text.to_string();
    for _ in 0..10_000 {
        let step = {
            let doc = scan_toml(&source);
            let values = doc.values();
            let hits: Vec<&Located<'_>> = values.iter().filter(|located| is_path_prefix(path, &located.path)).collect();
            // A value nested inside another hit goes when its container goes.
            let top: Vec<&Located<'_>> = hits
                .iter()
                .copied()
                .filter(|located| {
                    !hits.iter().any(|other| {
                        other.path.len() < located.path.len() && is_path_prefix(&other.path, &located.path)
                    })
                })
                .collect();
            if let Some(nested) = top.iter().find(|located| !located.owner.is_section()) {
                Step::Nested { text: remove_nested(&source, nested), array: nested.owner.is_array() }
            } else {
                let mut ranges: Vec<Range> = top.iter().map(|located| entry_range(&source, located)).collect();
                for section in &doc.sections {
                    if section.header.is_some() && is_path_prefix(path, &section.path) {
                        ranges.push(section_range(&source, section));
                    }
                }
                Step::Done(remove_ranges(&source, &ranges, 0).0)
            }
        };
        match step {
            Step::Nested { text, array } => {
                source = text;
                // Removing an array element shifts the next one into its index: stop here.
                if array {
                    return source;
                }
            }
            Step::Done(text) => return text,
        }
    }
    source
}

/// Adds an entry to an inline table.
fn insert_inline(source: &str, located: &Located<'_>, keys: &[&str], value: &Value) -> String {
    let entry = format!("{} = {}", format_key_path(keys), format_value(value, false));
    match located.node.entries.last() {
        None => splice(source, located.node.start, located.node.end, &format!("{{ {entry} }}")),
        Some(last) => splice(source, last.value.end, last.value.end, &format!(", {entry}")),
    }
}

/// Appends a `key = value` line to the body of a section.
fn append_line(doc: &TomlDoc<'_>, section: &SectionNode, line: &str) -> String {
    let (source, eol) = (doc.source, doc.eol);
    if section.header.is_some() || !section.entries.is_empty() {
        let offset = body_end(source, section);
        let lead = if offset > 0 && source.as_bytes()[offset - 1] != b'\n' { eol } else { "" };
        return splice(source, offset, offset, &format!("{lead}{line}{eol}"));
    }
    // No top-level keys yet: put the first one under the file's leading comment.
    let mut offset = 0;
    while offset < section.end {
        let next = line_end_after(source, offset);
        if !source[offset..next].trim_start_matches(is_js_whitespace).starts_with('#') {
            break;
        }
        offset = next;
    }
    let lead = if offset == 0 {
        String::new()
    } else if source.as_bytes()[offset - 1] == b'\n' {
        eol.to_string()
    } else {
        format!("{eol}{eol}")
    };
    let following = &source[offset..line_end_after(source, offset)];
    let trail = if is_blank(following) { "" } else { eol };
    splice(source, offset, offset, &format!("{lead}{line}{eol}{trail}"))
}

/// Writes one value as a single token: replaces it where it stands, or adds a
/// `key = value` line to the table that owns it, creating the table if needed.
fn set_leaf(doc: &TomlDoc<'_>, parsed: &Table, path: &[PathSeg], value: &Value) -> Result<String> {
    let (source, eol) = (doc.source, doc.eol);
    let line_multiline = eol == "\n";
    let values = doc.values();

    if let Some(exact) = find_value(&values, path) {
        let multiline = exact.owner.is_section() && line_multiline;
        return Ok(splice(source, exact.node.start, exact.node.end, &format_value(value, multiline)));
    }

    // The path runs through a value written as a single token: an inline table
    // takes a new entry; anything else is rewritten with the change applied.
    for length in (1..path.len()).rev() {
        let prefix = &path[..length];
        let Some(located) = find_value(&values, prefix) else { continue };
        let rest = &path[length..];
        if located.node.kind == ValueKind::Inline
            && let Some(keys) = keys_of(rest)
        {
            return Ok(insert_inline(source, located, &keys, value));
        }
        let rewritten = set_at(get_at(parsed, prefix).cloned(), rest, value.clone());
        return Ok(splice(source, located.node.start, located.node.end, &format_value(&rewritten, false)));
    }

    let table_path = &path[..path.len() - 1];
    let mut owner = &doc.sections[0];
    for section in &doc.sections {
        if section.header.is_some()
            && section.path.len() <= table_path.len()
            && section.path.len() >= owner.path.len()
            && is_path_prefix(&section.path, path)
        {
            owner = section;
        }
    }
    let rest = &path[owner.path.len()..];
    let Some(rest_keys) = keys_of(rest) else { return Err(no_such_item(path)) };

    let empty_table = matches!(value, Value::Table(table) if table.is_empty());
    // Whether a dotted key is legal here depends on how the table is already spelled.
    let dotted_here =
        owner.entries.iter().any(|entry| entry.key.first().map(String::as_str) == rest_keys.first().copied());
    let first_step = &path[..owner.path.len() + 1];
    let header_below = doc.sections.iter().any(|section| {
        !std::ptr::eq(section, owner) && section.header.is_some() && is_path_prefix(first_step, &section.path)
    });
    let new_header = if dotted_here {
        false
    } else if rest.len() >= 2 {
        owner.header.is_none() || header_below
    } else {
        empty_table && owner.header.is_none()
    };

    if !new_header {
        let line = format!("{} = {}", format_key_path(&rest_keys), format_value(value, line_multiline));
        return Ok(append_line(doc, owner, &line));
    }

    let header_path = if empty_table { path } else { table_path };
    let header_keys: Vec<&str> = header_path.iter().filter_map(key_of).collect();
    let mut block = format!("[{}]{eol}", format_key_path(&header_keys));
    if !empty_table && let Some(last) = rest_keys.last() {
        block.push_str(&format!("{} = {}{eol}", format_key(last), format_value(value, line_multiline)));
    }
    let mut offset = source.len();
    if let Some(last_index) = header_path.iter().rposition(|seg| matches!(seg, PathSeg::Index(_))) {
        // A sub-table of a list item must stay inside that item, not at the end of the file.
        let item = &header_path[..=last_index];
        offset = doc
            .sections
            .iter()
            .rfind(|section| is_path_prefix(item, &section.path))
            .map_or(source.len(), |section| section.end);
    } else if header_path.len() >= 2 {
        // Keep `[providers.openai]` next to `[providers.groq]` rather than below the profiles.
        let parent = &header_path[..header_path.len() - 1];
        let last =
            doc.sections.iter().rfind(|section| section.header.is_some() && is_path_prefix(parent, &section.path));
        if let Some(last) = last {
            offset = body_end(source, last);
        }
    }
    Ok(insert_block(source, offset, &block, eol, true))
}

/// Makes the table at `path` hold exactly the keys of `value`, key by key.
fn set_table(text: &str, path: &[PathSeg], value: &Table, current: Option<&Value>) -> Result<String> {
    let mut out = text.to_string();
    match current {
        Some(Value::Table(current)) => {
            for key in current.keys() {
                if !value.contains_key(key) {
                    out = remove_path(&out, &child_path(path, key));
                }
            }
        }
        Some(_) => out = remove_path(&out, path),
        None => {}
    }
    for (key, child) in value {
        out = set_value(&out, &child_path(path, key), child)?;
    }
    // An empty table, or one whose keys were all dotted, still has to exist.
    let parsed = parse_toml(&out)?;
    if get_at(&parsed, path).is_none() {
        out = set_leaf(&scan_toml(&out), &parsed, path, &Value::Table(Table::new()))?;
    }
    Ok(out)
}

fn child_path(path: &[PathSeg], key: &str) -> Vec<PathSeg> {
    let mut out = path.to_vec();
    out.push(PathSeg::Key(key.to_string()));
    out
}

fn set_value(text: &str, path: &[PathSeg], value: &Value) -> Result<String> {
    let parsed = parse_toml(text)?;
    let current = get_at(&parsed, path);
    if deep_equal_opt(current, Some(value)) {
        return Ok(text.to_string());
    }
    let doc = scan_toml(text);
    let values = doc.values();
    let exact = find_value(&values, path);
    let head = path.first().and_then(key_of);

    if let (Some(items), 1, Some(head), None) = (table_list(value), path.len(), head, exact) {
        return Ok(replace_list(&doc, head, &items));
    }

    if let Value::Table(table) = value {
        // A scalar becoming a table keeps its place, as an inline table.
        if let Some(exact) = exact
            && exact.node.kind != ValueKind::Inline
        {
            return set_leaf(&doc, &parsed, path, value);
        }
        if let Some(head) = head {
            let new_item = path.len() == 2
                && matches!(path[1], PathSeg::Index(_))
                && current.is_none()
                && find_value(&values, &path[..1]).is_none();
            if new_item {
                return Ok(append_list_item(&doc, head, table));
            }
        }
        return set_table(text, path, table, current);
    }

    if exact.is_none() && current.is_some() {
        // A table or a list of blocks is being replaced by a plain value.
        let cleared = remove_path(text, path);
        let parsed = parse_toml(&cleared)?;
        return set_leaf(&scan_toml(&cleared), &parsed, path, value);
    }
    set_leaf(&doc, &parsed, path, value)
}

/// Rejects paths that index past the end of a list, or into something that is not a list.
fn check_indexes(root: &Table, path: &[PathSeg]) -> Result<()> {
    for (i, part) in path.iter().enumerate() {
        let PathSeg::Index(index) = part else { continue };
        let length: i64 = if i == 0 {
            -1
        } else {
            match get_at(root, &path[..i]) {
                Some(Value::Array(items)) => items.len() as i64,
                None => 0,
                Some(_) => -1,
            }
        };
        let is_last = i == path.len() - 1;
        let index = *index as i64;
        if index > length || (index == length && !is_last) {
            return Err(no_such_item(path));
        }
    }
    Ok(())
}

/// Empty tables and empty lists read the same as absent ones.
fn prune(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(prune).collect()),
        Value::Table(table) => Value::Table(prune_table(table)),
        other => other.clone(),
    }
}

pub(crate) fn prune_table(table: &Table) -> Table {
    let mut out = Table::new();
    for (key, child) in table {
        let pruned = prune(child);
        let empty = match &pruned {
            Value::Array(items) => items.is_empty(),
            Value::Table(table) => table.is_empty(),
            _ => false,
        };
        if !empty {
            out.insert(key.clone(), pruned);
        }
    }
    out
}

fn apply_patch(text: &str, patch: &ConfigPatch) -> Result<String> {
    let path = patch.path.as_slice();
    let Some(PathSeg::Key(head)) = path.first() else {
        return Err(TomlPatchError::Invalid(format!("Invalid config path: {}", path_json(path))));
    };
    let before = parse_toml(text)?;
    let current = get_at(&before, path);
    let mut value = patch.value.as_ref().and_then(normalise_value);

    if let Some(Value::Array(items)) = &value
        && items.is_empty()
        && path.len() == 1
    {
        // An emptied list of blocks: drop the blocks instead of writing `name = []`.
        if !list_sections(&scan_toml(text), head).is_empty() {
            value = None;
        } else if current.is_none() && LIST_KEYS.contains(&head.as_str()) {
            return Ok(text.to_string());
        }
    }

    let unchanged = match &value {
        None => current.is_none(),
        Some(value) => deep_equal_opt(current, Some(value)),
    };
    if unchanged {
        return Ok(text.to_string());
    }
    check_indexes(&before, path)?;

    let mut expected = before.clone();
    match &value {
        None => remove_at(&mut expected, path),
        Some(value) => set_in_table(&mut expected, path, value.clone()),
    }

    let attempt = match &value {
        None => Ok(remove_path(text, path)),
        Some(value) => set_value(text, path, value),
    };
    match attempt {
        Ok(out) => {
            if let Ok(parsed) = parse_toml(&out)
                && tables_equal(&prune_table(&parsed), &prune_table(&expected))
            {
                return Ok(out);
            }
        }
        Err(TomlPatchError::Invalid(message)) => return Err(TomlPatchError::Invalid(message)),
        Err(TomlPatchError::Syntax(_)) => {}
    }
    // Last resort for a layout the in-place editor cannot express: the change
    // is kept and the formatting is not.
    toml::to_string(&expected).map_err(|error| TomlPatchError::Invalid(error.to_string()))
}

/// Applies patches to TOML source text, preserving everything not touched.
/// Fails with [`TomlPatchError::Syntax`] if `source` is not valid TOML, or with
/// [`TomlPatchError::Invalid`] for a path that cannot be addressed.
pub fn patch_toml(source: &str, patches: &[ConfigPatch]) -> std::result::Result<String, TomlPatchError> {
    let mut text = source.to_string();
    for patch in patches {
        text = apply_patch(&text, patch)?;
    }
    Ok(text)
}
