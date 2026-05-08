//! Test-only mock implementation of [`NotesProvider`].
//!
//! Records every call to a captured log so tests can assert what the
//! orchestrator/ladder asked the provider to do, and returns canned
//! responses. Always available (not gated behind `#[cfg(test)]`) so
//! integration tests in the `tests/` directory can use it — they live
//! in a separate crate and don't see the lib's test-cfg.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::audit::{AuditSink, CallStatus, LlmCallAudit, hash_hex, now_unix_ms};
use crate::models::TokenUsage;
use crate::providers::{CompletionRequest, CompletionResponse, NotesProvider, ProviderError};

/// A captured invocation of [`MockProvider::complete`].
#[derive(Debug, Clone)]
pub struct CapturedCall {
    pub label: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MockProviderConfig {
    /// Provider name reported to the audit log. Tests can set this so
    /// the audit assertions can pin a specific value.
    pub provider_name: String,
    pub model: String,
    /// Optional audit sink to record calls into. When `None`, calls are
    /// captured only in-memory for test assertions.
    pub audit_sink: Option<AuditSink>,
}

/// Function-style mock responder. Boxed in a type alias so clippy's
/// `type_complexity` lint doesn't fire on the enum variant.
pub type ResponderFn =
    Arc<dyn Fn(&CompletionRequest<'_>) -> Result<String, ProviderError> + Send + Sync>;

/// Behaviour mode for the mock provider.
#[derive(Clone)]
pub enum MockResponse {
    /// Always return the same text.
    Static(String),
    /// Pop responses in FIFO order. Panics if the queue is empty.
    Sequence(Arc<Mutex<Vec<String>>>),
    /// Always raise an [`ProviderError::Api`].
    Error(String),
    /// Compute the response from the request.
    Function(ResponderFn),
}

impl std::fmt::Debug for MockResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(s) => f.debug_tuple("Static").field(s).finish(),
            Self::Sequence(seq) => f
                .debug_tuple("Sequence")
                .field(&seq.lock().unwrap().len())
                .finish(),
            Self::Error(s) => f.debug_tuple("Error").field(s).finish(),
            Self::Function(_) => f.write_str("Function(<fn>)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MockProvider {
    config: MockProviderConfig,
    response: MockResponse,
    captured: Arc<Mutex<Vec<CapturedCall>>>,
    /// Tokens reported by every call.
    tokens_per_call: TokenUsage,
}

impl MockProvider {
    pub fn new(config: MockProviderConfig, response: MockResponse) -> Self {
        Self {
            config,
            response,
            captured: Arc::new(Mutex::new(Vec::new())),
            tokens_per_call: TokenUsage::new(100, 50, 0),
        }
    }

    /// Convenience: a mock that always returns the given JSON string.
    pub fn returning(json: impl Into<String>) -> Self {
        Self::new(
            MockProviderConfig {
                provider_name: "mock".into(),
                model: "mock-model".into(),
                audit_sink: None,
            },
            MockResponse::Static(json.into()),
        )
    }

    /// Convenience: a mock that pops responses from a queue in order.
    pub fn returning_sequence(items: Vec<String>) -> Self {
        Self::new(
            MockProviderConfig {
                provider_name: "mock".into(),
                model: "mock-model".into(),
                audit_sink: None,
            },
            MockResponse::Sequence(Arc::new(Mutex::new(items))),
        )
    }

    /// Convenience: a mock that always errors.
    pub fn failing(message: impl Into<String>) -> Self {
        Self::new(
            MockProviderConfig {
                provider_name: "mock".into(),
                model: "mock-model".into(),
                audit_sink: None,
            },
            MockResponse::Error(message.into()),
        )
    }

    pub fn with_audit_sink(mut self, sink: AuditSink) -> Self {
        self.config.audit_sink = Some(sink);
        self
    }

    pub fn with_tokens(mut self, tokens: TokenUsage) -> Self {
        self.tokens_per_call = tokens;
        self
    }

    /// Snapshot of every captured call, in invocation order.
    pub fn captured_calls(&self) -> Vec<CapturedCall> {
        self.captured
            .lock()
            .expect("captured lock poisoned")
            .clone()
    }

    pub fn call_count(&self) -> usize {
        self.captured.lock().expect("captured lock poisoned").len()
    }
}

#[async_trait]
impl NotesProvider for MockProvider {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let started = now_unix_ms();
        let started_instant = std::time::Instant::now();

        self.captured
            .lock()
            .expect("captured lock poisoned")
            .push(CapturedCall {
                label: request.label.to_string(),
                system_prompt: request.system_prompt.to_string(),
                user_prompt: request.user_prompt.to_string(),
                max_tokens: request.max_tokens,
            });

        let result = match &self.response {
            MockResponse::Static(s) => Ok(s.clone()),
            MockResponse::Sequence(q) => {
                let mut q = q.lock().expect("sequence lock poisoned");
                if q.is_empty() {
                    Err(ProviderError::Api(
                        "MockProvider sequence exhausted".to_string(),
                    ))
                } else {
                    Ok(q.remove(0))
                }
            }
            MockResponse::Error(msg) => Err(ProviderError::Api(msg.clone())),
            MockResponse::Function(f) => f(&request),
        };

        let duration_ms = started_instant.elapsed().as_millis() as u64;

        if let Some(sink) = &self.config.audit_sink {
            let prompt_hash = hash_hex(&format!(
                "{}\n\n{}",
                request.system_prompt, request.user_prompt
            ));
            let (status, response_hash) = match &result {
                Ok(text) => (CallStatus::Success, Some(hash_hex(text))),
                Err(e) => (
                    CallStatus::Failed {
                        error: e.to_string(),
                    },
                    None,
                ),
            };
            sink.record_call(LlmCallAudit {
                label: request.label.to_string(),
                provider: self.config.provider_name.clone(),
                model: self.config.model.clone(),
                started_at_unix_ms: started,
                duration_ms,
                status,
                tokens: self.tokens_per_call,
                prompt_hash,
                response_hash,
                commit_shas: request.commit_shas.to_vec(),
                retries: 0,
                agent: None,
            });
        }

        result.map(|text| CompletionResponse {
            text,
            tokens: self.tokens_per_call,
        })
    }

    fn provider_name(&self) -> &str {
        &self.config.provider_name
    }

    fn model(&self) -> &str {
        &self.config.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>() -> CompletionRequest<'a> {
        CompletionRequest {
            system_prompt: "sys",
            user_prompt: "user",
            schema: serde_json::json!({}),
            max_tokens: 1000,
            label: "test",
            commit_shas: &[],
        }
    }

    #[tokio::test]
    async fn static_response_is_returned() {
        let m = MockProvider::returning("hello");
        let r = m.complete(req()).await.unwrap();
        assert_eq!(r.text, "hello");
        assert_eq!(m.call_count(), 1);
    }

    #[tokio::test]
    async fn sequence_pops_in_order() {
        let m = MockProvider::returning_sequence(vec!["a".into(), "b".into()]);
        let r1 = m.complete(req()).await.unwrap();
        let r2 = m.complete(req()).await.unwrap();
        assert_eq!(r1.text, "a");
        assert_eq!(r2.text, "b");
        let r3 = m.complete(req()).await;
        assert!(r3.is_err());
    }

    #[tokio::test]
    async fn error_response_propagates() {
        let m = MockProvider::failing("boom");
        let err = m.complete(req()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Api(_)));
    }

    #[tokio::test]
    async fn captured_calls_record_prompts_and_label() {
        let m = MockProvider::returning("ok");
        m.complete(req()).await.unwrap();
        let calls = m.captured_calls();
        assert_eq!(calls[0].label, "test");
        assert_eq!(calls[0].system_prompt, "sys");
        assert_eq!(calls[0].user_prompt, "user");
    }

    #[tokio::test]
    async fn audit_sink_receives_call_when_enabled() {
        let sink = AuditSink::new();
        sink.enable();
        let m = MockProvider::returning("ok").with_audit_sink(sink.clone());
        m.complete(req()).await.unwrap();
        let audit = sink.finalize(crate::models::Classified::default(), String::new());
        assert_eq!(audit.llm_calls.len(), 1);
        assert_eq!(audit.llm_calls[0].label, "test");
    }
}
