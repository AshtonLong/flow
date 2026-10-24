//! Serialises values as TOML tokens, and small helpers on parsed TOML values.
//!
//! Parsed values are `toml::Value`s. Numbers follow JavaScript semantics, as in
//! the TypeScript: an integer and a float with the same value are equal, and a
//! float with an integral value is written as an integer.
//!
//! PORT: src/main/config/toml-write.ts

use serde_json::Value as Json;
use toml::Value;

use super::toml_parse::MAX_SAFE_INTEGER;
use super::PathSeg;

pub type Table = toml::Table;

pub fn is_plain_object(value: &Value) -> bool {
    matches!(value, Value::Table(_))
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// `Object.is` on two numbers: NaN equals NaN, 0 and -0 differ.
fn same_number(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    a == b && a.is_sign_negative() == b.is_sign_negative()
}

/// Structural equality over parsed TOML values.
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => x == y,
        (Value::Integer(_) | Value::Float(_), Value::Integer(_) | Value::Float(_)) => {
            same_number(number(a).unwrap_or(f64::NAN), number(b).unwrap_or(f64::NAN))
        }
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Boolean(x), Value::Boolean(y)) => x == y,
        (Value::Datetime(x), Value::Datetime(y)) => x.to_string() == y.to_string(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| deep_equal(p, q)),
        (Value::Table(x), Value::Table(y)) => tables_equal(x, y),
        _ => false,
    }
}

pub fn tables_equal(a: &Table, b: &Table) -> bool {
    a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|other| deep_equal(value, other)))
}

/// [`deep_equal`] where `None` stands for `undefined`.
pub fn deep_equal_opt(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => deep_equal(a, b),
        _ => false,
    }
}

/// Turns a patch value into a TOML value, dropping what TOML cannot hold:
/// `null` members of tables and arrays. A top-level `null` becomes `None`,
/// which means "remove". An integer outside the range a JavaScript number holds
/// exactly becomes a float, as it would have in the TypeScript.
pub fn normalise_value(value: &Json) -> Option<Value> {
    match value {
        Json::Null => None,
        Json::Bool(b) => Some(Value::Boolean(*b)),
        Json::Number(n) => Some(if let Some(i) = n.as_i64() {
            if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 { Value::Integer(i) } else { Value::Float(i as f64) }
        } else if let Some(u) = n.as_u64() {
            Value::Float(u as f64)
        } else {
            Value::Float(n.as_f64().unwrap_or(f64::NAN))
        }),
        Json::String(s) => Some(Value::String(s.clone())),
        Json::Array(items) => Some(Value::Array(items.iter().filter_map(normalise_value).collect())),
        Json::Object(map) => {
            let mut out = Table::new();
            for (key, child) in map {
                if let Some(normalised) = normalise_value(child) {
                    out.insert(key.clone(), normalised);
                }
            }
            Some(Value::Table(out))
        }
    }
}

/// The table key a path segment addresses (a number is used as its text, as in JavaScript).
fn key_text(seg: &PathSeg) -> String {
    match seg {
        PathSeg::Key(key) => key.clone(),
        PathSeg::Index(index) => index.to_string(),
    }
}

fn step<'a>(node: &'a Value, seg: &PathSeg) -> Option<&'a Value> {
    match node {
        Value::Array(items) => match seg {
            PathSeg::Index(index) => items.get(*index),
            PathSeg::Key(_) => None,
        },
        Value::Table(table) => table.get(&key_text(seg)),
        _ => None,
    }
}

fn step_mut<'a>(node: &'a mut Value, seg: &PathSeg) -> Option<&'a mut Value> {
    match node {
        Value::Array(items) => match seg {
            PathSeg::Index(index) => items.get_mut(*index),
            PathSeg::Key(_) => None,
        },
        Value::Table(table) => table.get_mut(&key_text(seg)),
        _ => None,
    }
}

