//! Port of `tests/unit/transcribe/cloud.test.ts`.

use std::time::Duration;

use flow_core::catalog::{Catalog, CloudModelEntry};
use flow_core::errors::{TranscribeError, TranscribeErrorCode};
use flow_core::protocol::settings::KeyTestResult;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::server::{Reply, Server, headers};
use super::{deps_with, providers, with_secrets};
use crate::{CloudDeps, TranscribeOptions, create_cloud_transcriber, test_provider_key};

/// A catalog cloud entry. The TS helper leaves `hints` unset, which means "on";
/// the Rust schema defaults it to off, so the caller says which.
fn cloud(id: &str, provider: &str, model: &str, hints: bool) -> CloudModelEntry {
    CloudModelEntry {
        id: id.into(),
        provider: provider.into(),
        model: model.into(),
        name: id.into(),
        description: String::new(),
        price_per_hour: 0.0,
        recommended: false,
        hints,
        min_billed_seconds: None,
        docs_url: None,
    }
}

fn catalog(server: &Server) -> Catalog {
    Catalog {
        version: 1,
        as_of: "2026-10-03".into(),
        providers: providers(server),
        local: Vec::new(),
        cloud: vec![
            cloud("groq/whisper-large-v3-turbo", "groq", "whisper-large-v3-turbo", true),
            cloud("groq/whisper-large-v3", "groq", "whisper-large-v3", false),
            cloud("openai/gpt-4o-mini-transcribe", "openai", "gpt-4o-mini-transcribe", true),
            cloud("openai/gpt-transcribe", "openai", "gpt-transcribe", true),
            cloud("mistral/voxtral-mini-transcribe-2", "mistral", "voxtral-mini-2602", true),
            cloud("elevenlabs/scribe_v2", "elevenlabs", "scribe_v2", true),
            cloud("deepgram/nova-3", "deepgram", "nova-3", true),
            cloud("deepgram/nova-2", "deepgram", "nova-2", true),
            cloud("anthropic/claude-haiku-4-5", "anthropic", "claude-haiku-4-5", true),
            cloud("nowhere/model", "nowhere", "model", true),
        ],
        cleanup: Vec::new(),
    }
}

fn deps(server: &Server, overrides: Value) -> CloudDeps {
    deps_with(catalog(server), overrides)
}

fn options() -> TranscribeOptions {
    TranscribeOptions { language: "en".into(), hints: Vec::new(), cancel: CancellationToken::new() }
}

fn hinted(hints: &[&str]) -> TranscribeOptions {
    TranscribeOptions { hints: hints.iter().map(|h| h.to_string()).collect(), ..options() }
}

fn numbered(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|i| format!("{prefix}{i}")).collect()
}

/// Half a second of audio: 8000 samples, a 16 044-byte WAV.
fn audio() -> Vec<f32> {
    vec![0.1; 8000]
}
const WAV_BYTES: usize = 44 + 8000 * 2;

fn expect_wav_file(form: &super::server::Form) {
    let file = form.part("file").expect("a file part");
    assert_eq!(file.data.len(), WAV_BYTES);
    assert_eq!(file.content_type.as_deref(), Some("audio/wav"));
    assert_eq!(file.file_name.as_deref(), Some("audio.wav"));
}

/// Transcribes [`audio`] with one model against a server with one reply.
async fn transcribe_once(
    id: &str,
    reply: Reply,
    overrides: Value,
    opts: TranscribeOptions,
) -> (Server, Result<flow_core::types::Transcript, TranscribeError>) {
    let server = Server::start(vec![reply]);
    let transcriber = create_cloud_transcriber(id, &deps(&server, overrides)).expect("a transcriber");
    let result = transcriber.transcribe(&audio(), &opts).await;
    (server, result)
}

mod openai_compatible {
    use super::*;

