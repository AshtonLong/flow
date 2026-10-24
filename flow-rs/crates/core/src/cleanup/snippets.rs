//! Snippets: a spoken trigger phrase expands to saved text.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;

use super::js::{self, Memo};
use super::text::{PHRASE_END, PHRASE_START, phrase_source};
use crate::config::Snippet;

const SLOT_OPEN: &str = "\u{E000}";
const SLOT_CLOSE: &str = "\u{E001}";

/// Punctuation and quotes a speech model may wrap around a lone trigger.
static WRAP_BEFORE: LazyLock<String> = LazyLock::new(|| format!(r#"[{}"'“”‘’(\[]*"#, js::SPACE));
static WRAP_AFTER: LazyLock<String> = LazyLock::new(|| format!(r#"[{}.,!?;:…"'“”‘’)\]]*"#, js::SPACE));

/// Punctuation a trigger may be saved with; it is not part of what is said.
const TRIGGER_PUNCTUATION: [char; 7] = ['.', ',', '!', '?', ';', ':', '…'];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpandedSnippets {
    /// The text with each expansion replaced by an opaque slot marker.
    pub text: String,
    /// Slot contents, restored with [`restore_snippets`].
    pub slots: Vec<String>,
}

struct Rule<'a> {
    source: String,
    text: &'a str,
    length: usize,
}

fn build_rules(snippets: &[Snippet]) -> Vec<Rule<'_>> {
    let mut rules = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for snippet in snippets {
        let Some(source) = phrase_source(snippet.trigger.trim_end_matches(TRIGGER_PUNCTUATION)) else {
            continue;
        };
        if !seen.insert(source.to_lowercase()) {
            continue;
        }
        rules.push(Rule { source, text: &snippet.text, length: js::utf16_len(&snippet.trigger) });
    }
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.length));
    rules
}

/// The compiled patterns for one set of rules: the lone-trigger test for each
/// rule, then the inline pattern.
struct Patterns {
    whole: Vec<FancyRegex>,
    inline: FancyRegex,
}

/// Cached by the inline pattern's source, which spells out every rule in order.
static PATTERNS: Memo<Patterns> = Memo::new();

static SELF_TERMINATED: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(&format!("[.,!?;:…{}]$", js::SPACE)));

/// Replaces each trigger with a slot marker and returns the saved texts
/// separately, so later rules (filler removal, spacing) cannot alter a snippet.
/// An utterance that is only the trigger, give or take punctuation, becomes
/// just the snippet.
///
/// Returns the text unchanged if a pattern cannot be run (see
/// [`super::apply_rules`] for the fallback that applies inside the full pipeline).
pub fn expand_snippets(text: &str, snippets: &[Snippet]) -> ExpandedSnippets {
    try_expand_snippets(text, snippets)
        .unwrap_or_else(|_| ExpandedSnippets { text: text.to_string(), slots: Vec::new() })
}

pub(crate) fn try_expand_snippets(text: &str, snippets: &[Snippet]) -> Result<ExpandedSnippets, js::RuleError> {
    let mut slots: Vec<String> = Vec::new();
    if text.is_empty() || snippets.is_empty() {
        return Ok(ExpandedSnippets { text: text.to_string(), slots });
    }
    let rules = build_rules(snippets);
    if rules.is_empty() {
        return Ok(ExpandedSnippets { text: text.to_string(), slots });
    }
    let mut slot = |value: &str| -> String {
        slots.push(value.to_string());
        format!("{SLOT_OPEN}{}{SLOT_CLOSE}", slots.len() - 1)
    };

    let alternatives: Vec<String> = rules.iter().map(|r| format!("({})", r.source)).collect();
    let inline_source = format!(r"(?i){PHRASE_START}(?:{}){PHRASE_END}([.,!?;:…]*)([ \t]*)", alternatives.join("|"));
    let patterns = PATTERNS.get(&inline_source, || -> Result<Patterns, js::RuleError> {
        let whole = rules
            .iter()
            .map(|rule| js::fancy(&format!("(?i)^{}{}{}$", WRAP_BEFORE.as_str(), rule.source, WRAP_AFTER.as_str())))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Patterns { whole, inline: js::fancy(&inline_source)? })
    })?;

    for (rule, whole) in rules.iter().zip(&patterns.whole) {
        if whole.is_match(text)? {
            let text = slot(rule.text);
            return Ok(ExpandedSnippets { text, slots });
        }
    }

    let out = js::replace_all(&patterns.inline, text, |caps| {
        let Some(rule) = (1..=rules.len()).find(|&i| caps.get(i).is_some()).and_then(|i| rules.get(i - 1)) else {
            return js::group_or_empty(caps, 0).to_string();
        };
        let punctuation = js::group_or_empty(caps, rules.len() + 1);
        // A snippet that already ends a clause (or a line) doesn't take the model's punctuation too.
        let self_terminated = SELF_TERMINATED.is_match(rule.text);
        let space = js::group_or_empty(caps, rules.len() + 2);
        // After a snippet that ends in a line break, the next word starts the new line.
        let gap = if rule.text.chars().next_back().is_some_and(js::is_space) { "" } else { space };
        let punctuation = if self_terminated { "" } else { punctuation };
        format!("{}{punctuation}{gap}", slot(rule.text))
    })?;
    Ok(ExpandedSnippets { text: out, slots })
}