/// The value at `path` below a table; `None` for an empty path's parent-less lookups too.
pub fn get_at<'a>(root: &'a Table, path: &[PathSeg]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut node = root.get(&key_text(first))?;
    for seg in rest {
        node = step(node, seg)?;
    }
    Some(node)
}

fn get_at_mut<'a>(root: &'a mut Table, path: &[PathSeg]) -> Option<&'a mut Value> {
    let (first, rest) = path.split_first()?;
    let mut node = root.get_mut(&key_text(first))?;
    for seg in rest {
        node = step_mut(node, seg)?;
    }
    Some(node)
}

/// Sets `value` at `path`, replacing anything in the way with tables. Returns the new root.
pub fn set_at(root: Option<Value>, path: &[PathSeg], value: Value) -> Value {
    let Some((head, rest)) = path.split_first() else { return value };
    match head {
        PathSeg::Index(index) => {
            let mut list = match root {
                Some(Value::Array(items)) => items,
                _ => Vec::new(),
            };
            let child = list.get(*index).cloned();
            let next = set_at(child, rest, value);
            if *index < list.len() {
                list[*index] = next;
            } else {
                list.push(next);
            }
            Value::Array(list)
        }
        PathSeg::Key(_) => {
            let mut table = match root {
                Some(Value::Table(table)) => table,
                _ => Table::new(),
            };
            set_in_table(&mut table, path, value);
            Value::Table(table)
        }
    }
}

/// [`set_at`] on a table root, in place.
pub fn set_in_table(table: &mut Table, path: &[PathSeg], value: Value) {
    let Some((head, rest)) = path.split_first() else { return };
    let key = key_text(head);
    let child = table.get(&key).cloned();
    table.insert(key, set_at(child, rest, value));
}

/// Removes the key or list item at `path`, if present.
pub fn remove_at(root: &mut Table, path: &[PathSeg]) {
    let Some((last, parents)) = path.split_last() else { return };
    if parents.is_empty() {
        root.remove(&key_text(last));
        return;
    }
    match get_at_mut(root, parents) {
        Some(Value::Array(items)) => {
            if let PathSeg::Index(index) = last
                && *index < items.len()
            {
                items.remove(*index);
            }
        }
        Some(Value::Table(table)) => {
            table.remove(&key_text(last));
        }
        _ => {}
    }
}

fn is_bare_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A single key segment, quoted only when it has to be.
pub fn format_key(key: &str) -> String {
    if is_bare_key(key) { key.to_string() } else { basic_string(key) }
}

/// A dotted key: `["cleanup", "llm_enabled"]` → `cleanup.llm_enabled`.
pub fn format_key_path<S: AsRef<str>>(keys: &[S]) -> String {
    keys.iter().map(|key| format_key(key.as_ref())).collect::<Vec<_>>().join(".")
}

/// Control characters, quotes and backslashes cannot appear raw in a basic string.
fn needs_escape(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}' | '"' | '\\')
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '\u{8}' => out.push_str("\\b"),
        '\t' => out.push_str("\\t"),
        '\n' => out.push_str("\\n"),
        '\u{c}' => out.push_str("\\f"),
        '\r' => out.push_str("\\r"),
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        other => out.push_str(&format!("\\u{:04x}", other as u32)),
    }
}

/// A single-line TOML basic string.
pub fn basic_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if needs_escape(c) {
            push_escaped(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}

/// A multi-line TOML basic string, with real line breaks so long prompt and
/// snippet text stays readable in an editor.
fn multiline_string(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_raw_quote = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let is_last = chars.peek().is_none();
        let raw_quote = if c == '\n' {
            out.push('\n');
            false
        } else if c == '"' {
            // Never leave two raw quotes together or one at the end: either could
            // run into the closing delimiter.
            if last_was_raw_quote || is_last {
                out.push_str("\\\"");
                false
            } else {
                out.push('"');
                true
            }
        } else {
            if needs_escape(c) {
                push_escaped(&mut out, c);
            } else {
                out.push(c);
            }
            false
        };
        last_was_raw_quote = raw_quote;
    }
    // The newline right after the opening delimiter is not part of the value.
    format!("\"\"\"\n{out}\"\"\"")
}

