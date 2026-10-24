//! Spoken formatting ("new line", "new paragraph") and spoken punctuation
//! ("comma", "question mark", ...).
//!
//! Speech models such as Parakeet already punctuate, and every command here is
//! also an ordinary English word, so conversion is deliberately conservative.
//! A phrase is treated as a command unless its neighbours show it is being
//! used as a noun or verb:
//!
//! - a determiner or typical modifier directly before it ("a comma", "the
//!   Oxford comma", "a new line of code", "trial period");
//! - a word directly after it that continues a noun phrase ("comma separated",
//!   "colon cancer", "a dash of salt", "question mark key").
//!
//! "period" is the most common ordinary word of the set, so it must also sit at
//! the edge of a clause: at the end of the utterance, next to punctuation the
//! model wrote, before a line break, or before a capitalised word. That is why
//! "the Jurassic period was long" is left alone while "that is all period"
//! becomes "that is all."
//!
//! Punctuation the model placed around a command ("Hello. New line. How") is
//! absorbed so the result isn't doubled.

use std::collections::HashMap;
use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;

use super::js;
use super::text::{CAP, COMMAND_END, COMMAND_START, apply_capital_marks, tidy, word_after, word_before};

/// Words that make the following command a noun: "a comma", "this period".
#[rustfmt::skip]
const DETERMINERS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "each", "every", "any", "no", "another",
    "one", "some", "which", "what", "my", "your", "his", "her", "its", "our", "their", "per",
    "single", "double", "extra", "missing", "word", "words",
];

/// Words that turn the preceding command into a noun phrase: "comma key".
const NOUN_FOLLOWERS: &[&str] = &["key", "keys", "character", "characters", "symbol", "symbols"];

/// Words after "new line" that show it is a noun: "a new line of code".
const LINE_FOLLOWERS: &[&str] = &["of", "character", "characters", "break", "breaks", "item"];

static BREAK_PATTERN: LazyLock<FancyRegex> = LazyLock::new(|| {
    js::fixed(
        &[
            r"(?i)[ \t]*([,;:]?)[ \t]*",
            COMMAND_START,
            r"(new[ \t-]?paragraph|new[ \t-]?line)",
            COMMAND_END,
            r"[ \t]*[.,!?;:…]*[ \t]*",
        ]
        .concat(),
    )
});

/// The command was followed by punctuation the model wrote.
static PUNCTUATED: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"[.,!?;:…][ \t]*$"));

/// Converts "new line" to a line break and "new paragraph" to a blank line,
/// absorbing the spaces and punctuation around the command and capitalising
/// the word that follows.
///
/// Returns the text unchanged if the pattern cannot be run (see
/// [`super::apply_rules`] for the fallback that applies inside the full pipeline).
pub fn apply_spoken_formatting(text: &str) -> String {
    try_apply_spoken_formatting(text).unwrap_or_else(|_| text.to_string())
}

pub(crate) fn try_apply_spoken_formatting(text: &str) -> Result<String, js::RuleError> {
    if text.is_empty() {
        return Ok(String::new());
    }
    let out = js::replace_all(&BREAK_PATTERN, text, |caps| {
        let (whole, offset) = (text, caps.get(0).map_or(0, |m| m.start()));
        let matched = js::group_or_empty(caps, 0);
        let comma = js::group_or_empty(caps, 1);
        let command = js::group_or_empty(caps, 2);
        let previous = if comma.is_empty() { word_before(whole, offset) } else { None };
        if previous.is_some_and(|p| DETERMINERS.contains(&p.as_str())) {
            return matched.to_string();
        }
        let next = if PUNCTUATED.is_match(matched) { None } else { word_after(whole, offset + matched.len()) };
        if next.is_some_and(|n| LINE_FOLLOWERS.contains(&n.to_lowercase().as_str())) {
            return matched.to_string();
        }
        let paragraph = command.to_ascii_lowercase().contains("paragraph");
        format!("{}{CAP}", if paragraph { "\n\n" } else { "\n" })
    })?;
    Ok(tidy(&apply_capital_marks(&out)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stop,
    Join,
    Open,
    Close,
    Dash,
    Hyphen,
}

struct Command {
    phrases: &'static [&'static str],
    kind: Kind,
    symbol: &'static str,
    /// Words directly before the phrase that mark it as an ordinary noun.
    before: &'static [&'static str],
    /// Words directly after the phrase that mark it as an ordinary noun or verb.
    after: &'static [&'static str],
    /// Only a command at the edge of a clause (see the module comment).
    edge_only: bool,
}

