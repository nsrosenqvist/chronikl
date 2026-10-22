//! Request-parameter compatibility across models.
//!
//! chronikl sends the same parameters to every model: a fixed
//! `temperature` and an output-token cap. Some current models reject
//! one or the other:
//!
//! - Anthropic's Claude 5.5 models: `400 invalid_request_error:
//!   "temperature is deprecated for this model."`
//! - OpenAI's reasoning models on Chat Completions: `Unsupported
//!   parameter: 'temperature' is not supported with this model.` and
//!   `Unsupported parameter: 'max_tokens' is not supported with this
//!   model. Use 'max_completion_tokens' instead.`
//!
//! [`call_adapting`] runs a call, recognizes those rejections, adjusts
//! the parameters (leave `temperature` out; send the cap as
//! `max_completion_tokens`) and retries. The adjustment is remembered in
//! [`ParamCompat`] for the rest of the run, so later calls go straight
//! through, and it's logged once per parameter.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::providers::ProviderError;

/// A request parameter a model rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// The model doesn't accept `temperature` (or not this value).
    Temperature,
    /// The model wants `max_completion_tokens` instead of `max_tokens`.
    MaxTokens,
}

impl Rejected {
    /// Name used in logs and the audit log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::MaxTokens => "max_tokens",
        }
    }
}

/// Recognize a provider error that rejects one of chronikl's request
/// parameters. Matches on the message text, since providers report
/// these as plain `400 invalid_request_error`s.
pub fn detect_rejection(err: &ProviderError) -> Option<Rejected> {
    let ProviderError::Api(msg) = err else {
        return None;
    };
    let m = msg.to_lowercase();
    let refuses = [
        "deprecated",
        "not supported",
        "unsupported",
        "does not support",
        "not available",
        "not allowed",
        "only the default",
        "unknown parameter",
        "unrecognized",
    ]
    .iter()
    .any(|k| m.contains(k));
    if m.contains("max_tokens") && (m.contains("max_completion_tokens") || refuses) {
        return Some(Rejected::MaxTokens);
    }
    if m.contains("temperature") && refuses {
        return Some(Rejected::Temperature);
    }
    None
}

/// Parameters for one call, as adjusted so far in this run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallParams {
    /// `None` leaves `temperature` out of the request.
    pub temperature: Option<f64>,
    /// Send the output cap as `max_completion_tokens` (OpenAI reasoning
    /// models) instead of `max_tokens`.
    pub max_completion_tokens: bool,
}

/// What this run has learned about the model's accepted parameters.
/// Shared by every call of a provider, including concurrent ones.
#[derive(Debug)]
pub struct ParamCompat {
    temperature: Option<f64>,
    omit_temperature: AtomicBool,
    max_completion_tokens: AtomicBool,
    /// Model id, for log lines.
    model: String,
}

impl ParamCompat {
    /// `temperature` is the configured value (`None` = never send it).
    pub fn new(model: impl Into<String>, temperature: Option<f64>) -> Self {
        Self {
            temperature,
            omit_temperature: AtomicBool::new(false),
            max_completion_tokens: AtomicBool::new(false),
            model: model.into(),
        }
    }

    /// Parameters to use for the next call.
    pub fn params(&self) -> CallParams {
        CallParams {
            temperature: if self.omit_temperature.load(Ordering::Relaxed) {
                None
            } else {
                self.temperature
            },
            max_completion_tokens: self.max_completion_tokens.load(Ordering::Relaxed),
        }
    }

    /// Adjust after `err`, given the parameters the failed call used.
    /// Returns the adjusted parameter when a retry with the new
    /// parameters is worth making; `None` when the error isn't a
    /// parameter rejection, or the call already went without it.
    pub fn adapt(&self, err: &ProviderError, used: CallParams) -> Option<Rejected> {
        let rejected = detect_rejection(err)?;
        let flag = match rejected {
            Rejected::Temperature if used.temperature.is_some() => &self.omit_temperature,
            Rejected::MaxTokens if !used.max_completion_tokens => &self.max_completion_tokens,
            _ => return None,
        };
        // Log once per parameter, even when concurrent calls hit the
        // same rejection.
        if !flag.swap(true, Ordering::Relaxed) {
            let what = match rejected {
                Rejected::Temperature => "retrying without `temperature`",
                Rejected::MaxTokens => "retrying with `max_completion_tokens`",
            };
            eprintln!(
                "warn: model `{}` rejected `{}`; {what} for the rest of this run",
                self.model,
                rejected.as_str()
            );
        }
        Some(rejected)
    }
}

/// Apply `params` and the output cap to a rig request builder.
pub fn apply_params<M: rig::completion::CompletionModel>(
    builder: rig::completion::CompletionRequestBuilder<M>,
    params: CallParams,
    max_tokens: u64,
) -> rig::completion::CompletionRequestBuilder<M> {
    let builder = builder.temperature_opt(params.temperature);
    if params.max_completion_tokens {
        builder.additional_params(serde_json::json!({ "max_completion_tokens": max_tokens }))
    } else {
        builder.max_tokens(max_tokens)
    }
}

/// Result of [`call_adapting`].
#[derive(Debug)]
pub struct Adapted<T> {
    pub result: Result<T, ProviderError>,
    /// Parameters this call had to change, in order (for the audit log).
    pub adapted: Vec<Rejected>,
}

