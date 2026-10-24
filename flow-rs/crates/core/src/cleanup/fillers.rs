//! Filler removal: strips "um"/"uh"-style sounds and collapses stuttered words.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;

use super::js;
use super::text::{CAP, COMMAND_END, COMMAND_START, apply_capital_marks, tidy};

/// um, umm, uh, uhh, uhm, er, erm, hm, hmm. Deliberately not "ah", "eh" or "mm" (a unit).
const FILLER: &str = "(?:u+m+|u+h+m*|erm+|er|h+m+)";

/// A run of fillers, with whatever commas or pauses the model put between them.
static FILLER_RUN: LazyLock<String> = LazyLock::new(|| {
    [FILLER, COMMAND_END, r"(?:[ \t]*(?:[,;]|\.{2,}|…)?[ \t]*", COMMAND_START, FILLER, COMMAND_END, ")*"].concat()
});

static FILLER_PATTERN: LazyLock<FancyRegex> = LazyLock::new(|| {
    js::fixed(
        &[
            r"(?i)([ \t]*(?:(?:[,;]|\.{2,}|…)[ \t]*)?)",
            COMMAND_START,
            "(",
            FILLER_RUN.as_str(),
            r")([ \t]*(?:[,;]|\.{2,}|…|[-–—](?![\p{L}\p{N}]))?[ \t]*)([.!?]+)?",
        ]
        .concat(),
    )
});

/// Words that are legitimately said twice in a row ("she had had enough",
/// "I know that that is true", "very very good"). Everything else is treated as
/// a stutter.
#[rustfmt::skip]
const KEEP_DOUBLED: &[&str] = &[
    "had", "that", "is", "do", "her", "very", "really", "so", "no", "yes", "yeah", "ok", "okay",
    "bye", "ha", "there", "now", "well", "many", "much", "far", "long", "never", "hear", "knock",
    "blah",
];

/// Spoken digits repeat in phone numbers and codes.
static NUMBER_WORD: LazyLock<Regex> = LazyLock::new(|| {
    js::fixed_plain(
        r"(?i)^(?:\p{N}+|zero|oh|one|two|three|four|five|six|seven|eight|nine|ten|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million)$",
    )
});

/// The text before a filler ends a sentence (or is empty).
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r#"(?:^|[.!?\n]["'”’)\]]*)[ \t]*$"#));

fn strip_fillers(text: &str) -> Result<String, js::RuleError> {
    js::replace_all(&FILLER_PATTERN, text, |caps| {
        let (whole, offset) = (text, caps.get(0).map_or(0, |m| m.start()));
        let matched = js::group_or_empty(caps, 0);
        let lead = js::group_or_empty(caps, 1);
        let fillers = js::group_or_empty(caps, 2);
        let stop = js::group(caps, 4);
        // "ER" and "UM" in capitals are abbreviations, not hesitations.
        if js::utf16_len(fillers) > 1 && fillers == fillers.to_uppercase() {
            return matched.to_string();
        }
        let before = &whole[..offset];
        let starts_sentence = js::trim(lead).is_empty() && SENTENCE_END.is_match(before);
        if starts_sentence {
            // "Um. I think" and "Um, I think" both leave "I think".
            let space = if before.is_empty() || before.ends_with('\n') { "" } else { " " };
            return format!("{space}{CAP}");
        }
        stop.unwrap_or(" ").to_string()
    })
}

/// True for a character of `[\p{L}\p{N}]` under the `iu` flags of the
/// pattern this stands in for (case folding adds e.g. U+0345, which folds to ι).
fn is_letter_or_number(c: char) -> bool {
    static CLASS: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(r"(?i)^[\p{L}\p{N}]$"));
    let mut buf = [0u8; 4];
    CLASS.is_match(c.encode_utf8(&mut buf))
}

/// `[\p{L}\p{N}'’]`: the characters of a repeatable word.
fn is_repeat_char(c: char) -> bool {
    matches!(c, '\'' | '’') || is_letter_or_number(c)
}

