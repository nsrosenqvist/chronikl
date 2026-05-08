//! Centralized HTTP client construction.
//!
//! # Bounded Context: HTTP Infrastructure
//!
//! Owns `reqwest::Client` construction with consistent timeout,
//! user-agent, and TLS settings. Other modules call `http::build_client()`
//! rather than building their own clients.

use crate::constants::{HTTP_CONNECT_TIMEOUT, HTTP_TIMEOUT, USER_AGENT};

/// Build a pre-configured HTTP client with sensible defaults.
///
/// All outgoing HTTP should go through this factory so that timeout,
/// user-agent, and proxy settings are centralized.
pub fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}