/// `String(value)` for a JavaScript number: the shortest digits that read back
/// as the same number, in fixed notation from 1e-7 to 1e21 and exponential
/// notation outside that range.
pub fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if value == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // `{:e}` gives the shortest round-trip digits: `d.ddde±x`.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let k = digits.len() as i64;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e_sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{e_sign}{}", e.abs())
        } else {
            format!("{}.{}e{e_sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    };
    format!("{sign}{body}")
}

fn number_token(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value == f64::INFINITY {
        return "inf".into();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".into();
    }
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER as f64 {
        return format!("{}", value as i64);
    }
    let text = js_number_string(value);
    // A float needs a fraction or an exponent to be read back as one.
    if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") }
}

/// Serialises a value as a TOML token; tables become inline tables.
///
/// `multiline` allows `"""` strings for text containing line breaks. Only safe
/// for a `key = value` line in a document with `\n` line endings: in a CRLF
/// document the line breaks would be read back as `\r\n`.
pub fn format_value(value: &Value, multiline: bool) -> String {
    match value {
        Value::String(text) => {
            if multiline && text.contains('\n') {
                multiline_string(text)
            } else {
                basic_string(text)
            }
        }
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => number_token(*f),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Datetime(d) => d.to_string(),
        Value::Array(items) => {
            format!("[{}]", items.iter().map(|item| format_value(item, false)).collect::<Vec<_>>().join(", "))
        }
        Value::Table(table) => {
            if table.is_empty() {
                return "{}".into();
            }
            let body: Vec<String> =
                table.iter().map(|(key, child)| format!("{} = {}", format_key(key), format_value(child, false))).collect();
            format!("{{ {} }}", body.join(", "))
        }
    }
}

/// The `key = value` lines of one table body. Nested tables are flattened to
/// dotted keys (`cleanup.llm_enabled = true`), as in the spec's profile example.
pub fn format_table_lines(table: &Table, multiline: bool, prefix: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, value) in table {
        let mut keys = prefix.to_vec();
        keys.push(key.clone());
        match value {
            Value::Table(inner) if !inner.is_empty() => lines.extend(format_table_lines(inner, multiline, &keys)),
            _ => lines.push(format!("{} = {}", format_key_path(&keys), format_value(value, multiline))),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_like_javascript() {
        let cases: [(f64, &str); 12] = [
            (0.0, "0"),
            (-0.0, "0"),
            (42.0, "42"),
            (1.5, "1.5"),
            (-0.25, "-0.25"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (123456.789, "123456.789"),
            (0.000001, "0.000001"),
            (9007199254740992.0, "9007199254740992"),
            (1.2345e25, "1.2345e+25"),
        ];
        for (value, text) in cases {
            assert_eq!(js_number_string(value), text, "{value}");
        }
        assert_eq!(number_token(9007199254740992.0), "9007199254740992.0");
        assert_eq!(number_token(-1.0), "-1");
        assert_eq!(number_token(f64::NAN), "nan");
        assert_eq!(number_token(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn escapes_strings() {
        assert_eq!(basic_string("a\"b\\c\u{0}\u{7f}\t"), "\"a\\\"b\\\\c\\u0000\\u007f\\t\"");
        assert_eq!(format_key("odd.name"), "\"odd.name\"");
        assert_eq!(format_key("x-1"), "x-1");
        assert_eq!(multiline_string("a\"\"b\""), "\"\"\"\na\"\\\"b\\\"\"\"\"");
    }

    #[test]
    fn compares_numbers_like_javascript() {
        assert!(deep_equal(&Value::Integer(3), &Value::Float(3.0)));
        assert!(deep_equal(&Value::Float(f64::NAN), &Value::Float(f64::NAN)));
        assert!(!deep_equal(&Value::Integer(0), &Value::Float(-0.0)));
        assert!(!deep_equal(&Value::Integer(1), &Value::String("1".into())));
    }
}
