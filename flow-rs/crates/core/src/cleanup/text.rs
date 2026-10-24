//! Regex building blocks and small string helpers shared by the cleanup rules.
//!
//! The pattern constants are `fancy_regex` source (they use lookaround).
//! Character classes spell out what the TS writes as `\s` with
//! [`js::SPACE`](super::js::SPACE), since Rust's `\s` is a different set.

use std::sync::LazyLock;

use regex::Regex;

use super::js;

/// Lookbehind for the start of a spoken phrase: not inside a larger word, and
/// not the tail of an elided word such as `o'clock`. (Word characters are
/// Unicode letters, digits and underscore.)
pub const PHRASE_START: &str = r"(?<![\p{L}\p{N}_])(?<![\p{L}\p{N}]['’])";

/// Lookahead for the end of a spoken phrase: not inside a larger word. A
/// possessive `'s` may follow (`Tory's`), any other contraction may not (`don't`).
pub const PHRASE_END: &str = r"(?![\p{L}\p{N}_])(?!['’](?!s(?![\p{L}\p{N}_]))\p{L})";

/// Stricter boundaries for command words: a hyphenated compound is not a command.
pub const COMMAND_START: &str = r"(?<![\p{L}\p{N}_'’-])";
pub const COMMAND_END: &str = r"(?![\p{L}\p{N}_]|['’-][\p{L}\p{N}])";

/// Private-use marker meaning "capitalise the next letter". Rules insert it
/// while rewriting and resolve it with [`apply_capital_marks`] before returning.
pub const CAP: &str = "\u{E002}";

/// Escapes the characters JS `RegExp` treats as syntax (`.*+?^${}()|[]\`).
/// Every other character is literal outside a class in Rust patterns too.
pub fn escape_reg_exp(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What may stand between two words of a phrase: a hyphen, en dash or comma
/// with optional spaces, or just spaces.
static PHRASE_GAP: LazyLock<String> = LazyLock::new(|| format!("(?:[{s}]*[-–,][{s}]*|[{s}]+)", s = js::SPACE));

/// Regex source that matches a phrase the way a speech model might write it:
/// any run of spaces between words, a hyphen or a comma in place of the space,
/// and either apostrophe style. Returns `None` for a phrase with no words.
pub fn phrase_source(phrase: &str) -> Option<String> {
    let words: Vec<String> = js::trim(phrase)
        .split(|c: char| js::is_space(c) || c == '-')
        .filter(|w| !w.is_empty())
        .map(|w| escape_reg_exp(w).replace(['\'', '’'], "['’]"))
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(words.join(PHRASE_GAP.as_str()))
}

static WORD_BEFORE: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"([\p{L}\p{N}'’-]+)[ \t]*$"));
static WORD_AFTER: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"^[ \t]*([\p{L}\p{N}'’-]+)"));

/// The word directly before byte offset `index`, lower-cased, if only spaces
/// separate them. Looks back at most 64 UTF-16 code units, like the TS.
pub fn word_before(text: &str, index: usize) -> Option<String> {
    let index = js::floor_boundary(text, index);
    let window = &text[js::back_utf16(text, index, 64)..index];
    WORD_BEFORE.captures(window).and_then(|c| c.get(1)).map(|m| m.as_str().to_lowercase())
}

/// The word directly after byte offset `index`, in its original case, if only
/// spaces separate them. Looks ahead at most 64 UTF-16 code units, like the TS.
pub fn word_after(text: &str, index: usize) -> Option<&str> {
    let index = js::floor_boundary(text, index);
    let window = &text[index..js::forward_utf16(text, index, 64)];
    WORD_AFTER.captures(window).and_then(|c| c.get(1)).map(|m| m.as_str())
}

static CAPITAL_MARK: LazyLock<Regex> =
    LazyLock::new(|| js::fixed_plain(&format!(r#"{CAP}((?:{CAP}|[{s}"'“‘(\[])*)(\p{{Ll}})"#, s = js::SPACE)));

/// Upper-cases the letter after each capitalisation mark and removes the marks.
pub fn apply_capital_marks(text: &str) -> String {
    CAPITAL_MARK
        .replace_all(text, |c: &regex::Captures<'_>| {
            let gap = c.get(1).map_or("", |m| m.as_str());
            let letter = c.get(2).map_or("", |m| m.as_str());
            format!("{gap}{}", letter.to_uppercase())
        })
        .replace(CAP, "")
}

static INLINE_SPACE: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"[ \t\xA0]+"));
/// ` +([,.;:!?…]+)` followed by `(?=\s|$|["”’])`, which is checked by hand.
static SPACE_BEFORE_STOP: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r" +([,.;:!?…]+)"));
static SPACE_AROUND_BREAK: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r" *\n *"));
static EDGE_SPACE: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"^ +| +$"));

/// Normalises whitespace: one space between words, none before punctuation that
/// closes a clause, none around line breaks, none at either end. Line breaks
/// are kept.
pub fn tidy(text: &str) -> String {
    let out = INLINE_SPACE.replace_all(text, " ");
    // Only before punctuation that ends a clause, so ` .NET` and ` :)` survive.
    let out = remove_space_before_stops(&out);
    let out = SPACE_AROUND_BREAK.replace_all(&out, "\n");
    EDGE_SPACE.replace_all(&out, "").into_owned()
}

/// `text.replace(/ +([,.;:!?…]+)(?=\s|$|["”’])/g, '$1')`. The lookahead only
/// ever sees the character after the greedy punctuation run: a shorter run
/// would leave punctuation next, which never satisfies it, and a later start
/// inside the same run of spaces finds the same punctuation. So a failed
/// candidate is skipped one character at a time, as the JS engine does.
fn remove_space_before_stops(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while let Some(caps) = SPACE_BEFORE_STOP.captures_at(text, pos) {
        let (Some(m), Some(stops)) = (caps.get(0), caps.get(1)) else { break };
        let next = text[m.end()..].chars().next();
        let closes = next.is_none_or(|c| js::is_space(c) || matches!(c, '"' | '”' | '’'));
        if closes {
            out.push_str(&text[last..m.start()]);
            out.push_str(stops.as_str());
            last = m.end();
            pos = m.end();
        } else {
            // Spaces are one byte: the next candidate start is one byte on.
            pos = m.start() + 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_source_matches_the_ts_output() {
        assert_eq!(phrase_source("  "), None);
        assert_eq!(phrase_source("- -"), None);
        assert_eq!(
            phrase_source("don't a.b").as_deref(),
            Some(format!("don['’]t{}a\\.b", PHRASE_GAP.as_str()).as_str())
        );
    }

    #[test]
    fn word_before_and_after() {
        assert_eq!(word_before("the Comma", 3).as_deref(), Some("the"));
        assert_eq!(word_before("a the  ", 7).as_deref(), Some("the"));
        assert_eq!(word_before("the, ", 5), None);
        assert_eq!(word_after("x  Key here", 1), Some("Key"));
        assert_eq!(word_after("x, key", 1), None);
    }

    #[test]
    fn capital_marks() {
        assert_eq!(apply_capital_marks("a\u{E002} \"b c\u{E002}"), "a \"B c");
        assert_eq!(apply_capital_marks("\u{E002}ß"), "SS");
    }

    #[test]
    fn tidy_retries_after_a_failed_lookahead() {
        assert_eq!(tidy("a  ,x  , b"), "a ,x, b");
        assert_eq!(tidy("a ...\"b"), "a...\"b");
    }
}