    #[tokio::test]
    async fn groq_multipart_upload_with_model_language_prompt_and_json_format() {
        let server = Server::start(vec![Reply::json(json!({ "text": " Hello from Tauri. " }))]);
        let transcriber = create_cloud_transcriber("groq/whisper-large-v3-turbo", &deps(&server, json!({}))).unwrap();
        assert_eq!(transcriber.id(), "groq/whisper-large-v3-turbo");
        // Safe to log: the key never prints.
        assert!(!format!("{transcriber:?}").contains("gsk-test"));

        let result = transcriber.transcribe(&audio(), &hinted(&["Tauri", "Parakeet"])).await.unwrap();
        assert_eq!(result.text, "Hello from Tauri.");
        assert_eq!(result.model, "groq/whisper-large-v3-turbo");
        assert_eq!(result.audio_ms, 500.0);
        assert!(result.elapsed_ms >= 0.0);

        let calls = server.calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.path(), "/groq/openai/v1/audio/transcriptions");
        assert_eq!(call.method, "POST");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer gsk-test")]));
        let form = call.form();
        expect_wav_file(&form);
        assert_eq!(form.get("model").as_deref(), Some("whisper-large-v3-turbo"));
        assert_eq!(form.get("language").as_deref(), Some("en"));
        assert_eq!(form.get("response_format").as_deref(), Some("json"));
        assert_eq!(form.get("prompt").as_deref(), Some("Tauri, Parakeet"));
    }

    #[tokio::test]
    async fn uploads_a_real_wav_of_the_audio() {
        let (server, result) =
            transcribe_once("groq/whisper-large-v3-turbo", Reply::json(json!({ "text": "x" })), json!({}), options()).await;
        result.unwrap();
        let bytes = server.call(0).form().part("file").unwrap().data.clone();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]), 16000);
        let first = i16::from_le_bytes([bytes[44], bytes[45]]);
        assert_eq!(first, (f64::from(0.1f32) * 32767.0).round() as i16);
    }

    #[tokio::test]
    async fn sends_no_prompt_without_hints() {
        let (server, result) =
            transcribe_once("groq/whisper-large-v3-turbo", Reply::json(json!({ "text": "x" })), json!({}), options()).await;
        result.unwrap();
        assert!(!server.call(0).form().has("prompt"));
    }

    #[tokio::test]
    async fn sends_no_hints_to_a_model_the_catalog_marks_hints_false() {
        let (server, result) =
            transcribe_once("groq/whisper-large-v3", Reply::json(json!({ "text": "x" })), json!({}), hinted(&["Tauri"]))
                .await;
        result.unwrap();
        assert!(!server.call(0).form().has("prompt"));
    }

    #[tokio::test]
    async fn keeps_the_prompt_within_the_whisper_limit_whole_terms_only() {
        let opts = TranscribeOptions { hints: numbered("Term", 400), ..options() };
        let (server, result) =
            transcribe_once("groq/whisper-large-v3-turbo", Reply::json(json!({ "text": "x" })), json!({}), opts).await;
        result.unwrap();
        let prompt = server.call(0).form().get("prompt").unwrap();
        assert!(prompt.chars().count() <= 600, "{}", prompt.len());
        assert!(prompt.starts_with("Term0, Term1, "));
        let last = prompt.rsplit(", ").next().unwrap();
        assert!(last.strip_prefix("Term").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
    }

    #[tokio::test]
    async fn openai_gpt_4o_mini_transcribe_uses_the_same_fields() {
        let (server, result) = transcribe_once(
            "openai/gpt-4o-mini-transcribe",
            Reply::json(json!({ "text": "ok" })),
            json!({}),
            hinted(&["Tauri"]),
        )
        .await;
        result.unwrap();
        let call = server.call(0);
        assert_eq!(call.path(), "/openai/v1/audio/transcriptions");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer sk-test")]));
        let form = call.form();
        assert_eq!(form.get("model").as_deref(), Some("gpt-4o-mini-transcribe"));
        assert_eq!(form.get("language").as_deref(), Some("en"));
        assert_eq!(form.get("prompt").as_deref(), Some("Tauri"));
        assert_eq!(form.get("response_format").as_deref(), Some("json"));
    }

    #[tokio::test]
    async fn openai_gpt_transcribe_takes_languages_and_keywords_instead() {
        let (server, result) = transcribe_once(
            "openai/gpt-transcribe",
            Reply::json(json!({ "text": "ok", "languages": [{ "code": "en" }] })),
            json!({}),
            hinted(&["Tauri", "premium <plan>", "two\nlines"]),
        )
        .await;
        assert_eq!(result.unwrap().text, "ok");
        let form = server.call(0).form();
        assert_eq!(form.get("model").as_deref(), Some("gpt-transcribe"));
        assert_eq!(form.get_all("languages[]"), ["en"]);
        assert_eq!(form.get_all("keywords[]"), ["Tauri", "premium plan", "two lines"]);
        assert!(!form.has("language"));
        assert!(!form.has("prompt"));
        assert!(!form.has("response_format"));
    }

    #[tokio::test]
    async fn custom_endpoint_base_url_and_model_from_the_config_key_from_its_secret_reference() {
        let server = Server::start(vec![Reply::json(json!({ "text": "ok" }))]);
        let d = deps(
            &server,
            json!({
                "providers": {
                    "custom-lan": {
                        "base_url": server.url("/lan/v1/"),
                        "api_key": "secret:lan-key",
                        "model": "Systran/faster-whisper-large-v3",
                    },
                },
            }),
        );
        let transcriber = create_cloud_transcriber("custom/custom-lan", &d).unwrap();
        assert_eq!(transcriber.id(), "custom/custom-lan");
        let result = transcriber.transcribe(&audio(), &hinted(&["Tauri"])).await.unwrap();
        assert_eq!(result.model, "custom/custom-lan");
        let call = server.call(0);
        assert_eq!(call.path(), "/lan/v1/audio/transcriptions");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer lan-secret")]));
        let form = call.form();
        assert_eq!(form.get("model").as_deref(), Some("Systran/faster-whisper-large-v3"));
        assert_eq!(form.get("prompt").as_deref(), Some("Tauri"));
        expect_wav_file(&form);
    }

    #[tokio::test]
    async fn custom_endpoint_key_optional_model_defaults_to_whisper_1() {
        let server = Server::start(vec![Reply::json(json!({ "text": "ok" }))]);
        let d = deps(&server, json!({ "providers": { "lan": { "base_url": server.url("/v1") } } }));
        create_cloud_transcriber("custom/lan", &d).unwrap().transcribe(&audio(), &options()).await.unwrap();
        let call = server.call(0);
        assert_eq!(call.path(), "/v1/audio/transcriptions");
        assert_eq!(call.sent_headers(), headers(&[]));
        assert_eq!(call.form().get("model").as_deref(), Some("whisper-1"));
    }

    #[tokio::test]
    async fn base_url_in_the_config_overrides_a_catalog_provider() {
        let server = Server::start(vec![Reply::json(json!({ "text": "ok" }))]);
        let d = deps(&server, json!({ "providers": { "groq": { "base_url": server.url("/proxy/v1") } } }));
        create_cloud_transcriber("groq/whisper-large-v3-turbo", &d)
            .unwrap()
            .transcribe(&audio(), &options())
            .await
            .unwrap();
        let call = server.call(0);
        assert_eq!(call.path(), "/proxy/v1/audio/transcriptions");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer gsk-test")]));
    }
}