impl Command {
    const fn new(phrases: &'static [&'static str], kind: Kind, symbol: &'static str) -> Self {
        Self { phrases, kind, symbol, before: &[], after: &[], edge_only: false }
    }
}

/// Words before "period" that make it a span of time: "trial period", "the Jurassic period".
#[rustfmt::skip]
const PERIOD_BEFORE: &[&str] = &[
    "time", "trial", "grace", "class", "waiting", "notice", "probation", "probationary",
    "cooling-off", "long", "short", "brief", "extended", "same", "whole", "entire", "first",
    "second", "third", "fourth", "last", "next", "previous", "current", "given", "certain",
    "incubation", "transition", "reporting", "billing", "pay", "rest", "free", "lunch", "exam",
    "review", "comment", "warranty", "return", "refund", "blackout", "quiet", "jurassic",
    "cretaceous", "triassic", "victorian", "edwardian", "colonial", "medieval", "of",
];

static COMMANDS: [Command; 15] = [
    Command::new(&["question mark"], Kind::Stop, "?"),
    Command::new(&["exclamation mark", "exclamation point"], Kind::Stop, "!"),
    Command { before: &["complete", "dead", "sudden"], ..Command::new(&["full stop"], Kind::Stop, ".") },
    Command {
        edge_only: true,
        before: PERIOD_BEFORE,
        after: &["of", "drama", "dramas", "piece", "pieces", "costume", "costumes"],
        ..Command::new(&["period"], Kind::Stop, ".")
    },
    Command {
        before: &["oxford", "serial", "trailing", "leading", "inverted"],
        after: &["separated", "delimited", "splice", "splices"],
        ..Command::new(&["comma"], Kind::Join, ",")
    },
    Command::new(&["semi colon"], Kind::Join, ";"),
    Command {
        after: &["cancer", "cleanse", "surgery", "polyp", "polyps", "health"],
        ..Command::new(&["colon"], Kind::Join, ":")
    },
    Command::new(&["ellipsis", "dot dot dot"], Kind::Join, "..."),
    Command::new(&["open quote", "open quotes", "begin quote"], Kind::Open, "\""),
    Command::new(&["close quote", "close quotes", "end quote", "unquote"], Kind::Close, "\""),
    Command::new(&["open paren", "open parenthesis", "open parentheses"], Kind::Open, "("),
    Command::new(&["close paren", "close parenthesis", "close parentheses"], Kind::Close, ")"),
    Command::new(&["em dash"], Kind::Dash, "—"),
    Command {
        before: &["mad", "quick", "to", "will", "must", "gotta", "i'll", "we'll", "let's"],
        after: &["of", "to", "cam", "off", "out", "for", "into", "across", "through", "over", "back"],
        ..Command::new(&["dash"], Kind::Dash, "-")
    },
    Command::new(&["hyphen"], Kind::Hyphen, "-"),
];

/// Commands keyed by phrase with the spaces removed, so "semi-colon" and "semicolon" both hit.
static BY_PHRASE: LazyLock<HashMap<String, &'static Command>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for command in &COMMANDS {
        for phrase in command.phrases {
            map.insert(phrase.replace(' ', ""), command);
        }
    }
    map
});

static PUNCTUATION_PATTERN: LazyLock<FancyRegex> = LazyLock::new(|| {
    let mut sources: Vec<String> =
        COMMANDS.iter().flat_map(|c| c.phrases).map(|phrase| phrase.replace(' ', r"[ \t-]*")).collect();
    // Longest first (a stable sort, like `Array.prototype.sort`).
    sources.sort_by_key(|source| std::cmp::Reverse(source.len()));
    js::fixed(
        &[
            r"(?i)([ \t]*(?:[,.;:!?…]+[ \t]*)?)",
            COMMAND_START,
            "(",
            &sources.join("|"),
            ")",
            COMMAND_END,
            r"([ \t]*[,.;:!?…]*[ \t]*)",
        ]
        .concat(),
    )
});

fn look_up(spoken: &str) -> Option<&'static Command> {
    let key: String = spoken.to_lowercase().chars().filter(|c| !matches!(c, ' ' | '\t' | '-')).collect();
    BY_PHRASE.get(&key).copied()
}

