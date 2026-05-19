//! `NotesProvider` trait and LLM integration.
//!
//! # Bounded Context: LLM Providers
//!
//! Owns the `NotesProvider` trait and the rig-core multi-provider client
//! matrix. Higher tiers (ladder, prose pass) call into this layer through
//! a single `complete` primitive that returns text + token usage; they
//! own the schema and parsing of structured output.

pub mod agent_loop;
pub mod mock;
pub mod response;
pub mod rig;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use thiserror::Error;

use crate::models::TokenUsage;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("LLM API error: {0}")]
    Api(String),
    #[error("failed to parse LLM response: {0}")]
    Parse(String),
    #[error("provider not configured: {0}")]
    NotConfigured(String),
}

/// Inputs for a single completion call.
#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    pub system_prompt: &'a str,
    pub user_prompt: &'a str,
    /// JSON schema for structured output. Providers that support
    /// schema-constrained completion (Anthropic, OpenAI, Gemini, …)
    /// pass this through; others ignore it and the response parser
    /// handles markdown-fenced or prose-prefixed JSON.
    pub schema: serde_json::Value,
    pub max_tokens: u64,
    /// Short identifier for logs and audit records (e.g. "tier-1-batch").
    pub label: &'a str,
    /// Commit SHAs this call covers. Recorded into the audit entry at
    /// insert time so concurrent callers can record their audit trail
    /// without racing each other for "the most recent record".
    pub commit_shas: &'a [String],
}

/// Raw output of a completion call. Higher layers parse `text` into the
/// schema type they care about.
#[derive(Debug, Clone, Default)]
pub struct CompletionResponse {
    pub text: String,
    pub tokens: TokenUsage,
}

/// LLM provider abstraction.
#[async_trait]
pub trait NotesProvider: Send + Sync {
    /// Make a single completion call, returning the assistant text plus
    /// token usage. Implementations are responsible for retry, backoff,
    /// timeout, and audit recording.
    async fn complete(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<CompletionResponse, ProviderError>;

    /// Provider identity, used for the audit log.
    fn provider_name(&self) -> &str;

    /// Model identifier in use, used for the audit log.
    fn model(&self) -> &str;
}

/// Recognized provider names. Mirrors the rig-core provider matrix.
///
/// String form is what users put in `[provider] name = "..."` and the
/// `CHRONIKL_PROVIDER` env var.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, Default,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum ProviderName {
    #[default]
    Anthropic,
    OpenAI,
    Azure,
    Cohere,
    DeepSeek,
    Galadriel,
    Gemini,
    #[serde(rename = "github-models")]
    #[strum(serialize = "github-models")]
    GithubModels,
    Groq,
    HuggingFace,
    Hyperbolic,
    Mira,
    Mistral,
    Moonshot,
    Ollama,
    OpenRouter,
    Perplexity,
    Together,
    Xai,
    /// Generic OpenAI-API-compatible endpoint (set `base_url`).
    #[serde(rename = "openai-compatible")]
    #[strum(serialize = "openai-compatible")]
    OpenAICompatible,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_round_trip() {
        for name in [
            "anthropic",
            "openai",
            "gemini",
            "github-models",
            "ollama",
            "openai-compatible",
        ] {
            let parsed: ProviderName = name.parse().unwrap();
            assert_eq!(parsed.to_string(), name);
        }
    }

    #[test]
    fn provider_name_is_case_insensitive() {
        let p: ProviderName = "Anthropic".parse().unwrap();
        assert_eq!(p, ProviderName::Anthropic);
    }
}