mod mistral {
    use super::*;

    #[tokio::test]
    async fn voxtral_multipart_upload_with_context_bias_terms() {
        let opts = TranscribeOptions {
            language: "en-US".into(),
            ..hinted(&["Tauri", "Baker Street", "a,b", "tauri"])
        };
        let (server, result) = transcribe_once(
            "mistral/voxtral-mini-transcribe-2",
            Reply::json(json!({ "model": "voxtral-mini-2602", "text": " Bonjour. " })),
            json!({}),
            opts,
        )
        .await;
        let result = result.unwrap();
        assert_eq!(result.text, "Bonjour.");
        assert_eq!(result.model, "mistral/voxtral-mini-transcribe-2");
        let call = server.call(0);
        assert_eq!(call.path(), "/mistral/v1/audio/transcriptions");
        assert_eq!(call.method, "POST");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer mistral-test")]));
        let form = call.form();
        expect_wav_file(&form);
        assert_eq!(form.get("model").as_deref(), Some("voxtral-mini-2602"));
        assert_eq!(form.get("language").as_deref(), Some("en"));
        // The API rejects spaces and commas in a bias term.
        assert_eq!(form.get_all("context_bias"), ["Tauri", "Baker", "Street", "a", "b"]);
    }

    #[tokio::test]
    async fn caps_context_bias_at_100_terms() {
        let opts = TranscribeOptions { hints: numbered("Term", 150), ..options() };
        let (server, result) =
            transcribe_once("mistral/voxtral-mini-transcribe-2", Reply::json(json!({ "text": "x" })), json!({}), opts)
                .await;
        result.unwrap();
        assert_eq!(server.call(0).form().get_all("context_bias").len(), 100);
    }
}