static SLOT: LazyLock<Regex> = LazyLock::new(|| js::fixed_plain(&format!("{SLOT_OPEN}([0-9]+){SLOT_CLOSE}")));

/// Puts the saved texts back in place of their slot markers.
pub fn restore_snippets(text: &str, slots: &[String]) -> String {
    if slots.is_empty() {
        return text.to_string();
    }
    SLOT.replace_all(text, |c: &regex::Captures<'_>| {
        // `slots[Number(index)] ?? ''`: an index too large to parse is out of range too.
        c.get(1).and_then(|m| m.as_str().parse::<usize>().ok()).and_then(|i| slots.get(i)).cloned().unwrap_or_default()
    })
    .into_owned()
}

/// True if the text begins with a snippet slot (its casing must not be touched).
pub fn starts_with_snippet(text: &str) -> bool {
    text.starts_with(SLOT_OPEN)
}

/// Expands triggers in one step, for callers with no further rules to run.
pub fn apply_snippets(text: &str, snippets: &[Snippet]) -> String {
    let expanded = expand_snippets(text, snippets);
    restore_snippets(&expanded.text, &expanded.slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(trigger: &str, text: &str) -> Snippet {
        Snippet { trigger: trigger.to_string(), text: text.to_string() }
    }

    fn snippets() -> Vec<Snippet> {
        vec![
            snippet("my address", "221B Baker Street, London"),
            snippet("my address two", "Flat 2"),
            snippet("sign off", "Best,\nAshton\n"),
            snippet("Shrug.", "¯\\_(ツ)_/¯"),
        ]
    }

    mod apply_snippets {
        use super::*;

        macro_rules! cases {
            ($($name:ident: $input:expr => $expected:expr;)*) => {$(
                #[test]
                fn $name() {
                    assert_eq!(apply_snippets($input, &snippets()), $expected);
                }
            )*};
        }

        cases! {
            expands_a_lone_trigger: "my address" => "221B Baker Street, London";
            drops_punctuation_around_a_lone_trigger: "My address." => "221B Baker Street, London";
            drops_quotes_around_a_lone_trigger: "\"My address!\"" => "221B Baker Street, London";
            ignores_case: "MY ADDRESS" => "221B Baker Street, London";
            expands_inline_and_keeps_the_sentence_punctuation:
                "Send it to my address." => "Send it to 221B Baker Street, London.";
            expands_before_a_comma:
                "Send it to my address, please" => "Send it to 221B Baker Street, London, please";
            respects_word_boundaries: "all my addresses" => "all my addresses";
            prefers_the_longest_trigger: "use my address two" => "use Flat 2";
            outputs_a_multi_line_snippet_verbatim: "Sign off." => "Best,\nAshton\n";
            starts_the_next_word_on_the_new_line: "then sign off. Bye" => "then Best,\nAshton\nBye";
            ignores_punctuation_saved_with_the_trigger: "shrug" => "¯\\_(ツ)_/¯";
            tolerates_a_comma_inside_the_trigger: "my, address" => "221B Baker Street, London";
            handles_the_empty_string: "" => "";
        }

        #[test]
        fn does_nothing_without_snippets() {
            assert_eq!(apply_snippets("my address", &[]), "my address");
        }
    }

    mod expand_snippets {
        use super::*;

        #[test]
        fn hides_snippet_text_behind_slots_until_restored() {
            let expanded = expand_snippets("go to my address now", &snippets());
            assert_eq!(expanded.slots, ["221B Baker Street, London"]);
            assert!(!expanded.text.contains("Baker"));
            assert_eq!(restore_snippets(&expanded.text, &expanded.slots), "go to 221B Baker Street, London now");
        }
    }
}