/// `COMMAND_START` at byte offset `at`: no `[\p{L}\p{N}_'’-]` directly before.
fn command_start(text: &str, at: usize) -> bool {
    !text[..at].chars().next_back().is_some_and(|c| matches!(c, '_' | '\'' | '’' | '-') || is_letter_or_number(c))
}

/// `COMMAND_END` at byte offset `at`: `(?![\p{L}\p{N}_]|['’-][\p{L}\p{N}])`.
fn command_end(text: &str, at: usize) -> bool {
    let mut rest = text[at..].chars();
    match rest.next() {
        None => true,
        Some(c) if c == '_' || is_letter_or_number(c) => false,
        Some('\'' | '’' | '-') => !rest.next().is_some_and(is_letter_or_number),
        Some(_) => true,
    }
}

/// Where one repetition `[ \t]*,?[ \t]+\1` that starts at `at` ends, if it
/// matches. The separator has only one way to match before a word (`\1` cannot
/// start with a space or comma), and `\1` compares case-insensitively.
fn repetition_end(text: &str, at: usize, word: &str) -> Option<usize> {
    let spaces = |from: usize| text[from..].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let mut i = at + spaces(at);
    if text[i..].starts_with(',') {
        let after = spaces(i + 1);
        if after == 0 {
            return None;
        }
        i += 1 + after;
    } else if i == at {
        return None;
    }
    let mut rest = text[i..].chars();
    for expected in word.chars() {
        let c = rest.next()?;
        if !js::same_folded(expected, c) {
            return None;
        }
        i += c.len_utf8();
    }
    Some(i)
}

/// A match of `COMMAND_START([\p{L}\p{N}'’]+)((?:[ \t]*,?[ \t]+\1)+)COMMAND_END`
/// (flags `giu`) starting at byte offset `start`: `(end, word)`.
///
/// fancy-regex compares backreferences case-sensitively, so this pattern is
/// matched by hand, following the backtracking a JS engine does: the word is
/// the whole run of word characters (a shorter one leaves a word character
/// where the separator must start), the repetitions are as many as match, and
/// if the end boundary fails after the last one the engine gives back
/// repetitions until it holds.
fn repeat_at(text: &str, start: usize) -> Option<(usize, &str)> {
    if !command_start(text, start) {
        return None;
    }
    let len: usize = text[start..].chars().take_while(|&c| is_repeat_char(c)).map(char::len_utf8).sum();
    if len == 0 {
        return None;
    }
    let word = &text[start..start + len];
    let mut ends = Vec::new();
    let mut at = start + len;
    while let Some(end) = repetition_end(text, at, word) {
        ends.push(end);
        at = end;
    }
    ends.into_iter().rev().find(|&end| command_end(text, end)).map(|end| (end, word))
}