mod elevenlabs {
    use super::*;

    #[tokio::test]
    async fn scribe_v2_xi_api_key_model_id_and_keyterms() {
        let long = "x".repeat(50);
        let (server, result) = transcribe_once(
            "elevenlabs/scribe_v2",
            Reply::json(json!({ "language_code": "en", "language_probability": 1, "text": " Hello. ", "words": [] })),
            json!({}),
            hinted(&["Tauri", "Baker Street", &long, "one two three four five six"]),
        )
        .await;
        assert_eq!(result.unwrap().text, "Hello.");
        let call = server.call(0);
        assert_eq!(call.path(), "/elevenlabs/v1/speech-to-text");
        assert_eq!(call.method, "POST");
        assert_eq!(call.sent_headers(), headers(&[("xi-api-key", "xi-test")]));
        let form = call.form();
        expect_wav_file(&form);
        assert_eq!(form.get("model_id").as_deref(), Some("scribe_v2"));
        assert_eq!(form.get("language_code").as_deref(), Some("en"));
        assert_eq!(form.get("tag_audio_events").as_deref(), Some("false"));
        // Under 50 characters and at most five words each.
        assert_eq!(form.get_all("keyterms"), ["Tauri", "Baker Street"]);
    }

    #[tokio::test]
    async fn caps_keyterms_at_100_to_stay_under_the_20_second_minimum_billing() {
        let opts = TranscribeOptions { hints: numbered("Term", 300), ..options() };
        let (server, result) =
            transcribe_once("elevenlabs/scribe_v2", Reply::json(json!({ "text": "x" })), json!({}), opts).await;
        result.unwrap();
        assert_eq!(server.call(0).form().get_all("keyterms").len(), 100);
    }
}

mod deepgram {
    use super::*;

    fn response() -> Reply {
        Reply::json(json!({
            "results": { "channels": [{ "alternatives": [{ "transcript": " Hello there. ", "confidence": 0.99 }] }] },
        }))
    }

    #[tokio::test]
    async fn nova_3_raw_wav_body_token_auth_smart_formatting_and_keyterms() {
        let (server, result) =
            transcribe_once("deepgram/nova-3", response(), json!({}), hinted(&["Tauri", "Baker Street"])).await;
        let result = result.unwrap();
        assert_eq!(result.text, "Hello there.");
        assert_eq!(result.model, "deepgram/nova-3");
        let call = server.call(0);
        assert_eq!(call.path(), "/deepgram/v1/listen");
        assert_eq!(call.query_get("model").as_deref(), Some("nova-3"));
        assert_eq!(call.query_get("language").as_deref(), Some("en"));
        assert_eq!(call.query_get("smart_format").as_deref(), Some("true"));
        assert_eq!(call.query_all("keyterm"), ["Tauri", "Baker Street"]);
        assert!(call.target.contains("keyterm=Baker+Street"), "{}", call.target);
        assert_eq!(call.method, "POST");
        assert_eq!(
            call.sent_headers(),
            headers(&[("Authorization", "Token dg-test"), ("Content-Type", "audio/wav")])
        );
        assert_eq!(call.body.len(), WAV_BYTES);
        assert_eq!(&call.body[0..4], b"RIFF");
    }

    #[tokio::test]
    async fn caps_keyterms_at_100() {
        let opts = TranscribeOptions { hints: numbered("T", 300), ..options() };
        let (server, result) = transcribe_once("deepgram/nova-3", response(), json!({}), opts).await;
        result.unwrap();
        assert_eq!(server.call(0).query_all("keyterm").len(), 100);
    }

    #[tokio::test]
    async fn sends_no_keyterms_to_a_model_without_keyterm_prompting() {
        let (server, result) = transcribe_once("deepgram/nova-2", response(), json!({}), hinted(&["Tauri"])).await;
        result.unwrap();
        assert!(server.call(0).query_all("keyterm").is_empty());
    }

    #[tokio::test]
    async fn treats_an_empty_transcript_as_empty_text() {
        let reply = Reply::json(json!({ "results": { "channels": [{ "alternatives": [{ "transcript": "" }] }] } }));
        let (_server, result) = transcribe_once("deepgram/nova-3", reply, json!({}), options()).await;
        assert_eq!(result.unwrap().text, "");
    }
}

