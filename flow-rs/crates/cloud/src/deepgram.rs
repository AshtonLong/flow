//! Deepgram pre-recorded transcription. `POST {base_url}/listen` with the audio
//! as the raw request body and options in the query string.
//! Docs: https://developers.deepgram.com/docs/pre-recorded-audio
//!       https://developers.deepgram.com/docs/keyterm
//! Port of `src/main/transcribe/cloud/deepgram.ts`.

use serde_json::Value;
use url::form_urlencoded;

use crate::base::{BuildInput, CloudBody, CloudRequest};
use crate::hints::{HintLimits, limit_hints};
use crate::text::starts_with_ascii_ci;

/// "Maximum of 100 important terms per request".
const MAX_KEYTERMS: usize = 100;
/// Keyterms are limited to 500 tokens per request; allow about two characters per token.
const KEYTERM_CHAR_BUDGET: usize = 1000;

/// Keyterm prompting exists on Nova-3 (and Flux); older models reject the parameter.
fn supports_keyterms(model: &str) -> bool {
    starts_with_ascii_ci(model, "nova-3") || starts_with_ascii_ci(model, "flux")
}

pub(crate) fn build(wav: Vec<u8>, input: &BuildInput<'_>) -> Result<CloudRequest, reqwest::Error> {
    // `URLSearchParams` encoding: spaces become `+`.
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("model", input.model).append_pair("smart_format", "true");
    if !input.language.is_empty() {
        query.append_pair("language", input.language);
    }
    if supports_keyterms(input.model) {
        let terms = limit_hints(
            input.hints,
            HintLimits {
                max_terms: Some(MAX_KEYTERMS),
                max_total_chars: Some(KEYTERM_CHAR_BUDGET),
                ..HintLimits::default()
            },
        );
        for term in &terms {
            query.append_pair("keyterm", term);
        }
    }
    Ok(CloudRequest {
        url: format!("{}/listen?{}", input.provider.base_url, query.finish()),
        headers: vec![("Content-Type", "audio/wav".to_string())],
        body: CloudBody::Bytes(wav),
    })
}

/// `results.channels[0].alternatives[0].transcript`.
pub(crate) fn read(json: &Value) -> Option<&Value> {
    json.pointer("/results/channels/0/alternatives/0/transcript")
}
