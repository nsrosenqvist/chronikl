//! Classification ladder.
//!
//! Tier 0: deterministic, no LLM.
//! Tier 1: batched LLM classification (no diff).
//! Tier 2: per-commit LLM classification with diff context.
//! Tier 3: agentic LLM classification with read tools (opt-in).

pub mod tier0;
pub mod tier1;
pub mod tier2;
pub mod tier3;
