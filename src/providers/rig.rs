//! rig-core integration for chronikl.
//!
//! Wraps rig-core's multi-provider client matrix behind the
//! [`NotesProvider`] trait. The provider name in config selects which
//! rig-core provider client to construct; both completion (Tier 1/2 +
//! prose) and agentic (Tier 3) calls go through a single
//! [`Self::call`] dispatcher so the per-provider build logic isn't
//! duplicated.

use std::sync::Arc;

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, message::AssistantContent};
use rig::providers;
use rig::tool::ToolDyn;

use crate::audit::{AuditSink, CallStatus, LlmCallAudit, hash_hex, now_unix_ms};
use crate::config::ProviderConfig;
use crate::constants::{ENV_API_KEY, MAX_RETRIES};
use crate::models::TokenUsage;
use crate::providers::agent_loop::{LoopConfig, LoopOutcome, run_agent_loop};
use crate::providers::response::{is_retryable, retry_backoff};
use crate::providers::{
    CompletionRequest, CompletionResponse, NotesProvider, ProviderError, ProviderName,
};

/// Optional agent-loop configuration passed to [`RigProvider::call`].
/// When `Some`, the matrix dispatches via [`run_agent_loop`] with the
/// supplied tools instead of issuing a one-shot completion.
struct AgenticArgs<'a> {
    tools: Vec<Arc<dyn ToolDyn>>,
    max_turns: u32,
    terminal_tool: Option<&'a str>,
    self_repair: Option<&'a str>,
}

/// Per-call inputs for [`RigProvider::call`].
struct CallArgs<'a> {
    request: &'a CompletionRequest<'a>,
    /// `Some` for Tier 3 agentic calls; `None` for everything else.
    agentic: Option<AgenticArgs<'a>>,
}

/// Output of a single dispatch — either a one-shot completion or the
/// full agent-loop outcome (which the Tier 3 caller still needs to
/// inspect for diagnostics + the captured terminal-tool payload).
enum CallResult {
    Completion(CompletionResponse),
    Agent(LoopOutcome),
}

/// rig-core based notes provider. Constructs client + model handles per
/// call so each provider can apply its own pre-flight tweaks (e.g.
/// Anthropic's `with_automatic_caching()`) without bleeding provider
/// types into the trait.
#[derive(Debug)]
pub struct RigProvider {
    config: ProviderConfig,
    name: ProviderName,
    model_id: String,
    audit_sink: Option<AuditSink>,
}