mod cannot_build {
    use super::*;

    fn build(id: &str, d: &CloudDeps) -> TranscribeError {
        match create_cloud_transcriber(id, d) {
            Ok(_) => panic!("expected \"{id}\" to fail"),
            Err(err) => err,
        }
    }

    #[test]
    fn unknown_models() {
        let cases = [
            ("a model that is not in the catalog", "groq/whisper-tiny"),
            ("a local model id", "parakeet-tdt-0.6b-v2"),
            ("a model whose provider is unknown", "nowhere/model"),
            ("a provider with no speech API", "anthropic/claude-haiku-4-5"),
            ("a custom endpoint that is not configured", "custom/missing"),
            ("a custom id with no provider", "custom/"),
        ];
        for (name, id) in cases {
            let server = Server::start(vec![Reply::json(json!({}))]);
            assert_eq!(build(id, &deps(&server, json!({}))).code, TranscribeErrorCode::UnknownModel, "{name}");
            assert!(server.calls().is_empty(), "{name}");
        }
    }

    #[test]
    fn a_custom_provider_with_a_key_but_no_base_url_is_an_unknown_model() {
        let server = Server::start(vec![Reply::json(json!({}))]);
        let d = deps(&server, json!({ "providers": { "lan": { "api_key": "secret:lan-key" } } }));
        let err = build("custom/lan", &d);
        assert_eq!(err.code, TranscribeErrorCode::UnknownModel);
        assert_eq!(err.user_message(), "Custom endpoint is not configured");
    }

    #[test]
    fn no_stored_key_is_no_key_naming_the_provider() {
        let server = Server::start(vec![Reply::json(json!({}))]);
        let err = build("groq/whisper-large-v3-turbo", &with_secrets(deps(&server, json!({})), |_| None));
        assert_eq!(err.code, TranscribeErrorCode::NoKey);
        assert!(err.user_message().contains("Groq"));
        assert!(server.calls().is_empty());
    }

    #[test]
    fn a_blank_stored_key_counts_as_no_key() {
        let server = Server::start(vec![Reply::json(json!({}))]);
        let d = with_secrets(deps(&server, json!({})), |_| Some("  ".into()));
        assert_eq!(build("groq/whisper-large-v3-turbo", &d).code, TranscribeErrorCode::NoKey);
    }
}

mod failures {
    use super::*;

    const IDS: [&str; 4] =
        ["groq/whisper-large-v3-turbo", "mistral/voxtral-mini-transcribe-2", "elevenlabs/scribe_v2", "deepgram/nova-3"];

    async fn failure(id: &str, reply: Reply, opts: TranscribeOptions) -> TranscribeError {
        let (_server, result) = transcribe_once(id, reply, json!({}), opts).await;
        result.expect_err("a failure")
    }

    #[tokio::test]
    async fn maps_each_failure() {
        let cases: Vec<(&str, Reply, TranscribeErrorCode, Option<u16>)> = vec![
            (
                "401",
                Reply::status_json(401, json!({ "error": { "message": "Invalid API Key" } })),
                TranscribeErrorCode::Auth,
                Some(401),
            ),
            ("403", Reply::status_text(403, "Forbidden"), TranscribeErrorCode::Auth, Some(403)),
            (
                "429",
                Reply::status_json(429, json!({ "error": { "message": "Slow down" } })),
                TranscribeErrorCode::RateLimit,
                Some(429),
            ),
            (
                "400",
                Reply::status_json(400, json!({ "error": { "message": "bad audio" } })),
                TranscribeErrorCode::Http,
                Some(400),
            ),
            ("500", Reply::status_text(500, "oops"), TranscribeErrorCode::Http, Some(500)),
            ("a rejected fetch", Reply::Fail, TranscribeErrorCode::Network, None),
            ("a body that is not JSON", Reply::text("<html>gateway</html>"), TranscribeErrorCode::BadResponse, Some(200)),
            ("JSON without a transcript", Reply::json(json!({ "unexpected": true })), TranscribeErrorCode::BadResponse, None),
        ];
        for id in IDS {
            for (name, reply, code, status) in &cases {
                let err = failure(id, reply.clone(), options()).await;
                assert_eq!(err.code, *code, "{id}: {name}: {}", err.message);
                assert_eq!(err.status, *status, "{id}: {name}");
                assert!(!err.user_message().is_empty(), "{id}: {name}");
            }
        }
    }

