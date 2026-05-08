//! SHA-keyed disk cache for LLM-derived commit classifications.
//!
//! # Bounded Context: Cache
//!
//! Stores `(sha, model) → Classification` entries on disk so re-runs
//! against the same commit range skip the expensive LLM tiers when
//! nothing has changed. Tier 0 results are cheap to recompute, so we
//! only cache results derived from Tier 1 / Tier 2 / Tier 3.
//!
//! Layout under `<cache_root>/v<SCHEMA>/<sanitized-model>/<sha-prefix>/<sha>.json`:
//!
//! - `<cache_root>` defaults to `dirs::cache_dir()/chronikl/classifications`.
//!   The user can override via `CHRONIKL_CACHE_DIR`.
//! - `v<SCHEMA>` lets us invalidate the entire cache on a major change
//!   to prompts/behaviour by bumping a constant.
//! - `<sanitized-model>` keeps cache entries from one model from
//!   leaking into another.
//! - The two-level SHA layout (`abcd/abcd1234…`) keeps any single
//!   directory bounded.
//!
//! Cache invariant: a stored entry is the result the ladder *was about
//! to commit* for a commit. On miss the ladder runs as if there were no
//! cache. On hit the entry is loaded into the `Classified` *before* the
//! ladder runs, and confidence-threshold logic decides whether to
//! escalate further. So a cached low-confidence Tier 1 result will be
//! re-run by Tier 2, and the new result overwrites the old.

pub mod disk;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::{Classification, ClassificationSource, Classified};

/// Cache schema version. Bump when prompts or the on-disk layout change
/// in ways that should invalidate everything.
pub const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid cache entry at {path}: {message}")]
    Invalid { path: String, message: String },
}

/// One cached entry: the classification, plus enough context for
/// debugging stale or rotted caches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub schema: u32,
    pub sha: String,
    pub model: String,
    pub classification: Classification,
    pub computed_at_unix_ms: u64,
}

/// Cache abstraction. The disk impl is the production path; tests use
/// `MemoryCache` (defined here) and the no-op impl for `--no-cache`.
pub trait ClassificationCache: Send + Sync {
    fn get(&self, sha: &str, model: &str) -> Option<Classification>;
    fn put(&self, sha: &str, model: &str, classification: &Classification);

    /// Apply cached classifications to `classified` in place. Replaces
    /// any entry whose `sha` we have a hit for. Returns the number of
    /// entries updated.
    fn populate(&self, classified: &mut Classified, model: &str) -> usize {
        let mut hits = 0usize;
        for entry in classified.0.iter_mut() {
            if let Some(cached) = self.get(&entry.commit.sha, model) {
                entry.classification = cached;
                hits += 1;
            }
        }
        hits
    }

    /// Persist any LLM-derived classifications. Tier 0 results
    /// (`Conventional`, `FilesHeuristic`, `Default`) are deterministic
    /// — they recompute instantly, so we don't bother caching them.
    fn persist_llm_results(&self, classified: &Classified, model: &str) -> usize {
        let mut written = 0usize;
        for entry in classified.iter() {
            if is_llm_derived(&entry.classification.source) {
                self.put(&entry.commit.sha, model, &entry.classification);
                written += 1;
            }
        }
        written
    }

    /// Drop every entry. Returns the count removed.
    fn clear(&self) -> Result<usize, CacheError>;

    /// Snapshot of cache state for the `cache stats` subcommand.
    fn stats(&self) -> CacheStats;

    /// Path to the cache root, for the `cache path` subcommand.
    fn root(&self) -> Option<PathBuf>;
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
}

fn is_llm_derived(source: &ClassificationSource) -> bool {
    matches!(
        source,
        ClassificationSource::BatchedLlm
            | ClassificationSource::PerCommitLlm
            | ClassificationSource::Agentic
    )
}

/// Resolve the default cache root from env / dirs. The order:
///   1. `CHRONIKL_CACHE_DIR` env var (full path).
///   2. `dirs::cache_dir()/chronikl/classifications`.
///   3. `<repo_root>/.chronikl/cache/classifications` as a last resort.
pub fn default_cache_root(repo_root: &std::path::Path) -> PathBuf {
    if let Ok(env_dir) = std::env::var("CHRONIKL_CACHE_DIR") {
        return PathBuf::from(env_dir);
    }
    if let Some(d) = dirs::cache_dir() {
        return d.join("chronikl").join("classifications");
    }
    repo_root
        .join(".chronikl")
        .join("cache")
        .join("classifications")
}

