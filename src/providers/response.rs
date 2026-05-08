//! LLM response parsing, error classification, and retry helpers.
//!
//! Decoupled from provider construction so these concerns can be tested
//! and reused independently. When a provider supports native structured
//! output, the response should already be valid JSON matching the
//! requested schema. This module handles the cases where it isn't:
//! markdown fences, prose preamble, JSON-with-trailing-text, etc.

use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::constants::{INITIAL_BACKOFF, MAX_BACKOFF};
use crate::providers::ProviderError;

/// Maximum length of LLM response text included in parse-error messages.
const PARSE_ERROR_PREVIEW_LEN: usize = 2000;

/// Whether a provider error is transient and worth retrying.
///
/// Parse errors are never retried — re-rolling on the same prompt
/// usually produces the same malformed output (especially truncations).
pub fn is_retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Parse(_) => false,
        _ => classify_error(err).is_some(),
    }
}

/// Classify a provider error as a short, user-friendly message. Returns
/// `Some(...)` for transient/retryable errors, `None` otherwise.
pub fn classify_error(err: &ProviderError) -> Option<&'static str> {
    match err {
        ProviderError::Api(msg) => {
            let m = msg.to_lowercase();
            if m.contains("429") || m.contains("rate limit") || m.contains("too many requests") {
                Some("Rate limited by API")
            } else if m.contains("503")
                || m.contains("service unavailable")
                || m.contains("high demand")
            {
                Some("High model load")
            } else if m.contains("529") || m.contains("overloaded") {
                Some("API overloaded")
            } else if m.contains("502") {
                Some("API gateway error")
            } else if m.contains("timeout") || m.contains("timed out") {
                Some("Request timed out")
            } else if m.contains("connection") {
                Some("Connection error")
            } else if m.contains("temporarily") || m.contains("try again") {
                Some("Temporary API error")
            } else if m.contains("malformedfunctioncall") || m.contains("malformed function call") {
                // Gemini occasionally emits Python-style pseudocode instead of a
                // proper functionCall payload, which its own API then rejects.
                Some("Malformed tool call from model")
            } else {
                None
            }
        }
        ProviderError::Parse(_) => Some("Failed to parse LLM response"),
        ProviderError::NotConfigured(_) => None,
    }
}

/// Exponential backoff for retry attempts, capped at [`MAX_BACKOFF`].
pub fn retry_backoff(attempt: u32) -> Duration {
    INITIAL_BACKOFF
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(MAX_BACKOFF)
}

/// Generic LLM-response parser tolerant of markdown fences and prose
/// wrappers around the JSON payload.
///
/// Tries, in order:
/// 1. Parse the raw response as `T`.
/// 2. Find the first `[`/`{` and matching last `]`/`}` and parse that slice.
/// 3. Strip ``` ```json … ``` ``` markdown fences and parse the inner content.
pub fn parse_with_fallbacks<T: DeserializeOwned>(response: &str) -> Result<T, ProviderError> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::Parse("LLM response was empty".to_string()));
    }

    for candidate in extract_json_candidates(trimmed) {
        if let Ok(v) = serde_json::from_str::<T>(&candidate) {
            return Ok(v);
        }
    }

    let preview = &response[..response.len().min(PARSE_ERROR_PREVIEW_LEN)];
    Err(ProviderError::Parse(format!(
        "could not parse LLM response as JSON. Response: {preview}"
    )))
}

static FENCE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    // Closing ``` must start a line so we don't match triple-backticks
    // embedded inside JSON string values.
    regex::Regex::new(r"(?s)```(?:json)?\s*\n(.*?)\n```").expect("static regex compiles")
});

fn extract_json_candidates(text: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(3);
    out.push(text.to_string());

    if let (Some(start), Some(end_obj), Some(end_arr)) =
        (text.find(['[', '{']), text.rfind('}'), text.rfind(']'))
    {
        let end = end_obj.max(end_arr);
        if start < end {
            out.push(text[start..=end].to_string());
        }
    } else if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if start < end {
            out.push(text[start..=end].to_string());
        }
    } else if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if start < end {
            out.push(text[start..=end].to_string());
        }
    }

    for cap in FENCE_RE.captures_iter(text) {
        if let Some(inner) = cap.get(1) {
            let trimmed = inner.as_str().trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Item {
        name: String,
        n: u32,
    }

    #[test]
    fn parses_clean_json_array() {
        let v: Vec<Item> = parse_with_fallbacks(r#"[{"name":"a","n":1}]"#).unwrap();
        assert_eq!(
            v,
            vec![Item {
                name: "a".into(),
                n: 1
            }]
        );
    }

    #[test]
    fn parses_markdown_fenced_json() {
        let response = r#"Here you go:
```json
[{"name":"x","n":42}]
```"#;
        let v: Vec<Item> = parse_with_fallbacks(response).unwrap();
        assert_eq!(v[0].n, 42);
    }

    #[test]
    fn parses_fenced_without_json_label() {
        let response = "```\n[{\"name\":\"x\",\"n\":1}]\n```";
        let v: Vec<Item> = parse_with_fallbacks(response).unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parses_json_embedded_in_prose() {
        let response = r#"I picked these:
[{"name":"alpha","n":7}]
That's all."#;
        let v: Vec<Item> = parse_with_fallbacks(response).unwrap();
        assert_eq!(v[0].name, "alpha");
    }

    #[test]
    fn empty_response_is_parse_error() {
        let result: Result<Vec<Item>, _> = parse_with_fallbacks("");
        assert!(matches!(result, Err(ProviderError::Parse(_))));
    }

    #[test]
    fn whitespace_only_is_parse_error() {
        let result: Result<Vec<Item>, _> = parse_with_fallbacks("   \n\n");
        assert!(matches!(result, Err(ProviderError::Parse(_))));
    }

    #[test]
    fn unparseable_text_is_parse_error() {
        let result: Result<Vec<Item>, _> = parse_with_fallbacks("just regular text");
        assert!(matches!(result, Err(ProviderError::Parse(_))));
    }

    #[test]
    fn parses_object_response() {
        #[derive(Deserialize)]
        struct Wrapper {
            items: Vec<Item>,
        }
        let response = r#"{"items": [{"name":"a","n":1}]}"#;
        let w: Wrapper = parse_with_fallbacks(response).unwrap();
        assert_eq!(w.items.len(), 1);
    }

    #[test]
    fn retryable_429() {
        let err = ProviderError::Api("HTTP 429 Too Many Requests".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_503() {
        let err = ProviderError::Api("503 Service Unavailable".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_overloaded_message() {
        let err = ProviderError::Api("anthropic: overloaded — try again later".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_malformed_function_call() {
        let err = ProviderError::Api("ResponseError: finish_reason=MalformedFunctionCall".into());
        assert!(is_retryable(&err));
        assert_eq!(classify_error(&err), Some("Malformed tool call from model"));
    }

    #[test]
    fn not_retryable_auth_error() {
        let err = ProviderError::Api("401 Unauthorized".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_parse_error() {
        let err = ProviderError::Parse("bad json".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_not_configured() {
        let err = ProviderError::NotConfigured("missing key".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn backoff_is_exponential() {
        assert_eq!(retry_backoff(0), INITIAL_BACKOFF);
        assert_eq!(retry_backoff(1), Duration::from_secs(20));
        assert_eq!(retry_backoff(2), Duration::from_secs(40));
    }

    #[test]
    fn backoff_caps_at_max() {
        assert_eq!(retry_backoff(15), MAX_BACKOFF);
    }
}
