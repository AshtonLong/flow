//! What every cloud adapter shares: WAV upload, timing, cancellation and error mapping.
//! Port of `src/main/transcribe/cloud/base.ts`.

use std::time::{Duration, Instant};

use flow_core::errors::{TranscribeError, TranscribeErrorCode};
use flow_core::types::{SAMPLE_RATE, Transcript, samples_to_ms};
use flow_core::wav::encode_wav;
use reqwest::multipart::Part;
use serde_json::Value;

use crate::http::{HttpContext, describe_error, request_json};
use crate::provider::{Headers, ResolvedProvider, auth_headers, with_headers};
use crate::text::js_trim;
use crate::{TranscribeOptions, deepgram, elevenlabs, mistral, openai_compatible};

/// A provider that hasn't answered this long after the upload began is treated as unreachable.
const BASE_TIMEOUT_MS: f64 = 30_000.0;

/// How a provider's speech API is spoken; one adapter module per value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Adapter {
    OpenAiCompatible,
    Mistral,
    ElevenLabs,
    Deepgram,
}

/// What an adapter sends: the two things that differ per provider are how the
/// request is laid out and where the text sits in the response.
pub(crate) struct CloudRequest {
    pub url: String,
    pub headers: Headers,
    pub body: CloudBody,
}

pub(crate) enum CloudBody {
    Form(reqwest::multipart::Form),
    Bytes(Vec<u8>),
}

/// What an adapter needs to lay out its request.
pub(crate) struct BuildInput<'a> {
    /// Model name the provider knows.
    pub model: &'a str,
    pub provider: &'a ResolvedProvider,
    pub language: &'a str,
    /// Empty when the model takes no hints.
    pub hints: &'a [String],
}

impl Adapter {
    fn build(self, wav: Vec<u8>, input: &BuildInput<'_>) -> Result<CloudRequest, reqwest::Error> {
        match self {
            Self::OpenAiCompatible => openai_compatible::build(wav, input),
            Self::Mistral => mistral::build(wav, input),
            Self::ElevenLabs => elevenlabs::build(wav, input),
            Self::Deepgram => deepgram::build(wav, input),
        }
    }

    fn read(self, json: &Value) -> Option<&Value> {
        match self {
            Self::OpenAiCompatible | Self::Mistral | Self::ElevenLabs => json.get("text"),
            Self::Deepgram => deepgram::read(json),
        }
    }
}

/// A cloud speech model, ready to transcribe. Built by
/// [`create_cloud_transcriber`](crate::create_cloud_transcriber).
#[derive(Debug, Clone)]
pub struct CloudTranscriber {
    /// Flow model id, e.g. `groq/whisper-large-v3-turbo`.
    pub(crate) id: String,
    /// Model name the provider knows.
    pub(crate) model: String,
    pub(crate) provider: ResolvedProvider,
    pub(crate) adapter: Adapter,
    /// Send dictionary terms as a hint.
    pub(crate) use_hints: bool,
    pub(crate) http: reqwest::Client,
}

impl CloudTranscriber {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The provider the audio goes to.
    pub fn provider(&self) -> &ResolvedProvider {
        &self.provider
    }

    /// Uploads the audio as a WAV and returns the trimmed transcript. Fails only
    /// with TranscribeError; a cancel aborts the request and fails with `aborted`.
    pub async fn transcribe(
        &self,
        audio: &[f32],
        opts: &TranscribeOptions,
    ) -> Result<Transcript, TranscribeError> {
        let started = Instant::now();
        let audio_ms = samples_to_ms(audio.len());
        if opts.cancel.is_cancelled() {
            return Err(TranscribeError::aborted());
        }
        // Nothing to send; providers reject an empty file.
        if audio.is_empty() {
            return Ok(Transcript { text: String::new(), model: self.id.clone(), audio_ms: 0.0, elapsed_ms: 0.0 });
        }
        let name = self.provider.name.as_str();
        let input = BuildInput {
            model: &self.model,
            provider: &self.provider,
            language: &opts.language,
            hints: if self.use_hints { &opts.hints } else { &[] },
        };
        let request = self.adapter.build(encode_wav(audio, SAMPLE_RATE), &input).map_err(|err| {
            TranscribeError::new(
                TranscribeErrorCode::Network,
                format!("Request to {name} failed: {}", describe_error(&err)),
            )
            .with_user_message(format!("Couldn't reach {name}"))
            .with_cause(err)
        })?;
        let builder = with_headers(
            self.http.post(&request.url),
            auth_headers(&self.provider).into_iter().chain(request.headers),
        );
        let builder = match request.body {
            CloudBody::Form(form) => builder.multipart(form),
            CloudBody::Bytes(bytes) => builder.body(bytes),
        };
        let ctx = HttpContext {
            provider: name,
            cancel: &opts.cancel,
            // Long recordings take longer to upload and to transcribe.
            timeout: Duration::from_secs_f64((BASE_TIMEOUT_MS + audio_ms) / 1000.0),
        };
        let json = request_json(&ctx, builder).await?;
        let Some(text) = self.adapter.read(&json).and_then(Value::as_str) else {
            return Err(TranscribeError::new(
                TranscribeErrorCode::BadResponse,
                format!("{name} response has no transcript"),
            ));
        };
        Ok(Transcript {
            text: js_trim(text).to_string(),
            model: self.id.clone(),
            audio_ms,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// The WAV as a multipart file part.
pub(crate) fn wav_file(wav: Vec<u8>) -> Result<Part, reqwest::Error> {
    Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav")
}