/// Sanitize a model id for use as a directory name. Replaces any char
/// that isn't `[a-zA-Z0-9_.-]` with `_`. Empty/edge-case → `unknown`.
pub fn sanitize_model_id(model: &str) -> String {
    if model.trim().is_empty() {
        return "unknown".into();
    }
    let mut out = String::with_capacity(model.len());
    for ch in model.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

/// In-memory cache for tests. Thread-safe.
#[derive(Debug, Clone, Default)]
pub struct MemoryCache {
    inner: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, CacheEntry>>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(sha: &str, model: &str) -> String {
        format!("{model}:{sha}")
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ClassificationCache for MemoryCache {
    fn get(&self, sha: &str, model: &str) -> Option<Classification> {
        let map = self.inner.lock().unwrap();
        map.get(&Self::key(sha, model))
            .map(|e| e.classification.clone())
    }

    fn put(&self, sha: &str, model: &str, classification: &Classification) {
        let mut map = self.inner.lock().unwrap();
        map.insert(
            Self::key(sha, model),
            CacheEntry {
                schema: CACHE_SCHEMA_VERSION,
                sha: sha.to_string(),
                model: model.to_string(),
                classification: classification.clone(),
                computed_at_unix_ms: 0,
            },
        );
    }

    fn clear(&self) -> Result<usize, CacheError> {
        let mut map = self.inner.lock().unwrap();
        let n = map.len();
        map.clear();
        Ok(n)
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.len(),
            bytes: 0,
        }
    }

    fn root(&self) -> Option<PathBuf> {
        None
    }
}

/// No-op cache used when `--no-cache` is set.
#[derive(Debug, Clone, Default)]
pub struct NullCache;

impl ClassificationCache for NullCache {
    fn get(&self, _sha: &str, _model: &str) -> Option<Classification> {
        None
    }
    fn put(&self, _sha: &str, _model: &str, _classification: &Classification) {}
    fn clear(&self) -> Result<usize, CacheError> {
        Ok(0)
    }
    fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
    fn root(&self) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ClassifiedCommit, Commit, Section};

    fn classification(source: ClassificationSource, conf: f32) -> Classification {
        Classification {
            section: Section::Features,
            summary: "x".into(),
            source,
            confidence: conf,
        }
    }

    fn commit_entry(sha: &str, source: ClassificationSource, conf: f32) -> ClassifiedCommit {
        ClassifiedCommit {
            commit: Commit {
                sha: sha.into(),
                short_sha: sha[..7.min(sha.len())].into(),
                author_name: "ada".into(),
                author_email: "ada@x".into(),
                author_date: "2026-01-01T00:00:00+00:00".into(),
                parents: vec!["p".into()],
                subject: "x".into(),
                body: String::new(),
                files: vec![],
                pr_id: None,
                conventional: None,
                breaking: false,
            },
            pr: None,
            classification: classification(source, conf),
        }
    }

    #[test]
    fn sanitize_model_id_replaces_specials() {
        assert_eq!(sanitize_model_id("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(
            sanitize_model_id("provider/model:tag"),
            "provider_model_tag"
        );
        assert_eq!(sanitize_model_id(""), "unknown");
    }

    #[test]
    fn null_cache_never_hits() {
        let c = NullCache;
        c.put(
            "abc",
            "model",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        assert!(c.get("abc", "model").is_none());
    }

    #[test]
    fn memory_cache_round_trip() {
        let c = MemoryCache::new();
        c.put(
            "abc",
            "model",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        let got = c.get("abc", "model").unwrap();
        assert_eq!(got.summary, "x");
        assert_eq!(got.confidence, 0.9);
    }

    #[test]
    fn memory_cache_isolates_models() {
        let c = MemoryCache::new();
        c.put(
            "abc",
            "model-a",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        assert!(c.get("abc", "model-b").is_none());
    }

    #[test]
    fn populate_replaces_classification_for_hit_only() {
        let c = MemoryCache::new();
        c.put(
            "abc",
            "model",
            &classification(ClassificationSource::PerCommitLlm, 0.8),
        );
        let mut classified = Classified(vec![
            commit_entry("abc", ClassificationSource::Default, 0.5),
            commit_entry("def", ClassificationSource::Default, 0.5),
        ]);
        let hits = c.populate(&mut classified, "model");
        assert_eq!(hits, 1);
        assert!(matches!(
            classified.0[0].classification.source,
            ClassificationSource::PerCommitLlm
        ));
        assert!(matches!(
            classified.0[1].classification.source,
            ClassificationSource::Default
        ));
    }

    #[test]
    fn persist_only_writes_llm_derived() {
        let c = MemoryCache::new();
        let classified = Classified(vec![
            commit_entry("a", ClassificationSource::Conventional, 1.0),
            commit_entry("b", ClassificationSource::Default, 0.5),
            commit_entry(
                "c",
                ClassificationSource::FilesHeuristic {
                    reason: "lockfile".into(),
                },
                1.0,
            ),
            commit_entry("d", ClassificationSource::BatchedLlm, 0.9),
            commit_entry("e", ClassificationSource::PerCommitLlm, 0.85),
            commit_entry("f", ClassificationSource::Agentic, 0.95),
        ]);
        let written = c.persist_llm_results(&classified, "model");
        assert_eq!(written, 3);
        assert!(c.get("d", "model").is_some());
        assert!(c.get("e", "model").is_some());
        assert!(c.get("f", "model").is_some());
        // Deterministic sources should NOT be cached.
        assert!(c.get("a", "model").is_none());
        assert!(c.get("b", "model").is_none());
        assert!(c.get("c", "model").is_none());
    }

    #[test]
    fn clear_empties_the_cache() {
        let c = MemoryCache::new();
        c.put(
            "a",
            "m",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        c.put(
            "b",
            "m",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        let cleared = c.clear().unwrap();
        assert_eq!(cleared, 2);
        assert!(c.is_empty());
    }
}