    #[tokio::test]
    async fn maps_a_cancel_to_aborted() {
        for id in IDS {
            let server = Server::start(vec![Reply::Hang]);
            let transcriber = create_cloud_transcriber(id, &deps(&server, json!({}))).unwrap();
            let opts = options();
            let cancel = opts.cancel.clone();
            let pending = tokio::spawn(async move { transcriber.transcribe(&audio(), &opts).await });
            assert!(server.wait_for_calls(1).await, "{id}");
            assert_eq!(server.calls().len(), 1, "{id}");
            cancel.cancel();
            let err = pending.await.unwrap().expect_err("a cancel");
            assert_eq!(err.code, TranscribeErrorCode::Aborted, "{id}");
            // The request was abandoned: the connection is gone.
            assert!(server.wait_closed(0).await, "{id}");
        }
    }

    #[tokio::test]
    async fn does_not_send_anything_when_already_cancelled() {
        for id in IDS {
            let opts = options();
            opts.cancel.cancel();
            let (server, result) = transcribe_once(id, Reply::json(json!({ "text": "x" })), json!({}), opts).await;
            assert_eq!(result.expect_err("a cancel").code, TranscribeErrorCode::Aborted, "{id}");
            assert!(server.calls().is_empty(), "{id}");
        }
    }

    #[tokio::test]
    async fn includes_the_provider_and_its_message_for_the_log() {
        let err = failure(
            "groq/whisper-large-v3-turbo",
            Reply::status_json(401, json!({ "error": { "message": "Invalid API Key" } })),
            options(),
        )
        .await;
        assert_eq!(err.message, "Groq responded 401: Invalid API Key");
        assert_eq!(err.user_message(), "Groq rejected the API key");
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_on_a_provider_that_never_answers() {
        let server = Server::start(vec![Reply::Hang]);
        let transcriber =
            create_cloud_transcriber("groq/whisper-large-v3-turbo", &deps(&server, json!({}))).unwrap();
        let started = tokio::time::Instant::now();
        let pending = tokio::spawn(async move { transcriber.transcribe(&audio(), &options()).await });
        assert!(server.wait_for_calls(1).await);
        tokio::time::advance(Duration::from_millis(30_000)).await;
        assert!(!server.is_closed(0));
        assert!(!pending.is_finished());
        // Half a second of audio adds half a second to the 30 s base.
        let err = pending.await.unwrap().expect_err("a timeout");
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(30_500) && waited < Duration::from_millis(30_600), "{waited:?}");
        assert_eq!(err.code, TranscribeErrorCode::Network);
        assert_eq!(err.user_message(), "Groq timed out");
        assert!(server.wait_closed(0).await);
    }

    #[tokio::test]
    async fn returns_empty_text_for_empty_audio_without_a_request() {
        let server = Server::start(vec![Reply::json(json!({ "text": "x" }))]);
        let result = create_cloud_transcriber("groq/whisper-large-v3-turbo", &deps(&server, json!({})))
            .unwrap()
            .transcribe(&[], &options())
            .await
            .unwrap();
        assert_eq!(
            result,
            flow_core::types::Transcript {
                text: String::new(),
                model: "groq/whisper-large-v3-turbo".into(),
                audio_ms: 0.0,
                elapsed_ms: 0.0,
            }
        );
        assert!(server.calls().is_empty());
    }
}

mod test_provider_key {
    use super::*;

    fn result(ok: bool, message: &str) -> KeyTestResult {
        KeyTestResult { ok, message: message.into() }
    }

