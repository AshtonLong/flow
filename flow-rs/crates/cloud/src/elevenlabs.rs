//! ElevenLabs Scribe speech-to-text. `POST {base_url}/speech-to-text`, multipart, `xi-api-key`.
//! Docs: https://elevenlabs.io/docs/api-reference/speech-to-text/convert
//! Port of `src/main/transcribe/cloud/elevenlabs.ts`.

use reqwest::multipart::Form;

use crate::base::{BuildInput, CloudBody, CloudRequest, wav_file};
use crate::hints::{HintLimits, limit_hints};

/// The API allows 1000 keyterms, but more than 100 switches on a 20-second
/// minimum billed duration per request, which would dwarf a short dictation.
const MAX_KEYTERMS: usize = 100;

/// "The length of each keyterm must be less than 50 characters", "at most 5 words".
pub fn keyterms<S: AsRef<str>>(hints: &[S]) -> Vec<String> {
    limit_hints(
        hints,
        HintLimits {
            max_terms: Some(MAX_KEYTERMS),
            max_term_chars: Some(49),
            max_words: Some(5),
            ..HintLimits::default()
        },
    )
}

pub(crate) fn build(wav: Vec<u8>, input: &BuildInput<'_>) -> Result<CloudRequest, reqwest::Error> {
    let mut form = Form::new().part("file", wav_file(wav)?).text("model_id", input.model.to_string());
    if !input.language.is_empty() {
        form = form.text("language_code", input.language.to_string());
    }
    // Dictation wants words only, not "(laughter)".
    form = form.text("tag_audio_events", "false");
    for term in keyterms(input.hints) {
        form = form.text("keyterms", term);
    }
    Ok(CloudRequest {
        url: format!("{}/speech-to-text", input.provider.base_url),
        headers: Vec::new(),
        body: CloudBody::Form(form),
    })
}
