//! Spacing and casing: fits the finished transcript to the text around the cursor.

use std::sync::LazyLock;

use regex::Regex;

use super::js;

#[derive(Debug, Clone, Copy, Default)]
pub struct FitOptions<'a> {
    /// Append one space so the next dictation doesn't run into this one.
    pub trailing_space: bool,
    /// Match capitalisation and leading space to `preceding_text`.
    pub match_case: bool,
    /// Text immediately before the cursor, when it could be read.
    pub preceding_text: Option<&'a str>,
    /// Spellings that must keep their capital even mid-sentence (dictionary terms).
    pub keep_case: &'a [String],
    /// The transcript starts with a snippet, whose text is never re-cased.
    pub starts_with_snippet: bool,
}

const OPENERS: [char; 7] = ['(', '[', '{', '<', '“', '‘', '«'];
const AMBIGUOUS_QUOTES: [char; 3] = ['"', '\'', '`'];

static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r#"(?:[.?!…]["'”’)\]}]*|[\n\r])$"#));

/// True if the text before the cursor stops in the middle of a sentence.
fn ends_mid_sentence(preceding: &str) -> bool {
    let before = preceding.trim_end_matches([' ', '\t', '\u{A0}']);
    if before.is_empty() {
        return false;
    }
    !SENTENCE_END.is_match(before)
}

/// True if text typed after `preceding` should not be separated from it by a space.
fn joins_directly(preceding: &str) -> bool {
    let mut chars = preceding.chars().rev();
    let Some(last) = chars.next() else { return true };
    if js::is_space(last) || OPENERS.contains(&last) {
        return true;
    }
    if AMBIGUOUS_QUOTES.contains(&last) {
        // A straight quote opens when nothing, a space or a bracket comes before it.
        return chars.next().is_none_or(|prior| js::is_space(prior) || OPENERS.contains(&prior));
    }
    false
}

static FIRST_WORD: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r#"^(["'“‘(\[]*)(\p{Lu})([\p{L}\p{N}'’]*)"#));
static INNER_CAPITAL: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"[\p{Lu}\p{N}]"));
static WORD_CHAR_FIRST: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"^[\p{L}\p{N}_]"));

/// Lower-cases the first letter unless the first word has to keep its capital.
fn lower_first(text: &str, keep_case: &[String]) -> String {
    let Some(m) = FIRST_WORD.captures(text) else { return text.to_string() };
    let lead = m.get(1).map_or("", |g| g.as_str());
    let first = m.get(2).map_or("", |g| g.as_str());
    let rest = m.get(3).map_or("", |g| g.as_str());
    // "I", "I'm", "I'll"...
    if first == "I" && (rest.is_empty() || rest.starts_with(['\'', '’'])) {
        return text.to_string();
    }
    // Acronyms and names with inner capitals: "API", "GitHub", "B2B".
    if INNER_CAPITAL.is_match(rest) {
        return text.to_string();
    }
    let body = &text[lead.len()..];
    for term in keep_case {
        if !term.is_empty()
            && let Some(after) = body.strip_prefix(term.as_str())
            && !WORD_CHAR_FIRST.is_match(after)
        {
            return text.to_string();
        }
    }
    format!("{lead}{}{}", first.to_lowercase(), &text[lead.len() + first.len()..])
}

static ATTACHES: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(&format!(r"^[{},.;:!?…)\]}}]", js::SPACE)));

