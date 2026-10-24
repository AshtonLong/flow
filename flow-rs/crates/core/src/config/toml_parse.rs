//! Parsing TOML text into values, standing in for `smol-toml`'s `parse` (and
//! its `TomlError`) wherever the TypeScript calls it.

use super::toml_doc::locate_line;
use super::{ConfigPath, PathSeg};

/// The largest integer a JavaScript number holds exactly (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// A document that is not valid TOML. Stands in for `smol-toml`'s `TomlError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct TomlSyntaxError {
    /// One line, starting with `Invalid TOML document:`.
    pub message: String,
    /// 1-based line of the problem.
    pub line: u32,
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> u32 {
    let end = offset.min(text.len());
    let newlines = text.as_bytes()[..end].iter().filter(|&&b| b == b'\n').count();
    u32::try_from(newlines + 1).unwrap_or(u32::MAX)
}

/// Finds an integer that a JavaScript number cannot hold exactly. `smol-toml`
/// refuses those ("integer value cannot be represented losslessly"), so a file
/// with one is a syntax error here too.
fn find_unsafe_integer(value: &toml::Value, path: &mut ConfigPath) -> bool {
    match value {
        toml::Value::Integer(i) => i.unsigned_abs() > MAX_SAFE_INTEGER as u64,
        toml::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(PathSeg::Index(index));
                if find_unsafe_integer(item, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        toml::Value::Table(table) => find_unsafe_in_table(table, path),
        _ => false,
    }
}

fn find_unsafe_in_table(table: &toml::Table, path: &mut ConfigPath) -> bool {
    for (key, child) in table {
        path.push(PathSeg::Key(key.clone()));
        if find_unsafe_integer(child, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// Parses a TOML document into a table. The error message is one line, like
/// the first line of `smol-toml`'s (the TS only ever shows that line).
pub fn parse_toml(text: &str) -> Result<toml::Table, TomlSyntaxError> {
    match text.parse::<toml::Table>() {
        Ok(table) => {
            let mut path = Vec::new();
            if find_unsafe_in_table(&table, &mut path) {
                return Err(TomlSyntaxError {
                    message: "Invalid TOML document: integer value cannot be represented losslessly".into(),
                    line: locate_line(text, &path).unwrap_or(1),
                });
            }
            Ok(table)
        }
        Err(error) => {
            let reason = error.message().lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("parse error");
            let line = error.span().map_or(1, |span| line_of(text, span.start));
            Err(TomlSyntaxError { message: format!("Invalid TOML document: {reason}"), line })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_line_of_a_syntax_error() {
        let error = parse_toml("version = 1\n[audio]\nsounds = = true\n").unwrap_err();
        assert_eq!(error.line, 3);
        assert!(error.message.starts_with("Invalid TOML document: "), "{}", error.message);
        assert!(!error.message.contains('\n'));
        assert_eq!(parse_toml("[audio]\nsounds = fals\n").unwrap_err().line, 2);
        assert_eq!(parse_toml("version = 1\n[audio\nsounds = false\n").unwrap_err().line, 2);
        assert_eq!(parse_toml("[audio]\nsounds = false\nkeep_mic_warm = tru\n").unwrap_err().line, 3);
    }

    #[test]
    fn rejects_integers_a_javascript_number_cannot_hold() {
        assert!(parse_toml("a = 9007199254740991\n").is_ok());
        let error = parse_toml("[t]\nb = 1\na = 9007199254740992\n").unwrap_err();
        assert_eq!(error.line, 3);
        assert!(error.message.contains("losslessly"));
    }
}