/// True when the neighbouring words show the phrase is an ordinary noun or verb.
fn is_ordinary_word(command: &Command, previous: Option<&str>, next: Option<&str>) -> bool {
    if let Some(previous) = previous.filter(|p| !p.is_empty())
        && (DETERMINERS.contains(&previous) || command.before.contains(&previous))
    {
        return true;
    }
    if let Some(after) = next.map(str::to_lowercase).filter(|a| !a.is_empty())
        && (NOUN_FOLLOWERS.contains(&after.as_str()) || command.after.contains(&after.as_str()))
    {
        return true;
    }
    false
}

static STARTS_UPPER: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"^\p{Lu}"));
/// A determiner, then a (proper) word, before the command: "the Edo period".
static TWO_WORDS_BEFORE: LazyLock<Regex> =
    LazyLock::new(|| js::fixed_plain(r"([\p{L}'’-]+)[ \t]+([\p{L}\p{N}'’-]+)[ \t]*$"));
static SENTENCE_STOP: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"[.!?…]"));

/// Converts spoken punctuation commands to symbols. See the module comment for the heuristic.
///
/// Returns the text unchanged if the pattern cannot be run (see
/// [`super::apply_rules`] for the fallback that applies inside the full pipeline).
pub fn apply_spoken_punctuation(text: &str) -> String {
    try_apply_spoken_punctuation(text).unwrap_or_else(|_| text.to_string())
}

