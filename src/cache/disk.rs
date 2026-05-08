//! File-backed implementation of [`ClassificationCache`].
//!
//! Layout:
//!   `<root>/v<schema>/<model>/<sha-prefix>/<sha>.json`

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::audit::now_unix_ms;
use crate::cache::{
    CACHE_SCHEMA_VERSION, CacheEntry, CacheError, CacheStats, ClassificationCache,
    sanitize_model_id,
};
use crate::models::Classification;

#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
}

impl DiskCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn entry_path(&self, sha: &str, model: &str) -> PathBuf {
        let model_dir = sanitize_model_id(model);
        let prefix = if sha.len() >= 4 { &sha[..4] } else { sha };
        self.root
            .join(format!("v{CACHE_SCHEMA_VERSION}"))
            .join(model_dir)
            .join(prefix)
            .join(format!("{sha}.json"))
    }

    fn schema_root(&self) -> PathBuf {
        self.root.join(format!("v{CACHE_SCHEMA_VERSION}"))
    }

    fn ensure_parent(path: &Path) -> Result<(), CacheError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CacheError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }
        Ok(())
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

impl ClassificationCache for DiskCache {
    fn get(&self, sha: &str, model: &str) -> Option<Classification> {
        let path = self.entry_path(sha, model);
        let entry: CacheEntry = read_json(&path)?;
        if entry.schema != CACHE_SCHEMA_VERSION {
            return None;
        }
        if entry.sha != sha || entry.model != model {
            return None;
        }
        Some(entry.classification)
    }

    fn put(&self, sha: &str, model: &str, classification: &Classification) {
        let entry = CacheEntry {
            schema: CACHE_SCHEMA_VERSION,
            sha: sha.to_string(),
            model: model.to_string(),
            classification: classification.clone(),
            computed_at_unix_ms: now_unix_ms(),
        };
        let path = self.entry_path(sha, model);
        if Self::ensure_parent(&path).is_err() {
            return;
        }
        let json = match serde_json::to_vec_pretty(&entry) {
            Ok(b) => b,
            Err(_) => return,
        };
        // Atomic-ish write: write to a tmp file alongside, then rename.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_err() {
            return;
        }
        let _ = std::fs::rename(&tmp, &path);
    }

    fn clear(&self) -> Result<usize, CacheError> {
        let schema_root = self.schema_root();
        if !schema_root.exists() {
            return Ok(0);
        }
        let count = count_entries(&schema_root);
        std::fs::remove_dir_all(&schema_root).map_err(|e| CacheError::Io {
            path: schema_root.display().to_string(),
            source: e,
        })?;
        Ok(count)
    }

    fn stats(&self) -> CacheStats {
        let schema_root = self.schema_root();
        if !schema_root.exists() {
            return CacheStats::default();
        }
        let (entries, bytes) = walk_stats(&schema_root);
        CacheStats { entries, bytes }
    }

    fn root(&self) -> Option<PathBuf> {
        Some(self.schema_root())
    }
}

fn count_entries(root: &Path) -> usize {
    walk_stats(root).0
}

fn walk_stats(root: &Path) -> (usize, u64) {
    let mut entries = 0usize;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let read = match std::fs::read_dir(&p) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in read.flatten() {
            let path = ent.path();
            let ft = match ent.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
                entries += 1;
                if let Ok(meta) = ent.metadata() {
                    bytes += meta.len();
                }
            }
        }
    }
    (entries, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ClassificationSource, Section};

    fn classification(source: ClassificationSource, conf: f32) -> Classification {
        Classification {
            section: Section::Features,
            summary: "x".into(),
            source,
            confidence: conf,
        }
    }

    #[test]
    fn round_trip_via_disk() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        c.put(
            "abcdef0123456789",
            "claude-sonnet",
            &classification(ClassificationSource::BatchedLlm, 0.85),
        );
        let got = c.get("abcdef0123456789", "claude-sonnet").unwrap();
        assert_eq!(got.confidence, 0.85);
    }

    #[test]
    fn miss_for_different_model() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        c.put(
            "abc",
            "model-a",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        assert!(c.get("abc", "model-b").is_none());
    }

    #[test]
    fn miss_when_schema_mismatch_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        c.put(
            "abcdef",
            "model",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        // Tamper with the file: bump its schema to 9999.
        let path = c.entry_path("abcdef", "model");
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut entry: CacheEntry = serde_json::from_str(&raw).unwrap();
        entry.schema = 9999;
        std::fs::write(&path, serde_json::to_string(&entry).unwrap()).unwrap();
        assert!(c.get("abcdef", "model").is_none());
    }

    #[test]
    fn stats_walks_entries_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        c.put(
            "a",
            "m",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        c.put(
            "b",
            "m",
            &classification(ClassificationSource::PerCommitLlm, 0.85),
        );
        let s = c.stats();
        assert_eq!(s.entries, 2);
        assert!(s.bytes > 0);
    }

    #[test]
    fn clear_removes_only_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        c.put(
            "a",
            "m",
            &classification(ClassificationSource::BatchedLlm, 0.9),
        );
        let cleared = c.clear().unwrap();
        assert_eq!(cleared, 1);
        assert_eq!(c.stats().entries, 0);
    }

    #[test]
    fn entry_path_uses_two_level_layout() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().to_path_buf());
        let p = c.entry_path("abcd1234efgh", "claude-sonnet-4-6");
        let s = p.to_string_lossy();
        assert!(s.contains("v1"), "schema marker: {s}");
        assert!(s.contains("claude-sonnet-4-6"));
        assert!(s.contains("/abcd/abcd1234efgh.json"));
    }

    #[test]
    fn missing_root_yields_empty_stats() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::new(dir.path().join("nonexistent"));
        let s = c.stats();
        assert_eq!(s.entries, 0);
    }
}