impl RigProvider {
    /// Construct a `RigProvider` from a resolved [`ProviderConfig`].
    ///
    /// Errors if the provider requires an API key and none was resolved.
    /// (Ollama runs locally and is exempt.)
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let name = parse_provider_name(&config)?;
        let model_id = config
            .model
            .clone()
            .ok_or_else(|| ProviderError::NotConfigured("missing model".to_string()))?;
        if config.api_key.is_none() && name != ProviderName::Ollama {
            return Err(ProviderError::NotConfigured(format!(
                "no API key resolved for provider '{name}'. Set {ENV_API_KEY} or the \
                 provider-specific env var."
            )));
        }
        Ok(Self {
            config,
            name,
            model_id,
            audit_sink: None,
        })
    }

    pub fn with_audit_sink(mut self, sink: AuditSink) -> Self {
        self.audit_sink = Some(sink);
        self
    }

    fn api_key(&self) -> Result<&str, ProviderError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| ProviderError::NotConfigured("missing API key".to_string()))
    }

    fn require_base_url(&self) -> Result<&str, ProviderError> {
        self.config.base_url.as_deref().ok_or_else(|| {
            ProviderError::NotConfigured(format!("provider '{}' requires base_url", self.name))
        })
    }

    fn openai_client(
        &self,
        api_key: &str,
    ) -> Result<providers::openai::CompletionsClient, ProviderError> {
        let mut builder = providers::openai::CompletionsClient::builder().api_key(api_key);
        if let Some(ref base_url) = self.config.base_url {
            builder = builder.base_url(base_url);
        }
        build_client(builder.build(), "OpenAI")
    }

    /// Issue a single completion call against the configured provider,
    /// with retry on transient errors and audit recording.
    async fn dispatch(
        &self,
        request: &CompletionRequest<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        let mut attempt: u32 = 0;
        loop {
            let started_at = now_unix_ms();
            let started_instant = std::time::Instant::now();
            let outcome = self
                .call(CallArgs {
                    request,
                    agentic: None,
                })
                .await
                .and_then(|r| match r {
                    CallResult::Completion(c) => Ok(c),
                    CallResult::Agent(_) => Err(ProviderError::Api(
                        "internal: agent result returned for non-agentic dispatch".into(),
                    )),
                });
            let duration_ms = started_instant.elapsed().as_millis() as u64;

            self.record_audit(request, &outcome, started_at, duration_ms, attempt as usize);

            match outcome {
                Ok(response) => return Ok(response),
                Err(err) if is_retryable(&err) && attempt < MAX_RETRIES => {
                    let backoff = retry_backoff(attempt);
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn record_audit(
        &self,
        request: &CompletionRequest<'_>,
        outcome: &Result<CompletionResponse, ProviderError>,
        started_at: u64,
        duration_ms: u64,
        retries: usize,
    ) {
        let Some(sink) = &self.audit_sink else {
            return;
        };
        let prompt_hash = hash_hex(&format!(
            "{}\n\n{}",
            request.system_prompt, request.user_prompt
        ));
        let (status, tokens, response_hash) = match outcome {
            Ok(r) => (CallStatus::Success, r.tokens, Some(hash_hex(&r.text))),
            Err(e) => (
                CallStatus::Failed {
                    error: e.to_string(),
                },
                TokenUsage::default(),
                None,
            ),
        };
        sink.record_call(LlmCallAudit {
            label: request.label.to_string(),
            provider: self.name.to_string(),
            model: self.model_id.clone(),
            started_at_unix_ms: started_at,
            duration_ms,
            status,
            tokens,
            prompt_hash,
            response_hash,
            commit_shas: request.commit_shas.to_vec(),
            retries,
            agent: None,
        });
    }

    /// Single dispatch matrix. Builds the per-provider client + model
    /// handle (applying provider-specific tweaks like Anthropic's
    /// `with_automatic_caching()`), then forwards to
    /// [`dispatch_call`] which handles both completion and agentic
    /// modes.
    ///
    /// **Caching note.** Most providers do prompt caching implicitly
    /// server-side as long as repeat calls share an identical prefix
    /// (OpenAI, Azure, Gemini 2.5, DeepSeek, …) — no opt-in needed and
    /// the savings show up in `usage.cached_input_tokens` automatically.
    /// Anthropic is the exception: caching is opt-in via
    /// [`with_automatic_caching`] on the model handle.
    async fn call(&self, args: CallArgs<'_>) -> Result<CallResult, ProviderError> {
        let api_key = if self.name == ProviderName::Ollama {
            self.config.api_key.as_deref().unwrap_or("")
        } else {
            self.api_key()?
        };

        match self.name {
            ProviderName::Anthropic => {
                let client: providers::anthropic::Client = build_client(
                    providers::anthropic::Client::builder()
                        .api_key(api_key)
                        .build(),
                    "Anthropic",
                )?;
                let model = client
                    .completion_model(&self.model_id)
                    .with_automatic_caching();
                dispatch_call(model, args).await
            }
            ProviderName::OpenAI => {
                let client = self.openai_client(api_key)?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Cohere => {
                let client: providers::cohere::Client =
                    build_client(providers::cohere::Client::new(api_key), "Cohere")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Gemini => {
                let client: providers::gemini::Client =
                    build_client(providers::gemini::Client::new(api_key), "Gemini")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Perplexity => {
                let client: providers::perplexity::Client =
                    build_client(providers::perplexity::Client::new(api_key), "Perplexity")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::DeepSeek => {
                let client: providers::deepseek::Client =
                    build_client(providers::deepseek::Client::new(api_key), "DeepSeek")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Xai => {
                let client: providers::xai::Client =
                    build_client(providers::xai::Client::new(api_key), "xAI")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Groq => {
                let client: providers::groq::Client =
                    build_client(providers::groq::Client::new(api_key), "Groq")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::HuggingFace => {
                let client: providers::huggingface::Client =
                    build_client(providers::huggingface::Client::new(api_key), "HuggingFace")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Hyperbolic => {
                let client: providers::hyperbolic::Client =
                    build_client(providers::hyperbolic::Client::new(api_key), "Hyperbolic")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Mira => {
                let client: providers::mira::Client =
                    build_client(providers::mira::Client::new(api_key), "Mira")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Mistral => {
                let client: providers::mistral::Client =
                    build_client(providers::mistral::Client::new(api_key), "Mistral")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Moonshot => {
                let client: providers::moonshot::Client =
                    build_client(providers::moonshot::Client::new(api_key), "Moonshot")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Ollama => {
                let mut builder =
                    providers::ollama::Client::builder().api_key(rig::client::Nothing);
                if let Some(ref base_url) = self.config.base_url {
                    builder = builder.base_url(base_url);
                }
                let client: providers::ollama::Client = build_client(builder.build(), "Ollama")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::OpenRouter => {
                let client: providers::openrouter::Client =
                    build_client(providers::openrouter::Client::new(api_key), "OpenRouter")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Together => {
                let client: providers::together::Client =
                    build_client(providers::together::Client::new(api_key), "Together")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Azure => {
                let base_url = self.require_base_url()?.to_string();
                let client: providers::azure::Client = build_client(
                    providers::azure::Client::builder()
                        .api_key(providers::azure::AzureOpenAIAuth::ApiKey(
                            api_key.to_string(),
                        ))
                        .azure_endpoint(base_url)
                        .build(),
                    "Azure",
                )?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::Galadriel => {
                let client: providers::galadriel::Client =
                    build_client(providers::galadriel::Client::new(api_key), "Galadriel")?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
            ProviderName::OpenAICompatible => {
                let base_url = self.require_base_url()?.to_string();
                let client: providers::openai::CompletionsClient = build_client(
                    providers::openai::CompletionsClient::builder()
                        .api_key(api_key)
                        .base_url(base_url)
                        .build(),
                    "OpenAI-compatible",
                )?;
                dispatch_call(client.completion_model(&self.model_id), args).await
            }
        }
    }

    /// Run an agent loop against the configured provider. Reuses
    /// [`Self::call`] with `agentic: Some(...)`.
    ///
    /// Audit recording is the caller's responsibility — they have the
    /// full `LoopOutcome` (turns, tool calls, termination reason) and
    /// know the right `commit_shas` to attach to the entry.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_agent(
        &self,
        preamble: &str,
        user_prompt: &str,
        tools: Vec<Arc<dyn ToolDyn>>,
        max_tokens: u64,
        max_turns: u32,
        terminal_tool: Option<&str>,
        self_repair_message: Option<&str>,
    ) -> Result<LoopOutcome, ProviderError> {
        // Schema isn't honoured in agentic mode (Gemini in particular
        // rejects function calling combined with a JSON response mime
        // type), so we leave it null. The agent's terminal tool
        // captures the final structured payload.
        let request = CompletionRequest {
            system_prompt: preamble,
            user_prompt,
            schema: serde_json::Value::Null,
            max_tokens,
            label: "agent",
            commit_shas: &[],
        };
        let agentic = AgenticArgs {
            tools,
            max_turns,
            terminal_tool,
            self_repair: self_repair_message,
        };
        match self
            .call(CallArgs {
                request: &request,
                agentic: Some(agentic),
            })
            .await?
        {
            CallResult::Agent(outcome) => Ok(outcome),
            CallResult::Completion(_) => Err(ProviderError::Api(
                "internal: completion result returned for agentic dispatch".into(),
            )),
        }
    }

    /// Audit-record helper for Tier 3. Records the agent loop's outcome
    /// as a single audit entry with full diagnostics (turns, tool calls,
    /// termination reason).
    #[allow(clippy::too_many_arguments)]
    pub fn record_agent_audit(
        &self,
        sink: &AuditSink,
        label: &str,
        commit_shas: Vec<String>,
        prompt_hash: String,
        outcome: &Result<LoopOutcome, ProviderError>,
        started_at: u64,
        duration_ms: u64,
    ) {
        let (status, tokens, response_hash, agent) = match outcome {
            Ok(o) => (
                CallStatus::Success,
                TokenUsage {
                    input_tokens: o.usage.input_tokens,
                    output_tokens: o.usage.output_tokens,
                    cached_input_tokens: o.usage.cached_input_tokens,
                    total_tokens: o.usage.total_tokens,
                },
                Some(hash_hex(&o.final_text)),
                Some(o.to_diagnostics()),
            ),
            Err(e) => (
                CallStatus::Failed {
                    error: e.to_string(),
                },
                TokenUsage::default(),
                None,
                None,
            ),
        };
        sink.record_call(LlmCallAudit {
            label: label.to_string(),
            provider: self.name.to_string(),
            model: self.model_id.clone(),
            started_at_unix_ms: started_at,
            duration_ms,
            status,
            tokens,
            prompt_hash,
            response_hash,
            commit_shas,
            retries: 0,
            agent,
        });
    }
}

#[async_trait]
impl NotesProvider for RigProvider {
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, ProviderError> {
        self.dispatch(&request).await
    }

    fn provider_name(&self) -> &str {
        match self.name {
            ProviderName::OpenAICompatible => "openai-compatible",
            _ => self.static_name(),
        }
    }

    fn model(&self) -> &str {
        &self.model_id
    }
}

impl RigProvider {
    fn static_name(&self) -> &'static str {
        match self.name {
            ProviderName::Anthropic => "anthropic",
            ProviderName::OpenAI => "openai",
            ProviderName::Azure => "azure",
            ProviderName::Cohere => "cohere",
            ProviderName::DeepSeek => "deepseek",
            ProviderName::Galadriel => "galadriel",
            ProviderName::Gemini => "gemini",
            ProviderName::Groq => "groq",
            ProviderName::HuggingFace => "huggingface",
            ProviderName::Hyperbolic => "hyperbolic",
            ProviderName::Mira => "mira",
            ProviderName::Mistral => "mistral",
            ProviderName::Moonshot => "moonshot",
            ProviderName::Ollama => "ollama",
            ProviderName::OpenRouter => "openrouter",
            ProviderName::Perplexity => "perplexity",
            ProviderName::Together => "together",
            ProviderName::Xai => "xai",
            ProviderName::OpenAICompatible => "openai-compatible",
        }
    }
}

fn parse_provider_name(config: &ProviderConfig) -> Result<ProviderName, ProviderError> {
    let raw = config
        .name
        .as_deref()
        .ok_or_else(|| ProviderError::NotConfigured("provider name not set".into()))?;
    raw.parse::<ProviderName>()
        .map_err(|_| ProviderError::NotConfigured(format!("unknown provider '{raw}'")))
}

fn build_client<T, E>(result: Result<T, E>, label: &str) -> Result<T, ProviderError>
where
    E: std::fmt::Display,
{
    result.map_err(|e| ProviderError::Api(format!("failed to create {label} client: {e}")))
}

/// Dispatch a single LLM call against a pre-built model handle.
///
/// In non-agentic mode the request is built with `output_schema(...)`,
/// so providers that support native structured output constrain the
/// response server-side. In agentic mode the schema is **not** set
/// (at least Gemini rejects function calling combined with a JSON
/// response mime type — "Function calling with a response mime type:
/// 'application/json' is unsupported"). The agentic prompt is expected
/// to capture its result via a terminal tool; the loop's
/// `final_text` is still returned for tier-loop callers to fall back
/// on if the model never called the terminal tool.
async fn dispatch_call<M>(model: M, args: CallArgs<'_>) -> Result<CallResult, ProviderError>
where
    M: CompletionModel + 'static,
{
    let CallArgs { request, agentic } = args;

    if let Some(agent) = agentic {
        let mut loop_cfg = LoopConfig::new(
            request.system_prompt,
            request.max_tokens,
            agent.max_turns.max(1),
        );
        if let Some(name) = agent.terminal_tool {
            loop_cfg = loop_cfg.with_terminal_tool(name);
        }
        if let Some(msg) = agent.self_repair {
            loop_cfg = loop_cfg.with_self_repair(msg);
        }
        let outcome = run_agent_loop(
            model,
            request.user_prompt.to_string(),
            agent.tools,
            loop_cfg,
        )
        .await?;
        return Ok(CallResult::Agent(outcome));
    }

    let mut req = model
        .completion_request(request.user_prompt.to_string())
        .preamble(request.system_prompt.to_string())
        .temperature(0.0)
        .max_tokens(request.max_tokens);

    // Try to attach the schema. rig-core's `output_schema` takes a
    // `schemars::Schema`, and serde_json::Value → Schema via deserialize
    // is supported by schemars. Providers that don't honour structured
    // output ignore it; the response parser handles markdown-fenced
    // fallbacks via `parse_with_fallbacks`.
    if !request.schema.is_null() {
        if let Ok(schema) = serde_json::from_value::<schemars::Schema>(request.schema.clone()) {
            req = req.output_schema(schema);
        }
    }

    let response = req
        .send()
        .await
        .map_err(|e| ProviderError::Api(format!("{} API error: {e}", request.label)))?;

    let mut text = String::new();
    for piece in response.choice.iter() {
        if let AssistantContent::Text(t) = piece {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&t.text);
        }
    }

    Ok(CallResult::Completion(CompletionResponse {
        text,
        tokens: response.usage.into(),
    }))
}

// `Send + Sync` is required because the trait is `Send + Sync` and the
// audit sink is shared via `Arc` across `tokio::spawn` boundaries.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RigProvider>();
};

// Suppress Arc-of-non-clonable concerns for the const above when
// rig-core internals add !Send/!Sync trait bounds in the future.
const _UNUSED_ARC: Option<Arc<RigProvider>> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn missing_api_key_for_anthropic_errors() {
        let config = ProviderConfig {
            name: Some("anthropic".into()),
            model: Some("claude-haiku-4-5".into()),
            api_key: None,
            base_url: None,
        };
        let err = RigProvider::new(config).unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)));
    }

    #[test]
    fn missing_model_errors() {
        let config = ProviderConfig {
            name: Some("anthropic".into()),
            model: None,
            api_key: Some("sk".into()),
            base_url: None,
        };
        let err = RigProvider::new(config).unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)));
    }

    #[test]
    fn unknown_provider_errors() {
        let config = ProviderConfig {
            name: Some("not-a-real-provider".into()),
            model: Some("x".into()),
            api_key: Some("sk".into()),
            base_url: None,
        };
        let err = RigProvider::new(config).unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)));
    }

    #[test]
    fn ollama_does_not_require_api_key() {
        let config = ProviderConfig {
            name: Some("ollama".into()),
            model: Some("llama3".into()),
            api_key: None,
            base_url: Some("http://localhost:11434".into()),
        };
        assert!(RigProvider::new(config).is_ok());
    }

    #[test]
    fn openai_compatible_requires_base_url_at_call_time() {
        let config = ProviderConfig {
            name: Some("openai-compatible".into()),
            model: Some("custom".into()),
            api_key: Some("k".into()),
            base_url: None,
        };
        let p = RigProvider::new(config).unwrap();
        assert!(p.require_base_url().is_err());
    }

    #[test]
    fn openai_compatible_with_base_url_constructs() {
        let config = ProviderConfig {
            name: Some("openai-compatible".into()),
            model: Some("custom".into()),
            api_key: Some("k".into()),
            base_url: Some("https://api.example.com".into()),
        };
        let p = RigProvider::new(config).unwrap();
        assert_eq!(p.require_base_url().unwrap(), "https://api.example.com");
    }
}