pub(crate) fn try_apply_spoken_punctuation(text: &str) -> Result<String, js::RuleError> {
    if text.is_empty() {
        return Ok(String::new());
    }
    let out = js::replace_all(&PUNCTUATION_PATTERN, text, |caps| {
        let (whole, offset) = (text, caps.get(0).map_or(0, |m| m.start()));
        let matched = js::group_or_empty(caps, 0);
        let lead = js::group_or_empty(caps, 1);
        let spoken = js::group_or_empty(caps, 2);
        let trail = js::group_or_empty(caps, 3);
        let Some(command) = look_up(spoken) else { return matched.to_string() };
        let lead_mark = js::trim(lead);
        let trail_mark = js::trim(trail);
        let end = offset + matched.len();
        let previous = if lead_mark.is_empty() { word_before(whole, offset) } else { None };
        let next = if trail_mark.is_empty() { word_after(whole, end) } else { None };
        if is_ordinary_word(command, previous.as_deref(), next) {
            return matched.to_string();
        }

        let rest = &whole[end..];
        let at_end = js::trim(rest).is_empty();
        if command.edge_only {
            let starts_sentence = next.is_some_and(|n| STARTS_UPPER.is_match(n));
            let at_edge = at_end || !lead_mark.is_empty() || !trail_mark.is_empty() || rest.starts_with('\n');
            if !at_edge && !starts_sentence {
                return matched.to_string();
            }
            // "the Edo period.": a determiner, then a proper adjective, is a noun phrase.
            if lead_mark.is_empty() && previous.as_deref().is_some_and(|p| !p.is_empty()) {
                let before = &whole[..offset];
                if let Some(m) = TWO_WORDS_BEFORE.captures(before)
                    && let (Some(determiner), Some(word)) = (m.get(1), m.get(2))
                    && DETERMINERS.contains(&determiner.as_str().to_lowercase().as_str())
                    && STARTS_UPPER.is_match(word.as_str())
                {
                    return matched.to_string();
                }
            }
        }

        match command.kind {
            Kind::Stop => format!("{}{CAP} ", command.symbol),
            Kind::Join => format!("{} ", command.symbol),
            Kind::Dash => format!(" {} ", command.symbol),
            Kind::Hyphen => command.symbol.to_string(),
            Kind::Open => {
                // Keeps the model's "He said," before the quote; whatever followed the command goes.
                let space = if offset > 0 || !lead_mark.is_empty() { " " } else { "" };
                format!("{lead_mark}{space}{}", command.symbol)
            }
            Kind::Close => {
                // "hello. Close quote" keeps its full stop inside; a comma before the command was
                // only the model marking a pause.
                let inside = if SENTENCE_STOP.is_match(lead_mark) { lead_mark } else { "" };
                let after = if inside.is_empty() { trail_mark } else { "" };
                format!("{inside}{}{after} ", command.symbol)
            }
        }
    })?;
    Ok(tidy(&apply_capital_marks(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod apply_spoken_formatting {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(apply_spoken_formatting($input), $expected);
                }
            )*};
        }

        cases! {
            absorbs_the_punctuation_around_the_command: "Hello. New line. How are you" => "Hello.\nHow are you";
            drops_the_commas_around_the_command: "Hello, new line, how are you" => "Hello\nHow are you";
            works_without_punctuation: "hello new line how are you" => "hello\nHow are you";
            converts_new_paragraph_to_a_blank_line: "First new paragraph second" => "First\n\nSecond";
            handles_a_command_at_the_end: "Hello. New paragraph." => "Hello.\n\n";
            handles_a_command_at_the_start: "New line. Hello" => "\nHello";
            handles_consecutive_commands: "first new line new line second" => "first\n\nSecond";
            accepts_the_one_word_spelling: "done newline next" => "done\nNext";
            keeps_a_question_mark_before_the_command: "Really? New line. Yes" => "Really?\nYes";
            leaves_a_new_line_of_code_alone: "I need a new line of code" => "I need a new line of code";
            leaves_the_new_paragraph_alone: "the new paragraph is better" => "the new paragraph is better";
            leaves_the_plural_alone: "add new lines here" => "add new lines here";
            leaves_new_line_character_alone: "insert new line characters" => "insert new line characters";
            handles_the_empty_string: "" => "";
        }
    }

    mod apply_spoken_punctuation {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(apply_spoken_punctuation($input), $expected);
                }
            )*};
        }

        macro_rules! unchanged {
            ($($name:ident: $input:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(apply_spoken_punctuation($input), $input);
                }
            )*};
        }

        mod converts_clear_commands {
            use super::*;

            cases! {
                comma_and_question_mark: "hello comma how are you question mark" => "hello, how are you?";
                exclamation_mark: "stop exclamation mark go" => "stop! Go";
                exclamation_point: "wow exclamation point" => "wow!";
                full_stop: "yes full stop no" => "yes. No";
                colon: "note colon buy milk" => "note: buy milk";
                semicolon: "we came semicolon we saw" => "we came; we saw";
                hyphenated_semi_colon: "we came semi-colon we saw" => "we came; we saw";
                hyphen_joins_words: "well hyphen known" => "well-known";
                dash: "wait dash what" => "wait - what";
                em_dash: "wait em dash what" => "wait — what";
                ellipsis: "so dot dot dot yeah" => "so... yeah";
                quotes: "he said open quote hello close quote" => "he said \"hello\"";
                parentheses: "see open paren below close paren now" => "see (below) now";
                parenthesis_spelled_out: "see open parenthesis below close parenthesis" => "see (below)";
                period_at_the_end: "I went to the store period" => "I went to the store.";
                period_before_a_capitalised_word: "that is all period Then we left" => "that is all. Then we left";
            }
        }

        mod merges_with_punctuation_the_model_already_wrote {
            use super::*;

            cases! {
                a_period_the_model_also_punctuated: "I went to the store period." => "I went to the store.";
                a_command_transcribed_as_its_own_sentence: "I'm done. Period." => "I'm done.";
                commas_around_a_comma: "Hello, comma, how are you" => "Hello, how are you";
                a_doubled_question_mark: "Is it done? Question mark." => "Is it done?";
                commas_around_quote_commands: "He said, open quote, hello, close quote." => "He said, \"hello\".";
                a_sentence_that_ends_inside_the_quote: "she said open quote go. Close quote" => "she said \"go.\"";
            }
        }

        mod leaves_ordinary_uses_of_the_words_alone {
            use super::*;

            unchanged! {
                the_jurassic_period_was_long: "the Jurassic period was long";
                it_was_the_jurassic_period: "It was the Jurassic period.";
                it_was_the_edo_period: "It was the Edo period.";
                a_trial_period: "a trial period";
                the_trial_period_ended: "The trial period ended.";
                over_a_long_period_of_time: "over a long period of time";
                in_that_period_we_grew: "in that period we grew";
                use_a_comma_here: "use a comma here";
                the_oxford_comma: "the Oxford comma";
                comma_separated_values: "comma separated values";
                comma_hyphen_separated_values: "comma-separated values";
                colon_cancer_is_serious: "colon cancer is serious";
                a_dash_of_salt: "a dash of salt";
                i_must_dash: "I must dash";
                a_mad_dash_to_the_door: "a mad dash to the door";
                it_came_to_a_full_stop: "it came to a full stop";
                the_question_mark_key: "the question mark key";
                add_an_exclamation_mark: "add an exclamation mark";
                put_a_hyphen_between_them: "put a hyphen between them";
                periodic_commas_and_colonies: "periodic commas and colonies";
            }
        }

        #[test]
        fn handles_the_empty_string() {
            assert_eq!(apply_spoken_punctuation(""), "");
        }
    }
}
