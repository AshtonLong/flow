//! Layer 1 of text cleanup: instant local rules. Always on, no network.
//!
//! PORT: `src/main/cleanup/{index,dictionary,fillers,snippets,spacing,spoken,text}.ts`.
//! The rules lean on JavaScript `RegExp` semantics; `js` pins down where
//! Rust's `regex`/`fancy_regex` differ and how the port matches V8.

pub mod dictionary;
pub mod fillers;
mod js;
pub mod snippets;
pub mod spacing;
pub mod spoken;
pub mod text;

use std::panic::AssertUnwindSafe;

pub use dictionary::{apply_dictionary, dictionary_terms};
pub use fillers::remove_fillers;
pub use snippets::{ExpandedSnippets, apply_snippets, expand_snippets, restore_snippets, starts_with_snippet};
pub use spacing::{FitOptions, fit_to_context};
pub use spoken::{apply_spoken_formatting, apply_spoken_punctuation};
pub use text::tidy;

use crate::config::Config;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesContext {
    /// Text immediately before the cursor, when it can be read. Used for casing/spacing.
    pub preceding_text: Option<String>,
}

/// Private-use characters the rules use as internal markers; never valid transcript text.
fn is_marker(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{E002}')
}

/// Dictionary → snippets → spoken formatting → filler removal → spacing and casing. Pure.
pub fn apply_rules(text: &str, config: &Config, ctx: &RulesContext) -> String {
    guarded(text, || run_rules(text, config, ctx))
}

/// Runs the rules for a non-blank transcript. A rule must never cost the user
/// a dictation: if one fails (or panics, the TS `catch` covers bugs too), the
/// raw transcript is returned, trimmed.
fn guarded(text: &str, rules: impl FnOnce() -> Result<String, js::RuleError>) -> String {
    if js::trim(text).is_empty() {
        return String::new();
    }
    match std::panic::catch_unwind(AssertUnwindSafe(rules)) {
        Ok(Ok(out)) => out,
        Ok(Err(_)) | Err(_) => js::trim(text).to_string(),
    }
}

fn run_rules(text: &str, config: &Config, ctx: &RulesContext) -> Result<String, js::RuleError> {
    let out: String = text.chars().filter(|&c| !is_marker(c)).collect();
    let out = out.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = dictionary::try_apply_dictionary(&out, &config.dictionary)?;
    // Snippets become opaque slots so the rules below cannot alter saved text.
    let expanded = snippets::try_expand_snippets(&out, &config.snippets)?;
    out = expanded.text;
    if config.cleanup.spoken_formatting {
        out = spoken::try_apply_spoken_formatting(&out)?;
    }
    if config.cleanup.spoken_punctuation {
        out = spoken::try_apply_spoken_punctuation(&out)?;
    }
    if config.cleanup.filler_removal {
        out = fillers::try_remove_fillers(&out)?;
    }
    out = tidy(&out);
    let snippet_first = starts_with_snippet(&out);
    out = restore_snippets(&out, &expanded.slots);
    let keep_case = dictionary_terms(&config.dictionary);
    Ok(fit_to_context(
        &out,
        &FitOptions {
            trailing_space: config.insert.trailing_space,
            match_case: config.insert.match_case,
            preceding_text: ctx.preceding_text.as_deref(),
            keep_case: &keep_case,
            starts_with_snippet: snippet_first,
        },
    ))
}