fn collapse_repeats(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while pos < text.len() {
        let Some((end, word)) = repeat_at(text, pos) else {
            pos += text[pos..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        out.push_str(&text[last..pos]);
        if KEEP_DOUBLED.contains(&word.to_lowercase().as_str()) || NUMBER_WORD.is_match(word) {
            out.push_str(&text[pos..end]);
        } else {
            out.push_str(word);
        }
        last = end;
        pos = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Removes filler sounds along with the commas around them, collapses
/// immediately repeated words ("I I think" → "I think"), and repairs the
/// spacing and capitalisation left behind.
///
/// Returns the text unchanged if the pattern cannot be run (see
/// [`super::apply_rules`] for the fallback that applies inside the full pipeline).
pub fn remove_fillers(text: &str) -> String {
    try_remove_fillers(text).unwrap_or_else(|_| text.to_string())
}

pub(crate) fn try_remove_fillers(text: &str) -> Result<String, js::RuleError> {
    if text.is_empty() {
        return Ok(String::new());
    }
    Ok(tidy(&collapse_repeats(&tidy(&apply_capital_marks(&strip_fillers(text)?)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! cases {
        ($($name:ident: $input:expr => $expected:expr;)*) => {$(
            #[test]
            fn $name() {
                assert_eq!(remove_fillers($input), $expected);
            }
        )*};
    }

    macro_rules! unchanged {
        ($($name:ident: $input:expr;)*) => {$(
            #[test]
            fn $name() {
                assert_eq!(remove_fillers($input), $input);
            }
        )*};
    }

    mod remove_fillers {
        use super::*;

        mod strips_filler_sounds {
            use super::*;

            cases! {
                at_the_start_recapitalising: "Um, I think so" => "I think so";
                at_the_start_of_a_lower_case_clause: "um, i think so" => "I think so";
                mid_sentence_with_both_commas: "I think, um, we should go" => "I think we should go";
                mid_sentence_without_commas: "so uh I was er going" => "so I was going";
                before_the_full_stop: "I think um." => "I think.";
                before_a_question_mark: "What is it, um?" => "What is it?";
                as_its_own_sentence: "Hello. Um. I think so" => "Hello. I think so";
                at_the_start_of_a_later_sentence: "Hello. Um, we think" => "Hello. We think";
                in_a_run: "um, uh, well" => "Well";
                with_a_trailing_ellipsis: "It was, uh... fine" => "It was fine";
                hmm: "Hmm, let me see" => "Let me see";
                erm_and_uhm: "erm it is uhm fine" => "It is fine";
                stretched_spellings: "ummm okay uhhh sure" => "Okay sure";
                after_a_line_break: "Done.\num, next" => "Done.\nNext";
                when_nothing_else_was_said: "Um." => "";
            }
        }

        mod keeps_look_alikes {
            use super::*;

            unchanged! {
                the_er_was_full: "the ER was full";
                uh_huh_sure: "Uh-huh sure";
                umbrella_under_her_error: "umbrella under her error";
                five_mm_thick: "5 mm thick";
                hummus_and_thumbs: "hummus and thumbs";
                ah_i_see: "ah, I see";
            }
        }

        mod collapses_stuttered_words {
            use super::*;

            cases! {
                a_doubled_pronoun: "I I think" => "I think";
                a_doubled_article: "the the cat" => "the cat";
                keeps_the_first_capital: "The the cat" => "The cat";
                a_triple: "we we we should go" => "we should go";
                with_a_comma_between: "I, I think" => "I think";
                contractions: "I'm I'm going" => "I'm going";
                after_a_filler_is_removed: "I um I think" => "I think";
            }
        }

        mod keeps_legitimate_doubles {
            use super::*;

            unchanged! {
                she_had_had_enough: "she had had enough";
                i_know_that_that_is_true: "I know that that is true";
                what_it_is_is_a_problem: "what it is is a problem";
                very_very_good: "very very good";
                no_no_not_that: "no, no, not that";
                i_gave_her_her_book: "I gave her her book";
                call_nine_nine_nine: "call nine nine nine";
                room_2_2_is_free: "room 2 2 is free";
                is_isnt_the_same: "is isn't the same";
                the_theme: "the theme";
                i_know_know_what: "I know. Know what?";
            }
        }

        #[test]
        fn handles_the_empty_string() {
            assert_eq!(remove_fillers(""), "");
        }
    }

    #[test]
    fn repeats_give_back_a_repetition_at_a_word_boundary() {
        assert_eq!(collapse_repeats("the the theme"), "the theme");
        assert_eq!(collapse_repeats("go, go,go"), "go,go");
        assert_eq!(collapse_repeats("Ça ça va"), "Ça va");
    }

    #[test]
    fn word_classes_fold_case_like_the_iu_pattern() {
        // U+0345 folds to ι, so `[\p{L}]` matches it under `iu` (as in V8).
        assert!(is_letter_or_number('\u{345}'));
        assert_eq!(collapse_repeats("go go\u{345}"), "go go\u{345}");
    }
}
