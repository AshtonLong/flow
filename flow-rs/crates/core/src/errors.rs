//! The one error type every transcriber, local or cloud, fails with.
//! Port of `src/main/transcribe/errors.ts`.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// What went wrong, coarsely; decides the overlay line and whether the fallback runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscribeErrorCode {
    NoKey,
    Auth,
    Network,
    RateLimit,
    Http,
    Aborted,
    UnknownModel,
    BadResponse,
    NotInstalled,
    Engine,
}

impl TranscribeErrorCode {
    pub const ALL: [Self; 10] = [
        Self::NoKey,
        Self::Auth,
        Self::Network,
        Self::RateLimit,
        Self::Http,
        Self::Aborted,
        Self::UnknownModel,
        Self::BadResponse,
        Self::NotInstalled,
        Self::Engine,
    ];

    /// The code as the TS spells it, e.g. `rate-limit`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoKey => "no-key",
            Self::Auth => "auth",
            Self::Network => "network",
            Self::RateLimit => "rate-limit",
            Self::Http => "http",
            Self::Aborted => "aborted",
            Self::UnknownModel => "unknown-model",
            Self::BadResponse => "bad-response",
            Self::NotInstalled => "not-installed",
            Self::Engine => "engine",
        }
    }
}

impl fmt::Display for TranscribeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The underlying failure, shareable so the error stays `Clone`.
pub type ErrorCause = Arc<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Clone, Default)]
pub struct TranscribeErrorOptions {
    /// HTTP status, for `auth`, `rate-limit` and `http`.
    pub status: Option<u16>,
    pub cause: Option<ErrorCause>,
    /// Replaces the default overlay line for this code.
    pub user_message: Option<String>,
}

fn default_user_message(code: TranscribeErrorCode, status: Option<u16>) -> String {
    match code {
        TranscribeErrorCode::NoKey => "No API key saved for this model".into(),
        TranscribeErrorCode::Auth => "The API key was rejected".into(),
        TranscribeErrorCode::Network => "Couldn't reach the speech provider".into(),
        TranscribeErrorCode::RateLimit => "The provider is rate limiting requests".into(),
        TranscribeErrorCode::Http => match status {
            // A zero status is "no status", as it is falsy in the TS.
            Some(status) if status != 0 => format!("The provider returned an error ({status})"),
            _ => "The provider returned an error".into(),
        },
        TranscribeErrorCode::Aborted => "Cancelled".into(),
        TranscribeErrorCode::UnknownModel => "Unknown model".into(),
        TranscribeErrorCode::BadResponse => "The provider sent an unexpected response".into(),
        TranscribeErrorCode::NotInstalled => "The model is not downloaded".into(),
        TranscribeErrorCode::Engine => "The speech engine failed".into(),
    }
}

/// A failed transcription. `message` is for the log; [`user_message`](Self::user_message)
/// is one short line fit for the overlay.
#[derive(Debug, Clone)]
pub struct TranscribeError {
    pub code: TranscribeErrorCode,
    pub message: String,
    /// HTTP status, for `auth`, `rate-limit` and `http`.
    pub status: Option<u16>,
    pub cause: Option<ErrorCause>,
    user_message: Option<String>,
}

impl TranscribeError {
    pub fn new(code: TranscribeErrorCode, message: impl Into<String>) -> Self {
        Self::with_options(code, message, TranscribeErrorOptions::default())
    }

    pub fn with_options(code: TranscribeErrorCode, message: impl Into<String>, opts: TranscribeErrorOptions) -> Self {
        Self { code, message: message.into(), status: opts.status, cause: opts.cause, user_message: opts.user_message }
    }

