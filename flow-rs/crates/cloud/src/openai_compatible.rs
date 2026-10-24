//! OpenAI-compatible transcription: Groq, OpenAI and custom endpoints.
//! `POST {base_url}/audio/transcriptions`, multipart.
//! Docs: https://console.groq.com/docs/speech-to-text
//!       https://developers.openai.com/api/docs/guides/speech-to-text
//! Port of `src/main/transcribe/cloud/openai-compatible.ts`.

use reqwest::multipart::Form;

use crate::base::{BuildInput, CloudBody, CloudRequest, wav_file};
use crate::hints::{HintLimits, limit_hints};
use crate::text::starts_with_ascii_ci;

/// Whisper reads at most 224 tokens of prompt (Groq and OpenAI both document
/// it). Names tokenise badly, so budget about three characters per token.
const PROMPT_CHAR_BUDGET: usize = 600;

/// `gpt-transcribe` takes a keyword list instead; keep it to a sensible size.
const MAX_KEYWORDS: usize = 100;

/// `gpt-transcribe` replaced `language` with `languages[]` and added `keywords[]`.
fn uses_keyword_fields(model: &str) -> bool {
    starts_with_ascii_ci(model, "gpt-transcribe")
}

/// The hint terms as a Whisper-style prompt: a short glossary the model treats as prior text.
pub fn hint_prompt<S: AsRef<str>>(hints: &[S]) -> String {
    limit_hints(hints, HintLimits { max_total_chars: Some(PROMPT_CHAR_BUDGET), ..HintLimits::default() })
        .join(", ")
}

pub(crate) fn build(wav: Vec<u8>, input: &BuildInput<'_>) -> Result<CloudRequest, reqwest::Error> {
    let mut form = Form::new().part("file", wav_file(wav)?).text("model", input.model.to_string());
    if uses_keyword_fields(input.model) {
        if !input.language.is_empty() {
            form = form.text("languages[]", input.language.to_string());
        }
        // Keywords must stay on one line and may not contain angle brackets.
        let cleaned: Vec<String> =
            input.hints.iter().map(|h| h.replace(['<', '>', '\r', '\n'], " ")).collect();
        let keywords = limit_hints(&cleaned, HintLimits { max_terms: Some(MAX_KEYWORDS), ..HintLimits::default() });
        for keyword in keywords {
            form = form.text("keywords[]", keyword);
        }
    } else {
        form = form.text("response_format", "json");
        if !input.language.is_empty() {
            form = form.text("language", input.language.to_string());
        }
        let prompt = hint_prompt(input.hints);
        if !prompt.is_empty() {
            form = form.text("prompt", prompt);
        }
    }
    Ok(CloudRequest {
        url: format!("{}/audio/transcriptions", input.provider.base_url),
        headers: Vec::new(),
        body: CloudBody::Form(form),
    })
}
