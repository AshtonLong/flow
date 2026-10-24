//! One HTTP round trip for a cloud adapter, with every failure mapped to a TranscribeError.
//! Port of `src/main/transcribe/cloud/http.ts`.

use std::error::Error;
use std::time::Duration;

use flow_core::errors::{TranscribeError, TranscribeErrorCode};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::text::{collapse_spaces, js_trim, js_truncate};

pub(crate) struct HttpContext<'a> {
    /// Provider name for error messages, e.g. `Groq`.
    pub provider: &'a str,
    /// The caller's cancel token (Esc).
    pub cancel: &'a CancellationToken,
    /// Gives up on a provider that never answers, so the local fallback can run.
    pub timeout: Duration,
}

/// Pulls the human-readable message out of the error body shapes the providers use.
pub fn error_detail(body: &str) -> String {
    let text = js_trim(body);
    if text.is_empty() {
        return String::new();
    }
    // Only an object has the fields; any other JSON falls through to the raw text.
    if let Ok(json @ Value::Object(_)) = serde_json::from_str::<Value>(text) {
        let candidates = [
            json.get("error").and_then(|e| e.get("message")),
            json.get("error"),
            json.get("detail").and_then(|d| d.get("message")),
            json.get("detail"),
            json.get("message"),
            json.get("err_msg"),
        ];
        let found = candidates
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|c| !js_trim(c).is_empty());
        if let Some(found) = found {
            return js_truncate(js_trim(found), 300).to_string();
        }
    }
    js_truncate(&collapse_spaces(text), 300).to_string()
}

/// The TranscribeError for a non-2xx response.
pub fn http_error(provider: &str, status: u16, body: &str) -> TranscribeError {
    let detail = error_detail(body);
    let message = if detail.is_empty() {
        format!("{provider} responded {status}")
    } else {
        format!("{provider} responded {status}: {detail}")
    };
    let (code, user_message) = match status {
        401 | 403 => (TranscribeErrorCode::Auth, format!("{provider} rejected the API key")),
        429 => (TranscribeErrorCode::RateLimit, format!("{provider} rate limit reached")),
        _ => (TranscribeErrorCode::Http, format!("{provider} returned an error ({status})")),
    };
    TranscribeError::new(code, message).with_status(status).with_user_message(user_message)
}

/// An error with its sources, `outer: inner: root`, for the log.
pub(crate) fn describe_error(err: &(dyn Error + 'static)) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let line = cause.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = cause.source();
    }
    text
}

/// Sends the request and returns the parsed JSON body. Fails only with TranscribeError.
/// Cancelling drops the request, which closes its connection.
pub(crate) async fn request_json(
    ctx: &HttpContext<'_>,
    request: reqwest::RequestBuilder,
) -> Result<Value, TranscribeError> {
    if ctx.cancel.is_cancelled() {
        return Err(TranscribeError::aborted());
    }
    let exchange = async {
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        Ok::<_, reqwest::Error>((status, body))
    };
    // The caller's cancel wins over everything, as the TS checks its signal first.
    let (status, body) = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => return Err(TranscribeError::aborted()),
        result = exchange => match result {
            Ok(exchanged) => exchanged,
            Err(err) => {
                return Err(TranscribeError::new(
                    TranscribeErrorCode::Network,
                    format!("Request to {} failed: {}", ctx.provider, describe_error(&err)),
                )
                .with_user_message(format!("Couldn't reach {}", ctx.provider))
                .with_cause(err));
            }
        },
        () = tokio::time::sleep(ctx.timeout) => {
            return Err(TranscribeError::new(
                TranscribeErrorCode::Network,
                format!("{} did not respond in time", ctx.provider),
            )
            .with_user_message(format!("{} timed out", ctx.provider)));
        }
    };
    if !status.is_success() {
        return Err(http_error(ctx.provider, status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|err| {
        TranscribeError::new(
            TranscribeErrorCode::BadResponse,
            format!("{} sent a response that is not JSON", ctx.provider),
        )
        .with_status(status.as_u16())
        .with_cause(err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    mod http_error {
        use super::*;

        #[test]
        fn maps_status_to_code() {
            let cases = [
                (401, TranscribeErrorCode::Auth),
                (403, TranscribeErrorCode::Auth),
                (429, TranscribeErrorCode::RateLimit),
                (400, TranscribeErrorCode::Http),
                (404, TranscribeErrorCode::Http),
                (500, TranscribeErrorCode::Http),
                (503, TranscribeErrorCode::Http),
            ];
            for (status, code) in cases {
                let err = http_error("Groq", status, r#"{"error":{"message":"nope"}}"#);
                assert_eq!(err.code, code, "{status}");
                assert_eq!(err.status, Some(status));
                assert_eq!(err.message, format!("Groq responded {status}: nope"));
                assert!(err.user_message().contains("Groq"), "{status}");
            }
        }
    }

    mod error_detail {
        use super::*;

        #[test]
        fn reads_each_body_shape() {
            let cases = [
                (
                    "OpenAI and Groq",
                    r#"{"error":{"message":"Invalid API Key","type":"invalid_request_error"}}"#,
                    "Invalid API Key",
                ),
                ("a string error", r#"{"error":"bad model"}"#, "bad model"),
                (
                    "ElevenLabs",
                    r#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#,
                    "Invalid API key",
                ),
                ("FastAPI servers", r#"{"detail":"Not Found"}"#, "Not Found"),
                ("Mistral", r#"{"message":"Unauthorized","request_id":"abc"}"#, "Unauthorized"),
                (
                    "Deepgram",
                    r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials."}"#,
                    "Invalid credentials.",
                ),
                ("plain text", "upstream\n  timeout", "upstream timeout"),
                ("nothing", "", ""),
                ("JSON that is not an object", "null", "null"),
            ];
            for (name, body, expected) in cases {
                assert_eq!(error_detail(body), expected, "{name}");
            }
        }

        #[test]
        fn caps_a_long_body() {
            assert_eq!(error_detail(&"x".repeat(5000)).len(), 300);
        }
    }
}
