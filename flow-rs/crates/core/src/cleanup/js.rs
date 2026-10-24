//! JavaScript string and `RegExp` semantics the cleanup rules rely on.
//!
//! The rules were written against V8's `RegExp` (with the `u` flag) and the
//! `String` methods. Rust's `regex` and `str` differ in ways that matter here,
//! so the differences are pinned down in one place:
//!
//! - JS `\s` and `String.prototype.trim` include U+FEFF and exclude U+0085;
//!   Rust's `\s` and `str::trim` do the opposite. Patterns use [`SPACE`]
//!   instead of `\s`, and code uses [`trim`] and [`is_space`].
//! - A global `replace` re-runs the search from `lastIndex` over the whole
//!   string, so lookbehind sees text before earlier matches. [`replace_all`]
//!   does the same with `fancy_regex`.
//! - JS has no backtracking limit; [`fancy`] lifts fancy-regex's.
//! - JS string lengths and indices count UTF-16 code units ([`utf16_len`]).

use std::sync::Arc;

use fancy_regex::{Captures, Regex as FancyRegex, RegexBuilder};
use parking_lot::Mutex;

/// The body of a character class equal to JS `\s` (WhiteSpace + LineTerminator).
/// Use it inside brackets: `[{SPACE}]`.
pub(crate) const SPACE: &str = r"\t\n\x0B\x0C\r \xA0\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// True for a character JS `\s` matches (and `trim` removes).
pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `String.prototype.length`: the length in UTF-16 code units.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The largest char boundary at or below `index` (clamped to the text).
pub(crate) fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Byte offset of `text.slice(index - units, index)` in JS terms: walks back
/// `units` UTF-16 code units from the byte offset `index`. A surrogate pair cut
/// in half by the window is left out; JS would keep a lone surrogate there,
/// which no pattern in the rules can match, so the results are the same.
pub(crate) fn back_utf16(text: &str, index: usize, units: usize) -> usize {
    let mut remaining = units;
    let mut start = index;
    for (i, c) in text[..index].char_indices().rev() {
        let n = c.len_utf16();
        if n > remaining {
            break;
        }
        remaining -= n;
        start = i;
        if remaining == 0 {
            break;
        }
    }
    start
}

/// Byte offset of the end of `text.slice(index, index + units)` in JS terms.
/// See [`back_utf16`] for how a split surrogate pair is handled.
pub(crate) fn forward_utf16(text: &str, index: usize, units: usize) -> usize {
    let mut remaining = units;
    let mut end = index;
    for c in text[index..].chars() {
        let n = c.len_utf16();
        if n > remaining {
            break;
        }
        remaining -= n;
        end += c.len_utf8();
        if remaining == 0 {
            break;
        }
    }
    end
}

/// A rule pattern that could not be compiled or run (where JS would throw).
/// Boxed because `fancy_regex::Error` is large.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct RuleError(Box<fancy_regex::Error>);

impl From<fancy_regex::Error> for RuleError {
    fn from(err: fancy_regex::Error) -> Self {
        Self(Box::new(err))
    }
}

/// Compiles a pattern for `fancy_regex` without its backtracking limit: V8 has
/// none, and a limit would make long dictations fall back to the raw text.
pub(crate) fn fancy(pattern: &str) -> Result<FancyRegex, RuleError> {
    Ok(RegexBuilder::new(pattern).backtrack_limit(usize::MAX).build()?)
}

/// Compiles one of the rules' fixed patterns. They are constants covered by
/// the tests, so failing to compile is a programming error.
pub(crate) fn fixed(pattern: &str) -> FancyRegex {
    fancy(pattern).unwrap_or_else(|err| panic!("invalid cleanup pattern {pattern:?}: {err}"))
}

/// Compiles one of the rules' fixed patterns with the `regex` crate.
pub(crate) fn fixed_plain(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).unwrap_or_else(|err| panic!("invalid cleanup pattern {pattern:?}: {err}"))
}

/// `text.replace(globalRegExp, fn)`: replaces every match, searching again
/// from the end of the previous one (one code point further after an empty
/// match) with the whole text visible to lookbehind. `replace` receives the
/// match's captures; the match offset is `caps.get(0)`'s start and the whole
/// string is `text`.
pub(crate) fn replace_all(
    re: &FancyRegex,
    text: &str,
    mut replace: impl FnMut(&Captures<'_>) -> String,
) -> Result<String, RuleError> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while pos <= text.len() {
        let Some(caps) = re.captures_from_pos(text, pos)? else { break };
        let Some(m) = caps.get(0) else { break };
        out.push_str(&text[last..m.start()]);
        out.push_str(&replace(&caps));
        last = m.end();
        pos = if m.end() > m.start() {
            m.end()
        } else {
            match text[m.end()..].chars().next() {
                Some(c) => m.end() + c.len_utf8(),
                None => break,
            }
        };
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// The text of capture group `i`, or `None` when the group did not take part
/// in the match (`undefined` in a JS replace callback).
pub(crate) fn group<'t>(caps: &Captures<'t>, i: usize) -> Option<&'t str> {
    caps.get(i).map(|m| m.as_str())
}