/// Applies `match_case` and `trailing_space`. With no `preceding_text` the casing
/// is left alone: the model's capital is the best guess when the context is unknown.
pub fn fit_to_context(text: &str, opts: &FitOptions<'_>) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();
    if opts.match_case
        && let Some(preceding) = opts.preceding_text
    {
        if !opts.starts_with_snippet && ends_mid_sentence(preceding) {
            out = lower_first(&out, opts.keep_case);
        }
        let attaches = ATTACHES.is_match(&out);
        if !preceding.is_empty() && !attaches && !joins_directly(preceding) {
            out.insert(0, ' ');
        }
    }
    if opts.trailing_space && !out.chars().next_back().is_some_and(js::is_space) {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::text::tidy;

    mod tidy {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(tidy($input), $expected);
                }
            )*};
        }

        cases! {
            collapses_runs_of_spaces: "hello   world" => "hello world";
            removes_space_before_punctuation: "hello , world ." => "hello, world.";
            trims_both_ends: "  hello  " => "hello";
            strips_spaces_around_line_breaks: "one \n two" => "one\ntwo";
            keeps_line_breaks_at_the_ends: "\nhello\n\n" => "\nhello\n\n";
            keeps_a_leading_dot_on_a_word: "use .NET here" => "use .NET here";
            keeps_an_emoticon: "nice :)" => "nice :)";
            handles_the_empty_string: "" => "";
        }
    }

    mod fit_to_context {
        use super::*;

        mod trailing_space {
            use super::*;

            macro_rules! cases {
                ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                    #[test]
                    fn $name() {
                        let opts = FitOptions { trailing_space: true, match_case: false, ..Default::default() };
                        assert_eq!(fit_to_context($input, &opts), $expected);
                    }
                )*};
            }

            cases! {
                adds_one_space: "Hello." => "Hello. ";
                not_after_a_trailing_line_break: "Hello.\n" => "Hello.\n";
                not_a_second_one: "Hello. " => "Hello. ";
                not_to_nothing: "" => "";
            }

            #[test]
            fn is_off_when_trailing_space_is_off() {
                let opts = FitOptions { trailing_space: false, match_case: false, ..Default::default() };
                assert_eq!(fit_to_context("Hello.", &opts), "Hello.");
            }
        }

        mod match_case {
            use super::*;

            macro_rules! cases {
                ($($name:ident: $preceding:expr, $input:expr => $expected:expr;)*) => {$(
                    #[test]
                    fn $name() {
                        let keep_case = ["Tauri".to_string(), "Baker Street".to_string()];
                        let opts = FitOptions {
                            trailing_space: false,
                            match_case: true,
                            preceding_text: Some($preceding),
                            keep_case: &keep_case,
                            ..Default::default()
                        };
                        assert_eq!(fit_to_context($input, &opts), $expected);
                    }
                )*};
            }

            cases! {
                lower_cases_mid_sentence: "I went to the", "Store and back." => " store and back.";
                lower_cases_after_a_comma: "Well, ", "That works" => "that works";
                lower_cases_after_a_colon: "Note:", "Buy milk" => " buy milk";
                keeps_the_capital_after_a_full_stop: "Done. ", "Next one" => "Next one";
                keeps_the_capital_after_a_question_mark: "Really?", "Yes" => " Yes";
                keeps_the_capital_after_a_closing_quote: "He said \"go.\" ", "Then left" => "Then left";
                keeps_the_capital_on_a_new_line: "Title\n", "Body text" => "Body text";
                keeps_the_capital_in_an_empty_field: "", "Hello there" => "Hello there";
                keeps_the_capital_after_only_whitespace: "   ", "Hello there" => "Hello there";
                keeps_i: "and then", "I left" => " I left";
                keeps_im: "and then", "I'm leaving" => " I'm leaving";
                lower_cases_a_word_that_only_starts_with_i: "and then", "It broke" => " it broke";
                keeps_an_acronym: "use the", "API docs" => " API docs";
                keeps_inner_capitals: "push to", "GitHub now" => " GitHub now";
                keeps_a_dictionary_term: "I like", "Tauri a lot" => " Tauri a lot";
                keeps_a_multi_word_dictionary_term: "ask", "Baker Street about it" => " Baker Street about it";
                does_not_treat_a_prefix_as_the_term: "ask", "Tauriel about it" => " tauriel about it";
                lower_cases_inside_an_opening_quote: "he said", "\"Hello there\"" => " \"hello there\"";
            }

            #[test]
            fn leaves_casing_alone_when_the_preceding_text_is_unknown() {
                let opts = FitOptions { trailing_space: false, match_case: true, ..Default::default() };
                assert_eq!(fit_to_context("Store and back.", &opts), "Store and back.");
            }

            #[test]
            fn leaves_casing_alone_when_match_case_is_off() {
                let opts = FitOptions {
                    trailing_space: false,
                    match_case: false,
                    preceding_text: Some("the"),
                    ..Default::default()
                };
                assert_eq!(fit_to_context("Store.", &opts), "Store.");
            }

            #[test]
            fn never_re_cases_a_snippet() {
                let opts = FitOptions {
                    trailing_space: false,
                    match_case: true,
                    preceding_text: Some("go to"),
                    starts_with_snippet: true,
                    ..Default::default()
                };
                assert_eq!(fit_to_context("Baker Street", &opts), " Baker Street");
            }
        }

        mod leading_space {
            use super::*;

            macro_rules! cases {
                ($($name:ident: $preceding:expr, $input:expr => $expected:expr;)*) => {$(
                    #[test]
                    fn $name() {
                        let opts = FitOptions {
                            trailing_space: false,
                            match_case: true,
                            preceding_text: Some($preceding),
                            ..Default::default()
                        };
                        assert_eq!(fit_to_context($input, &opts), $expected);
                    }
                )*};
            }

            cases! {
                after_a_word: "hello", "world" => " world";
                not_after_a_space: "hello ", "world" => "world";
                not_after_a_line_break: "hello\n", "World" => "World";
                not_after_an_opening_bracket: "call (", "now" => "now";
                not_after_an_opening_curly_quote: "say “", "now" => "now";
                not_after_an_opening_straight_quote: "say \"", "now" => "now";
                after_a_closing_straight_quote: "he said \"hi\"", "and left" => " and left";
                not_before_attaching_punctuation: "hello", ", world" => ", world";
                not_before_a_line_break: "hello", "\nWorld" => "\nWorld";
                not_in_an_empty_field: "", "World" => "World";
                after_sentence_punctuation: "Done.", "Next" => " Next";
            }
        }
    }
}
