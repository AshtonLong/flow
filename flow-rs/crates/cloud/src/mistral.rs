//! Mistral Voxtral transcription. `POST {base_url}/audio/transcriptions`, multipart.
//! Docs: https://docs.mistral.ai/api/endpoint/audio/transcriptions
//! Port of `src/main/transcribe/cloud/mistral.ts`.

use reqwest::multipart::Form;

use crate::base::{BuildInput, CloudBody, CloudRequest, wav_file};
use crate::hints::{HintLimits, limit_hints};
use crate::text::is_js_space;

/// "Provide up to 100 words or phrases" (context biasing).
const MAX_CONTEXT_BIAS: usize = 100;

/// The API schema only accepts bias terms without spaces or commas
/// (`^[^,\s]+$`), so a multi-word hint is sent as its separate words.
pub fn context_bias<S: AsRef<str>>(hints: &[S]) -> Vec<String> {
    let words: Vec<&str> = hints
        .iter()
        .flat_map(|hint| hint.as_ref().split(|c: char| c == ',' || is_js_space(c)))
        .collect();
    limit_hints(&words, HintLimits { max_terms: Some(MAX_CONTEXT_BIAS), ..HintLimits::default() })
}

/// The two-letter language code at the start of `language`, lowercased (`en-US` → `en`).
fn two_letter_language(language: &str) -> Option<String> {
    let prefix = language.get(..2)?;
    prefix.bytes().all(|b| b.is_ascii_alphabetic()).then(|| prefix.to_ascii_lowercase())
}

pub(crate) fn build(wav: Vec<u8>, input: &BuildInput<'_>) -> Result<CloudRequest, reqwest::Error> {
    let mut form = Form::new().part("file", wav_file(wav)?).text("model", input.model.to_string());
    // The schema wants exactly two letters; `en-US` would be rejected.
    if let Some(language) = two_letter_language(input.language) {
        form = form.text("language", language);
    }
    for term in context_bias(input.hints) {
        form = form.text("context_bias", term);
    }
    Ok(CloudRequest {
        url: format!("{}/audio/transcriptions", input.provider.base_url),
        headers: Vec::new(),
        body: CloudBody::Form(form),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_leading_two_letters() {
        assert_eq!(two_letter_language("en-US").as_deref(), Some("en"));
        assert_eq!(two_letter_language("EN").as_deref(), Some("en"));
        assert_eq!(two_letter_language("e"), None);
        assert_eq!(two_letter_language(""), None);
        assert_eq!(two_letter_language("1a"), None);
        assert_eq!(two_letter_language("é"), None);
    }
}
