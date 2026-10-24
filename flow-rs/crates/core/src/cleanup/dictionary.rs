//! Dictionary: replaces misheard words with the spelling the user wants.

use fancy_regex::Regex as FancyRegex;

use super::js::{self, Memo};
use super::text::{PHRASE_END, PHRASE_START, phrase_source};
use crate::config::DictionaryEntry;

struct Rule<'a> {
    source: String,
    write: &'a str,
    words: usize,
    length: usize,
}

fn build_rules(dictionary: &[DictionaryEntry]) -> Vec<Rule<'_>> {
    let mut rules = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in dictionary {
        for heard in &entry.heard {
            // The first entry to claim a heard phrase wins.
            let Some(source) = phrase_source(heard) else { continue };
            if !seen.insert(source.to_lowercase()) {
                continue;
            }
            rules.push(Rule {
                source,
                write: &entry.write,
                // `heard.trim().split(/[\s-]+/).length`: one more than the number of separator runs.
                words: count_words(js::trim(heard)),
                length: js::utf16_len(heard),
            });
        }
    }
    // Longest phrase first, so "tory app" beats "tory" where both could match.
    rules.sort_by(|a, b| b.words.cmp(&a.words).then(b.length.cmp(&a.length)));
    rules
}

/// The number of pieces `text.split(/[\s-]+/)` returns, empty pieces included.
fn count_words(text: &str) -> usize {
    let mut pieces = 1;
    let mut in_separator = false;
    for c in text.chars() {
        let separator = js::is_space(c) || c == '-';
        if separator && !in_separator {
            pieces += 1;
        }
        in_separator = separator;
    }
    pieces
}

/// The combined pattern, cached by its source: the dictionary rarely changes.
static PATTERN: Memo<FancyRegex> = Memo::new();

/// Replaces every `heard` phrase with its `write` spelling. Matching ignores
/// case, respects word boundaries (Unicode-aware) and leaves the punctuation
/// around the phrase alone. One pass, so a replacement is never replaced again.
///
/// Returns the text unchanged if the pattern cannot be run (see [`super::apply_rules`]
/// for the fallback that applies inside the full pipeline).
pub fn apply_dictionary(text: &str, dictionary: &[DictionaryEntry]) -> String {
    try_apply_dictionary(text, dictionary).unwrap_or_else(|_| text.to_string())
}

pub(crate) fn try_apply_dictionary(text: &str, dictionary: &[DictionaryEntry]) -> Result<String, js::RuleError> {
    if text.is_empty() || dictionary.is_empty() {
        return Ok(text.to_string());
    }
    let rules = build_rules(dictionary);
    if rules.is_empty() {
        return Ok(text.to_string());
    }
    let alternatives: Vec<String> = rules.iter().map(|r| format!("({})", r.source)).collect();
    let source = format!("(?i){PHRASE_START}(?:{}){PHRASE_END}", alternatives.join("|"));
    let pattern = PATTERN.get(&source, || js::fancy(&source))?;
    js::replace_all(&pattern, text, |caps| {
        let index = (1..=rules.len()).find(|&i| caps.get(i).is_some());
        match index.and_then(|i| rules.get(i - 1)) {
            Some(rule) => rule.write.to_string(),
            None => js::group_or_empty(caps, 0).to_string(),
        }
    })
}

/// The `write` terms, trimmed and de-duplicated, in dictionary order.
pub fn dictionary_terms(dictionary: &[DictionaryEntry]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for entry in dictionary {
        let term = js::trim(&entry.write);
        if !term.is_empty() && !terms.iter().any(|t| t == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::dictionary_hints;
    use crate::config::Config;

    fn entry(heard: &[&str], write: &str) -> DictionaryEntry {
        DictionaryEntry { heard: heard.iter().map(|h| h.to_string()).collect(), write: write.to_string() }
    }

    fn dictionary() -> Vec<DictionaryEntry> {
        vec![
            entry(&["tory", "towery"], "Tauri"),
            entry(&["tory app"], "Tauri App"),
            entry(&["post gress", "postgres"], "PostgreSQL"),
            entry(&["don"], "Don"),
            entry(&["c plus plus"], "C++"),
            entry(&["élan vital"], "Élan Vital"),
            entry(&["cash money"], "$& Co"),
        ]
    }

    mod apply_dictionary {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(apply_dictionary($input, &dictionary()), $expected);
                }
            )*};
        }

        cases! {
            replaces_a_heard_word: "I use tory daily" => "I use Tauri daily";
            ignores_case_and_keeps_punctuation: "Tory, towery and TORY." => "Tauri, Tauri and Tauri.";
            does_not_replace_inside_larger_words: "history story toryism" => "history story toryism";
            does_not_treat_accented_letters_as_boundaries: "étory tory" => "étory Tauri";
            keeps_a_possessive: "tory's API" => "Tauri's API";
            leaves_other_contractions_alone: "I don't know don" => "I don't know Don";
            prefers_the_longest_phrase: "the tory app" => "the Tauri App";
            tolerates_a_hyphen_in_a_phrase: "Post-gress is nice" => "PostgreSQL is nice";
            tolerates_a_comma_in_a_phrase: "post, gress is nice" => "PostgreSQL is nice";
            tolerates_extra_spaces_in_a_phrase: "c  plus   plus rocks" => "C++ rocks";
            matches_unicode_phrases: "the Élan vital idea" => "the Élan Vital idea";
            inserts_the_replacement_literally: "cash money" => "$& Co";
            is_idempotent_on_the_written_form: "Tauri is Tauri" => "Tauri is Tauri";
            handles_the_empty_string: "" => "";
        }

        #[test]
        fn returns_the_text_unchanged_with_an_empty_dictionary() {
            assert_eq!(apply_dictionary("tory", &[]), "tory");
        }

        #[test]
        fn does_not_replace_a_replacement_again() {
            let chained = [entry(&["alpha"], "beta"), entry(&["beta"], "gamma")];
            assert_eq!(apply_dictionary("alpha beta", &chained), "beta gamma");
        }

        #[test]
        fn lets_the_first_entry_win_a_heard_phrase_claimed_twice() {
            let twice = [entry(&["kube"], "Kubernetes"), entry(&["Kube"], "cube")];
            assert_eq!(apply_dictionary("kube", &twice), "Kubernetes");
        }

        #[test]
        fn treats_regex_characters_in_a_heard_phrase_literally() {
            assert_eq!(apply_dictionary("use a.b here, not axb", &[entry(&["a.b"], "AB")]), "use AB here, not axb");
        }
    }

    mod dictionary_hints {
        use super::*;

        #[test]
        fn returns_the_write_terms_once_each_in_order() {
            let config = Config {
                dictionary: vec![
                    entry(&["tory"], "Tauri"),
                    entry(&["towery"], "Tauri"),
                    entry(&["cube control"], " kubectl "),
                ],
                ..Config::default()
            };
            assert_eq!(dictionary_hints(&config), ["Tauri", "kubectl"]);
        }

        #[test]
        fn is_empty_with_no_dictionary() {
            assert!(dictionary_hints(&Config::default()).is_empty());
        }
    }

    #[test]
    fn counts_words_like_js_split() {
        assert_eq!(count_words("tory"), 1);
        assert_eq!(count_words("post gress"), 2);
        assert_eq!(count_words("c  plus - plus"), 3);
        assert_eq!(count_words("-tory"), 2);
        assert_eq!(count_words(""), 1);
    }
}
