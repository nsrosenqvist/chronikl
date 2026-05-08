//! chronikl — AI-powered release notes generator.
//!
//! See `AGENTS.md` for architecture and module-layout conventions.

pub mod audit;
pub mod cache;
pub mod ci;
pub mod cli;
pub mod config;
pub mod constants;
pub mod enrichment;
pub mod env;
pub mod git;
pub mod http;
pub mod ladder;
pub mod license;
pub mod models;
pub mod output;
pub mod progress;
pub mod prompts;
pub mod prose;
pub mod providers;
pub mod telemetry;
pub mod tools;
pub mod update;
pub mod voice;
