//! String helpers that behave like the JavaScript built-ins the TS relies on,
//! so lengths, trimming and whitespace collapsing match the Electron build.

/// JavaScript's `\s` and what `String.prototype.trim` removes: WhiteSpace and
/// LineTerminator. (Rust's `char::is_whitespace` differs on U+0085 and U+FEFF.)
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `text.trim()`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `text.trimStart()`.
pub(crate) fn js_trim_start(text: &str) -> &str {
    text.trim_start_matches(is_js_space)
}

/// `text.trimEnd()`.
pub(crate) fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_space)
}

/// `text.replace(/\s+/g, ' ')`.
pub(crate) fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `text.length`: UTF-16 code units.
pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `text.slice(0, units)`, counting UTF-16 code units, except that a character
/// is never cut in half (JavaScript would leave a lone surrogate).
pub(crate) fn js_truncate(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}

/// Byte index of the first ASCII-case-insensitive match of `needle` at or after
/// `from`. Matches what a JS regex with the `i` flag (and no `u`) does for an
/// ASCII pattern: only ASCII letters fold.
pub(crate) fn find_ascii_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let hay = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || from > hay.len() || hay.len() - from < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// True if `text` starts with `prefix`, ignoring ASCII case (`/^prefix/i`).
pub(crate) fn starts_with_ascii_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len() && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_like_javascript() {
        assert_eq!(js_trim("\u{FEFF} a b \u{A0}"), "a b");
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
        assert_eq!(collapse_spaces("upstream\n  timeout"), "upstream timeout");
        assert_eq!(collapse_spaces(" a \t\r\n b "), " a b ");
        assert_eq!(js_trim_start("  a  "), "a  ");
        assert_eq!(js_trim_end("  a  "), "  a");
    }

    #[test]
    fn measures_and_slices_utf16() {
        assert_eq!(js_len("“x”"), 3);
        assert_eq!(js_len("😀"), 2);
        assert_eq!(js_truncate("abcdef", 3), "abc");
        assert_eq!(js_truncate("ab😀", 3), "ab");
        assert_eq!(js_truncate("ab😀", 4), "ab😀");
        assert_eq!(js_truncate("ab", 10), "ab");
    }

    #[test]
    fn finds_ascii_case_insensitively() {
        assert_eq!(find_ascii_ci("a <TRANSCRIPT> b", "<transcript>", 0), Some(2));
        assert_eq!(find_ascii_ci("a <transcript>", "<transcript>", 3), None);
        assert_eq!(find_ascii_ci("é<Think>", "<think>", 0), Some(2));
        assert!(starts_with_ascii_ci("GPT-Transcribe", "gpt-transcribe"));
        assert!(!starts_with_ascii_ci("gpt", "gpt-transcribe"));
    }
}
