//! Trims dictionary hints to what a provider documents for its prompt or keyterm list.
//! Port of `src/main/transcribe/cloud/hints.ts`.

use std::collections::HashSet;

use crate::text::{collapse_spaces, js_len, js_trim};

/// Caps for [`limit_hints`]. Lengths count UTF-16 code units, like the TS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HintLimits {
    /// Most terms to send.
    pub max_terms: Option<usize>,
    /// Budget for all terms together, in characters.
    pub max_total_chars: Option<usize>,
    /// Terms longer than this are dropped.
    pub max_term_chars: Option<usize>,
    /// Terms with more words than this are dropped.
    pub max_words: Option<usize>,
}

/// Cleans, de-duplicates (ignoring case) and caps a hint list. Whole terms are
/// kept or dropped, never cut, and earlier terms win when the budget runs out.
pub fn limit_hints<S: AsRef<str>>(hints: &[S], limits: HintLimits) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = 0;
    for raw in hints {
        let term = js_trim(&collapse_spaces(raw.as_ref())).to_string();
        let key = term.to_lowercase();
        if term.is_empty() || seen.contains(&key) {
            continue;
        }
        let length = js_len(&term);
        if limits.max_term_chars.is_some_and(|max| length > max) {
            continue;
        }
        if limits.max_words.is_some_and(|max| term.split(' ').count() > max) {
            continue;
        }
        if limits.max_terms.is_some_and(|max| out.len() >= max) {
            break;
        }
        if limits.max_total_chars.is_some_and(|max| total + length > max) {
            continue;
        }
        seen.insert(key);
        out.push(term);
        total += length + 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_collapses_spaces_and_drops_blanks_and_duplicates() {
        assert_eq!(
            limit_hints(&[" Tauri ", "tauri", "", "  ", "Baker   Street"], HintLimits::default()),
            ["Tauri", "Baker Street"]
        );
    }

    #[test]
    fn caps_the_number_of_terms_keeping_the_first_ones() {
        let limits = HintLimits { max_terms: Some(2), ..HintLimits::default() };
        assert_eq!(limit_hints(&["a", "b", "c", "d"], limits), ["a", "b"]);
    }

    #[test]
    fn caps_the_total_length_with_whole_terms_only() {
        let limits = HintLimits { max_total_chars: Some(12), ..HintLimits::default() };
        assert_eq!(limit_hints(&["alpha", "a-very-long-term", "beta"], limits), ["alpha", "beta"]);
    }

    #[test]
    fn drops_terms_that_are_too_long_or_have_too_many_words() {
        let limits = HintLimits { max_term_chars: Some(49), max_words: Some(5), ..HintLimits::default() };
        let long = "x".repeat(50);
        assert_eq!(limit_hints(&["short", long.as_str(), "one two three four five six"], limits), ["short"]);
    }
}