    #[tokio::test]
    async fn one_authenticated_get() {
        type Case<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
        let cases: [Case; 6] = [
            ("groq", "/groq/openai/v1/models", &[("Authorization", "Bearer gsk-test")]),
            ("openai", "/openai/v1/models", &[("Authorization", "Bearer sk-test")]),
            ("mistral", "/mistral/v1/models", &[("Authorization", "Bearer mistral-test")]),
            ("elevenlabs", "/elevenlabs/v1/user", &[("xi-api-key", "xi-test")]),
            ("deepgram", "/deepgram/v1/projects", &[("Authorization", "Token dg-test")]),
            (
                "anthropic",
                "/anthropic/v1/models?limit=1",
                &[("x-api-key", "sk-ant-test"), ("anthropic-version", "2023-06-01")],
            ),
        ];
        for (provider, target, expected) in cases {
            let server = Server::start(vec![Reply::json(json!({ "data": [] }))]);
            let outcome = test_provider_key(provider, &deps(&server, json!({}))).await;
            assert_eq!(outcome, result(true, "Key works"), "{provider}");
            let calls = server.calls();
            assert_eq!(calls.len(), 1, "{provider}");
            assert_eq!(calls[0].target, target, "{provider}");
            assert_eq!(calls[0].method, "GET", "{provider}");
            assert_eq!(calls[0].sent_headers(), headers(expected), "{provider}");
        }
    }

    #[tokio::test]
    async fn a_keyless_provider_is_checked_for_reachability() {
        let server = Server::start(vec![Reply::json(json!({ "data": [] }))]);
        let outcome = test_provider_key("ollama", &deps(&server, json!({}))).await;
        assert_eq!(outcome, result(true, "Ollama is reachable"));
        let call = server.call(0);
        assert_eq!(call.target, "/ollama/v1/models");
        assert_eq!(call.sent_headers(), headers(&[]));
    }

    #[tokio::test]
    async fn a_custom_endpoint_uses_its_base_url_and_secret_reference() {
        let server = Server::start(vec![Reply::json(json!({ "data": [] }))]);
        let d = deps(
            &server,
            json!({ "providers": { "lan": { "base_url": server.url("/v1"), "api_key": "secret:lan-key" } } }),
        );
        assert!(test_provider_key("lan", &d).await.ok);
        let call = server.call(0);
        assert_eq!(call.target, "/v1/models");
        assert_eq!(call.sent_headers(), headers(&[("Authorization", "Bearer lan-secret")]));
    }

    #[tokio::test]
    async fn maps_responses() {
        let cases = [
            (
                "a rejected key",
                Reply::status_json(401, json!({ "error": { "message": "Invalid API Key" } })),
                false,
                "Groq rejected the key (401)",
            ),
            ("a forbidden key", Reply::status(403), false, "Groq rejected the key (403)"),
            ("a rate limit", Reply::status(429), true, "Key accepted, but rate limited right now"),
            ("a server error", Reply::status_text(500, "boom"), false, "Groq returned 500: boom"),
            ("a network failure", Reply::Fail, false, "Couldn't reach Groq"),
        ];
        for (name, reply, ok, message) in cases {
            let server = Server::start(vec![reply]);
            assert_eq!(test_provider_key("groq", &deps(&server, json!({}))).await, result(ok, message), "{name}");
        }
    }

    #[tokio::test]
    async fn accepts_a_restricted_elevenlabs_key_that_cannot_read_the_account() {
        let server = Server::start(vec![Reply::status_json(
            401,
            json!({ "detail": { "status": "missing_permissions", "message": "Missing user_read" } }),
        )]);
        assert_eq!(
            test_provider_key("elevenlabs", &deps(&server, json!({}))).await,
            result(true, "Key works (restricted key)")
        );
    }

    #[tokio::test]
    async fn reports_a_missing_key_without_a_request() {
        let server = Server::start(vec![Reply::json(json!({}))]);
        let d = with_secrets(deps(&server, json!({})), |_| None);
        assert_eq!(test_provider_key("groq", &d).await, result(false, "No API key saved for Groq"));
        assert!(server.calls().is_empty());
    }

    #[tokio::test]
    async fn reports_an_unknown_provider() {
        let server = Server::start(vec![Reply::json(json!({}))]);
        assert_eq!(
            test_provider_key("nowhere", &deps(&server, json!({}))).await,
            result(false, "Unknown provider \"nowhere\"")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_without_throwing() {
        let server = Server::start(vec![Reply::Hang]);
        let d = deps(&server, json!({}));
        let started = tokio::time::Instant::now();
        let pending = tokio::spawn(async move { test_provider_key("groq", &d).await });
        assert!(server.wait_for_calls(1).await);
        assert_eq!(pending.await.unwrap(), result(false, "Groq did not respond"));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_secs(10) && waited < Duration::from_millis(10_100), "{waited:?}");
    }
}