/// Dictionary `write` terms, de-duplicated, to pass to speech models as hints.
pub fn dictionary_hints(config: &Config) -> Vec<String> {
    dictionary_terms(&config.dictionary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DictionaryEntry, Snippet};

    fn config() -> Config {
        Config {
            dictionary: vec![DictionaryEntry { heard: vec!["tory".into(), "towery".into()], write: "Tauri".into() }],
            snippets: vec![
                Snippet { trigger: "my address".into(), text: "221B Baker Street, London".into() },
                Snippet { trigger: "code block".into(), text: "if (x)  {\n    um  period\n}".into() },
            ],
            ..Config::default()
        }
    }

    fn rules(text: &str, config: &Config) -> String {
        apply_rules(text, config, &RulesContext::default())
    }

    mod apply_rules {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(rules($input, &config()), $expected);
                }
            )*};
        }

        cases! {
            empty_input: "" => "";
            whitespace_only: "  \n " => "";
            only_a_filler: "Um." => "";
            adds_a_trailing_space: "Hello world." => "Hello world. ";
            trims_what_the_model_padded: "  Hello world.  " => "Hello world. ";
            collapses_spaces_and_fixes_punctuation: "hello  world , ok" => "hello world, ok ";
            applies_the_dictionary: "I use tory." => "I use Tauri. ";
            expands_a_lone_snippet: "My address." => "221B Baker Street, London ";
            runs_every_rule_in_order:
                " Um, I use towery. New line. my address" => "I use Tauri.\n221B Baker Street, London ";
            converts_spoken_punctuation: "hello comma world question mark" => "hello, world? ";
            adds_no_space_after_a_trailing_line_break: "Hello. New line." => "Hello.\n";
            treats_a_line_break_as_the_end_of_a_clause:
                "I went home period new line next item" => "I went home.\nNext item ";
            normalises_windows_line_endings: "one\r\ntwo" => "one\ntwo ";
        }

        #[test]
        fn never_alters_the_text_of_a_snippet() {
            assert_eq!(rules("insert code block", &config()), "insert if (x)  {\n    um  period\n} ");
        }

        #[test]
        fn applies_the_dictionary_before_snippets_so_a_corrected_trigger_still_fires() {
            let cfg = Config {
                dictionary: vec![DictionaryEntry { heard: vec!["my a dress".into()], write: "my address".into() }],
                ..config()
            };
            assert_eq!(rules("my a dress", &cfg), "221B Baker Street, London ");
        }

        mod switches {
            use super::*;

            #[test]
            fn spoken_formatting_off_keeps_new_line_as_words() {
                let mut cfg = config();
                cfg.cleanup.spoken_formatting = false;
                assert_eq!(rules("Hello. New line. Bye", &cfg), "Hello. New line. Bye ");
            }

            #[test]
            fn spoken_punctuation_off_keeps_comma_as_a_word() {
                let mut cfg = config();
                cfg.cleanup.spoken_punctuation = false;
                assert_eq!(rules("hello comma world", &cfg), "hello comma world ");
            }

            #[test]
            fn spoken_punctuation_off_still_converts_new_line() {
                let mut cfg = config();
                cfg.cleanup.spoken_punctuation = false;
                assert_eq!(rules("hello new line world", &cfg), "hello\nWorld ");
            }

            #[test]
            fn filler_removal_off_keeps_fillers_and_repeats() {
                let mut cfg = config();
                cfg.cleanup.filler_removal = false;
                assert_eq!(rules("Um, I I think", &cfg), "Um, I I think ");
            }

            #[test]
            fn trailing_space_off_adds_nothing() {
                let mut cfg = config();
                cfg.insert.trailing_space = false;
                assert_eq!(rules("Hello.", &cfg), "Hello.");
            }
        }

        mod with_the_text_before_the_cursor {
            use super::*;

            macro_rules! cases {
                ($($name:ident: $preceding:expr, $input:expr => $expected:expr;)*) => {$(
                    #[test]
                    fn $name() {
                        let ctx = RulesContext { preceding_text: Some($preceding.to_string()) };
                        assert_eq!(apply_rules($input, &config(), &ctx), $expected);
                    }
                )*};
            }

            cases! {
                continues_a_sentence: "I went to the", "Store and bought milk." => " store and bought milk. ";
                starts_a_new_sentence: "Done. ", "Store and bought milk." => "Store and bought milk. ";
                keeps_a_dictionary_term_capitalised: "I like", "Tory a lot" => " Tauri a lot ";
                keeps_a_snippet_as_saved: "send it to", "my address" => " 221B Baker Street, London ";
                attaches_leading_punctuation: "I went", ", and then left" => ", and then left ";
                types_straight_into_an_empty_field: "", "Hello there" => "Hello there ";
            }

            #[test]
            fn leaves_casing_alone_when_match_case_is_off() {
                let mut cfg = config();
                cfg.insert.match_case = false;
                let ctx = RulesContext { preceding_text: Some("the".into()) };
                assert_eq!(apply_rules("Store.", &cfg, &ctx), "Store. ");
            }

            #[test]
            fn leaves_casing_alone_without_preceding_text() {
                assert_eq!(rules("Store and back.", &config()), "Store and back. ");
            }
        }

        #[test]
        fn strips_internal_marker_characters_from_the_input() {
            assert_eq!(rules("a\u{E000}0\u{E001}b \u{E002}c", &config()), "a0b c ");
        }

        #[test]
        fn never_throws_whatever_the_input() {
            // The TS also passes `undefined`, which a `&str` cannot be.
            let cfg = config();
            let inputs = ["((((", "\\", "$1 $& $`", "\n\n\n", "...", ",", "\"", "new line", "period", "um um um"];
            for input in inputs {
                let out = std::panic::catch_unwind(|| rules(input, &cfg));
                assert!(out.is_ok(), "{input:?}");
            }
        }

        #[test]
        fn falls_back_to_the_raw_transcript_if_a_rule_fails() {
            // The TS breaks the config (`dictionary: null`), which the Rust types rule
            // out, so the failure is injected: an error, then a panic.
            let error = fancy_regex::Regex::new("(").err();
            assert_eq!(guarded(" hello ", || Err(error.expect("an invalid pattern").into())), "hello");
            assert_eq!(guarded(" hello ", || panic!("a broken rule")), "hello");
        }
    }
}