/// Run `call` with the current parameters; when the model rejects one
/// of them, adapt and retry (at most once per parameter).
pub async fn call_adapting<T, F, Fut>(compat: &ParamCompat, mut call: F) -> Adapted<T>
where
    F: FnMut(CallParams) -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut adapted = Vec::new();
    loop {
        let params = compat.params();
        match call(params).await {
            Err(err) if adapted.len() < 2 => match compat.adapt(&err, params) {
                Some(rejected) if !adapted.contains(&rejected) => adapted.push(rejected),
                _ => {
                    return Adapted {
                        result: Err(err),
                        adapted,
                    };
                }
            },
            result => return Adapted { result, adapted },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const ANTHROPIC_TEMPERATURE: &str = "prose API error: CompletionError: ProviderError: \
        {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\
        \"message\":\"temperature is deprecated for this model.\"}}";
    const OPENAI_TEMPERATURE: &str = "tier-1 API error: HttpError: Invalid status code 400 Bad \
        Request with message: {\"error\":{\"message\":\"Unsupported value: 'temperature' does \
        not support 0 with this model. Only the default (1) value is supported.\",\
        \"type\":\"invalid_request_error\",\"param\":\"temperature\"}}";
    const OPENAI_MAX_TOKENS: &str = "prose API error: {\"error\":{\"message\":\"Unsupported \
        parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' \
        instead.\",\"type\":\"invalid_request_error\",\"param\":\"max_tokens\"}}";

    fn api(msg: &str) -> ProviderError {
        ProviderError::Api(msg.to_string())
    }

    #[test]
    fn detects_known_rejections() {
        assert_eq!(
            detect_rejection(&api(ANTHROPIC_TEMPERATURE)),
            Some(Rejected::Temperature)
        );
        assert_eq!(
            detect_rejection(&api(OPENAI_TEMPERATURE)),
            Some(Rejected::Temperature)
        );
        assert_eq!(
            detect_rejection(&api(OPENAI_MAX_TOKENS)),
            Some(Rejected::MaxTokens)
        );
    }

    #[test]
    fn ignores_unrelated_errors() {
        for msg in [
            "429 Too Many Requests",
            "401 Unauthorized",
            "max_tokens: 64000 > 32000, which is the maximum allowed",
            "temperature must be at most 1.0",
        ] {
            assert_eq!(detect_rejection(&api(msg)), None, "{msg}");
        }
        assert_eq!(
            detect_rejection(&ProviderError::Parse("temperature deprecated".into())),
            None
        );
    }

    /// A mock call that behaves like a model rejecting `temperature`
    /// with Anthropic's 400, recording the parameters of every attempt.
    fn rejecting_temperature(
        seen: &Mutex<Vec<CallParams>>,
    ) -> impl FnMut(CallParams) -> std::future::Ready<Result<String, ProviderError>> + '_ {
        move |p| {
            seen.lock().unwrap().push(p);
            std::future::ready(if p.temperature.is_some() {
                Err(api(ANTHROPIC_TEMPERATURE))
            } else {
                Ok("ok".to_string())
            })
        }
    }

    #[tokio::test]
    async fn retries_once_without_temperature_and_remembers() {
        let compat = ParamCompat::new("claude-haiku-5-5", Some(0.0));
        let seen = Mutex::new(Vec::new());

        let first = call_adapting(&compat, rejecting_temperature(&seen)).await;
        assert_eq!(first.result.unwrap(), "ok");
        assert_eq!(first.adapted, vec![Rejected::Temperature]);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[0].temperature, Some(0.0));
            assert_eq!(seen[1].temperature, None);
        }

        // The next call goes straight through without temperature.
        let second = call_adapting(&compat, rejecting_temperature(&seen)).await;
        assert_eq!(second.result.unwrap(), "ok");
        assert!(second.adapted.is_empty());
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn omitted_temperature_is_never_sent() {
        let compat = ParamCompat::new("m", None);
        let seen = Mutex::new(Vec::new());
        let out = call_adapting(&compat, rejecting_temperature(&seen)).await;
        assert!(out.result.is_ok());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn adapts_both_parameters_in_turn() {
        // An OpenAI reasoning model: rejects max_tokens first, then
        // temperature.
        let compat = ParamCompat::new("gpt-5", Some(0.0));
        let calls = Mutex::new(0);
        let out = call_adapting(&compat, |p| {
            *calls.lock().unwrap() += 1;
            std::future::ready(if !p.max_completion_tokens {
                Err(api(OPENAI_MAX_TOKENS))
            } else if p.temperature.is_some() {
                Err(api(OPENAI_TEMPERATURE))
            } else {
                Ok(())
            })
        })
        .await;
        assert!(out.result.is_ok());
        assert_eq!(
            out.adapted,
            vec![Rejected::MaxTokens, Rejected::Temperature]
        );
        assert_eq!(*calls.lock().unwrap(), 3);
        assert_eq!(
            compat.params(),
            CallParams {
                temperature: None,
                max_completion_tokens: true
            }
        );
    }

    #[tokio::test]
    async fn other_errors_pass_through_without_retry() {
        let compat = ParamCompat::new("m", Some(0.0));
        let calls = Mutex::new(0);
        let out: Adapted<()> = call_adapting(&compat, |_| {
            *calls.lock().unwrap() += 1;
            std::future::ready(Err(api("401 Unauthorized")))
        })
        .await;
        assert!(out.result.is_err());
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(compat.params().temperature, Some(0.0));
    }

    #[tokio::test]
    async fn a_rejection_that_persists_is_returned() {
        // Still rejected without temperature: give up after one retry.
        let compat = ParamCompat::new("m", Some(0.0));
        let calls = Mutex::new(0);
        let out: Adapted<()> = call_adapting(&compat, |_| {
            *calls.lock().unwrap() += 1;
            std::future::ready(Err(api(ANTHROPIC_TEMPERATURE)))
        })
        .await;
        assert!(out.result.is_err());
        assert_eq!(*calls.lock().unwrap(), 2);
    }
}
