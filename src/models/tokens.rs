//! Token-usage accounting for LLM calls.

use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

/// Token counts produced by a single LLM call.
///
/// `cached_input_tokens` is non-zero only for providers that report it
/// (Anthropic with explicit caching, OpenAI's automatic prefix caching,
/// Gemini 2.5+, DeepSeek, …). For providers that don't, it stays at 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    pub fn new(input: u64, output: u64, cached: u64) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            total_tokens: input + output,
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cached_input_tokens += rhs.cached_input_tokens;
        self.total_tokens += rhs.total_tokens;
    }
}

impl From<rig::completion::Usage> for TokenUsage {
    fn from(u: rig::completion::Usage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cached_input_tokens: u.cached_input_tokens,
            total_tokens: u.total_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_assign_sums_each_field() {
        let mut a = TokenUsage::new(10, 20, 5);
        a += TokenUsage::new(100, 50, 30);
        assert_eq!(a.input_tokens, 110);
        assert_eq!(a.output_tokens, 70);
        assert_eq!(a.cached_input_tokens, 35);
        assert_eq!(a.total_tokens, 30 + 150);
    }

    #[test]
    fn new_sets_total() {
        let u = TokenUsage::new(7, 13, 2);
        assert_eq!(u.total_tokens, 20);
    }
}
