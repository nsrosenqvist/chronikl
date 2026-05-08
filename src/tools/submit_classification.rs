//! `submit_classification` tool — terminal tool the agent calls to
//! return its final Tier 3 verdict and exit the loop.
//!
//! The verdict is captured into a thread-safe slot the orchestrator
//! drains after the loop ends. Calling this tool always succeeds (with
//! a placeholder result string); the value of the call is the *side
//! effect* of populating the captured slot.

use std::sync::{Arc, Mutex};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::ToolError;

pub const SUBMIT_CLASSIFICATION_TOOL_NAME: &str = "submit_classification";

/// What the model returns via the terminal tool. The orchestrator
/// post-processes this into a real `Classification`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Tier3Verdict {
    /// One of: breaking, features, bug-fixes, performance, refactor,
    /// documentation, tests, build, ci, chore, other.
    pub section: String,
    /// One concise line, leading with a verb. No PR id; no period.
    pub summary: String,
    /// 0.0..=1.0 — the model's confidence after exploration.
    pub confidence: f32,
    /// `true` if the agent's exploration revealed a breaking change
    /// not signalled in the message. When true, post-processing forces
    /// `section = "breaking"`.
    #[serde(default)]
    pub breaking: bool,
}

#[derive(Debug, Clone)]
pub struct SubmitClassificationTool {
    captured: Arc<Mutex<Option<Tier3Verdict>>>,
}

impl SubmitClassificationTool {
    pub fn new() -> (Self, Arc<Mutex<Option<Tier3Verdict>>>) {
        let slot = Arc::new(Mutex::new(None));
        (
            Self {
                captured: slot.clone(),
            },
            slot,
        )
    }
}

#[derive(Debug, Serialize)]
pub struct SubmitOutput {
    pub acknowledged: bool,
}

impl Tool for SubmitClassificationTool {
    const NAME: &'static str = SUBMIT_CLASSIFICATION_TOOL_NAME;

    type Error = ToolError;
    type Args = Tier3Verdict;
    type Output = SubmitOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Submit your final classification for the commit. Call this exactly \
                          once when you're confident; the loop ends after this call. \
                          Required fields: section, summary, confidence (0.0-1.0). Optional: \
                          breaking (true if you found an undeclared breaking change)."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(Tier3Verdict))
                .unwrap_or(serde_json::Value::Null),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut slot = self
            .captured
            .lock()
            .map_err(|_| ToolError::Other("submit slot poisoned".into()))?;
        *slot = Some(args);
        Ok(SubmitOutput { acknowledged: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn populates_captured_slot() {
        let (tool, slot) = SubmitClassificationTool::new();
        let out = tool
            .call(Tier3Verdict {
                section: "features".into(),
                summary: "Add x".into(),
                confidence: 0.9,
                breaking: false,
            })
            .await
            .unwrap();
        assert!(out.acknowledged);
        let captured = slot.lock().unwrap().clone().unwrap();
        assert_eq!(captured.section, "features");
        assert_eq!(captured.summary, "Add x");
    }
}