/// The text of capture group `i`, or `""` when it did not take part.
pub(crate) fn group_or_empty<'t>(caps: &Captures<'t>, i: usize) -> &'t str {
    group(caps, i).unwrap_or("")
}

/// True if two characters are equal under the `iu` flags (simple case folding),
/// as a case-insensitive backreference compares them.
pub(crate) fn same_folded(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(&b);
    }
    // Defer to the `regex` crate's case folding, which is Unicode simple case
    // folding like JS. Rare (non-ASCII text whose letters differ), so a small
    // per-thread cache of single-character matchers is enough.
    thread_local! {
        static FOLDS: std::cell::RefCell<std::collections::HashMap<char, Option<regex::Regex>>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    FOLDS.with(|folds| {
        let mut folds = folds.borrow_mut();
        if folds.len() > 512 {
            folds.clear();
        }
        let re = folds.entry(a).or_insert_with(|| {
            let mut buf = [0u8; 4];
            regex::Regex::new(&format!("(?i)^{}$", regex::escape(a.encode_utf8(&mut buf)))).ok()
        });
        let mut buf = [0u8; 4];
        re.as_ref().is_some_and(|re| re.is_match(b.encode_utf8(&mut buf)))
    })
}

/// A one-entry cache for a value built from the user's config (dictionary or
/// snippet patterns). The config rarely changes between dictations, so the
/// last build is almost always reused.
pub(crate) struct Memo<T> {
    last: Mutex<Option<(String, Arc<T>)>>,
}

impl<T> Memo<T> {
    pub(crate) const fn new() -> Self {
        Self { last: Mutex::new(None) }
    }

    /// The value for `key`, built with `build` unless the last call used the same key.
    pub(crate) fn get<E>(&self, key: &str, build: impl FnOnce() -> Result<T, E>) -> Result<Arc<T>, E> {
        if let Some((last_key, value)) = &*self.last.lock()
            && last_key == key
        {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(build()?);
        *self.last.lock() = Some((key.to_string(), Arc::clone(&value)));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_matches_js_whitespace() {
        let re = fixed_plain(&format!("^[{SPACE}]$"));
        for c in ['\t', '\n', '\u{0B}', '\u{0C}', '\r', ' ', '\u{A0}', '\u{1680}', '\u{2000}', '\u{2005}', '\u{200A}', '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}'] {
            assert!(re.is_match(&c.to_string()), "{c:?}");
            assert!(is_space(c), "{c:?}");
        }
        for c in ['\u{85}', '\u{180E}', '\u{200B}', 'a', '-'] {
            assert!(!re.is_match(&c.to_string()), "{c:?}");
            assert!(!is_space(c), "{c:?}");
        }
        assert!(fixed(&format!("^[{SPACE}]$")).is_match("\u{FEFF}").unwrap_or(false));
    }

    #[test]
    fn trim_matches_js() {
        assert_eq!(trim("\u{FEFF} a \u{3000}"), "a");
        assert_eq!(trim("\u{85}a"), "\u{85}a");
    }

    #[test]
    fn replace_all_sees_text_before_the_search_start() {
        // The second `b` still sees the first one (already replaced) before it.
        let re = fixed("(?<![ab])b");
        assert_eq!(replace_all(&re, "bb cb", |_| "x".into()).ok().as_deref(), Some("xb cx"));
    }

    #[test]
    fn replace_all_advances_past_empty_matches() {
        let re = fixed("a*");
        assert_eq!(replace_all(&re, "aaa", |_| "-".into()).ok().as_deref(), Some("--"));
        assert_eq!(replace_all(&re, "bé", |_| "-".into()).ok().as_deref(), Some("-b-é-"));
    }

    #[test]
    fn utf16_windows() {
        let text = "a😀b";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(back_utf16(text, text.len(), 3), 1);
        assert_eq!(back_utf16(text, text.len(), 2), 5);
        assert_eq!(back_utf16(text, text.len(), 1), 5);
        assert_eq!(back_utf16(text, 5, 1), 5);
        assert_eq!(forward_utf16(text, 0, 2), 1);
        assert_eq!(forward_utf16(text, 0, 3), 5);
    }

    #[test]
    fn folding_matches_simple_case_folding() {
        assert!(same_folded('a', 'A'));
        assert!(same_folded('é', 'É'));
        assert!(same_folded('\u{17F}', 's'));
        assert!(same_folded('\u{212A}', 'k'));
        assert!(same_folded('σ', 'ς'));
        assert!(!same_folded('ı', 'i'));
        assert!(!same_folded('a', 'b'));
    }
}