    /// `TranscribeError('aborted', 'Cancelled')`, the error every cancel produces.
    pub fn aborted() -> Self {
        Self::new(TranscribeErrorCode::Aborted, "Cancelled")
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_cause(mut self, cause: impl Error + Send + Sync + 'static) -> Self {
        self.cause = Some(Arc::new(cause));
        self
    }

    pub fn with_user_message(mut self, user_message: impl Into<String>) -> Self {
        self.user_message = Some(user_message.into());
        self
    }

    /// One short line fit for the overlay.
    pub fn user_message(&self) -> String {
        match &self.user_message {
            Some(message) => message.clone(),
            None => default_user_message(self.code, self.status),
        }
    }

    pub fn is_aborted(&self) -> bool {
        self.code == TranscribeErrorCode::Aborted
    }
}

impl fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for TranscribeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as &(dyn Error + 'static))
    }
}

/// True for the error a cancelled or timed-out operation produces. A
/// [`TranscribeError`] counts only with the `aborted` code; an I/O error counts
/// when it was interrupted or timed out (the `AbortError` and `TimeoutError`
/// that `fetch` rejects with in the TS).
pub fn is_abort_error(err: &(dyn Error + 'static)) -> bool {
    if let Some(err) = err.downcast_ref::<TranscribeError>() {
        return err.is_aborted();
    }
    if let Some(err) = err.downcast_ref::<std::io::Error>() {
        return matches!(err.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::TimedOut);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    mod transcribe_error {
        use super::*;

        #[test]
        fn carries_code_status_cause_and_a_one_line_user_message() {
            let cause = std::io::Error::other("socket hang up");
            let err = TranscribeError::new(TranscribeErrorCode::Http, "Groq responded 500: boom")
                .with_status(500)
                .with_cause(cause);
            let as_error: &dyn Error = &err;
            assert_eq!(err.code, TranscribeErrorCode::Http);
            assert_eq!(err.code.as_str(), "http");
            assert_eq!(err.status, Some(500));
            assert_eq!(as_error.source().map(|c| c.to_string()).as_deref(), Some("socket hang up"));
            assert_eq!(err.message, "Groq responded 500: boom");
            assert_eq!(err.to_string(), "Groq responded 500: boom");
            assert_eq!(err.user_message(), "The provider returned an error (500)");
        }

        #[test]
        fn has_a_short_user_message_for_every_code() {
            for code in TranscribeErrorCode::ALL {
                let user_message = TranscribeError::new(code, "detail").user_message();
                assert!(!user_message.is_empty(), "{code}");
                assert!(user_message.chars().count() <= 60, "{code}");
                assert!(!user_message.contains('\n'), "{code}");
            }
        }

        #[test]
        fn lets_the_thrower_supply_the_user_message() {
            let err =
                TranscribeError::new(TranscribeErrorCode::Auth, "x").with_user_message("Groq rejected the API key");
            assert_eq!(err.user_message(), "Groq rejected the API key");
        }

        #[test]
        fn codes_serialise_as_the_ts_spells_them() {
            for code in TranscribeErrorCode::ALL {
                let json = serde_json::to_value(code).unwrap();
                assert_eq!(json, serde_json::Value::String(code.as_str().into()));
            }
        }
    }

    mod is_abort_error {
        use super::*;

        #[derive(Debug)]
        struct Named(&'static str);
        impl fmt::Display for Named {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0)
            }
        }
        impl Error for Named {}

        #[test]
        fn recognises_cancellations_only() {
            let cases: Vec<(&str, Box<dyn Error>, bool)> = vec![
                (
                    "an interrupted I/O error (AbortError)",
                    Box::new(std::io::Error::new(std::io::ErrorKind::Interrupted, "aborted")),
                    true,
                ),
                (
                    "a timed-out I/O error (TimeoutError)",
                    Box::new(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out")),
                    true,
                ),
                (
                    "an aborted TranscribeError",
                    Box::new(TranscribeError::new(TranscribeErrorCode::Aborted, "Cancelled")),
                    true,
                ),
                ("a network TranscribeError", Box::new(TranscribeError::new(TranscribeErrorCode::Network, "x")), false),
                ("a failed fetch", Box::new(std::io::Error::other("fetch failed")), false),
                ("an error merely named AbortError", Box::new(Named("AbortError")), false),
            ];
            for (name, err, expected) in cases {
                assert_eq!(is_abort_error(err.as_ref()), expected, "{name}");
            }
        }
    }
}
