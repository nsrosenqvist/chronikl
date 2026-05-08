//! Self-update logic for the chronikl binary.
//!
//! # Bounded Context: Self-Update
//!
//! Owns release checking, binary download, checksum verification,
//! archive extraction, and atomic replacement. Isolated from all
//! release-notes logic — only called from the `update` CLI subcommand.
//!
//! Downloads the latest release from GitHub, verifies its SHA256
//! checksum, extracts the binary from the `.tar.gz` archive, and
//! atomically replaces the currently running executable.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::constants::{self, TARGET, VERSION as CURRENT_VERSION};

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("failed to query GitHub releases: {0}")]
    Api(String),
    #[error("failed to download release asset: {0}")]
    Download(String),
    #[error("checksum verification failed: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("checksum file does not contain entry for {0}")]
    ChecksumNotFound(String),
    #[error("failed to extract archive: {0}")]
    Extract(String),
    #[error("failed to replace binary: {0}")]
    Replace(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

#[derive(Debug)]
struct ReleaseInfo {
    tag: String,
    version: String,
}

/// Run the self-update process. Returns `Ok(())` on success — whether
/// the binary was updated or already up-to-date.
pub async fn run_update(force: bool) -> Result<(), UpdateError> {
    use colored::Colorize;

    if let Some(env) = detect_container_environment() {
        eprintln!(
            "  {} {}",
            "⚠".yellow().bold(),
            format!(
                "Running inside {env}. Consider rebuilding the image instead of self-updating."
            )
            .yellow(),
        );
        eprintln!();
    }

    if crate::ci::is_ci() {
        eprintln!(
            "  {} {}",
            "⚠".yellow().bold(),
            "Running in a CI environment. Consider pinning a version in your pipeline instead."
                .yellow(),
        );
        eprintln!();
    }

    validate_platform()?;

    eprintln!("  {} {}", "▸".dimmed(), "Checking for updates...".dimmed());

    let release = fetch_latest_release().await?;

    if !force && !is_newer(&release.version) {
        eprintln!(
            "  {} Already on the latest version ({}).",
            "✔".green().bold(),
            CURRENT_VERSION.green().bold(),
        );
        return Ok(());
    }

    eprintln!(
        "  {} Updating {} → {} ...",
        "▸".dimmed(),
        CURRENT_VERSION.dimmed(),
        release.version.bold(),
    );

    let current_exe = std::env::current_exe().map_err(|e| {
        UpdateError::Replace(format!("could not determine current executable path: {e}"))
    })?;
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);

    check_write_permission(&current_exe)?;

    let asset_name = format!("chronikl-{TARGET}.tar.gz");
    let asset_url = constants::release_asset_url(&release.tag, TARGET);
    eprintln!(
        "  {} {} {}",
        "▸".dimmed(),
        "Downloading".dimmed(),
        asset_name.dimmed()
    );
    let archive_bytes = download_bytes(&asset_url).await?;

    let checksums_url = constants::release_checksums_url(&release.tag);
    eprintln!("  {} {}", "▸".dimmed(), "Verifying checksum...".dimmed());
    let checksums_text = download_text(&checksums_url).await?;
    verify_checksum(&archive_bytes, &asset_name, &checksums_text)?;

    eprintln!("  {} {}", "▸".dimmed(), "Extracting...".dimmed());
    let new_binary = extract_binary(&archive_bytes)?;

    eprintln!("  {} {}", "▸".dimmed(), "Replacing binary...".dimmed());
    atomic_replace(&current_exe, &new_binary)?;

    eprintln!(
        "  {} Updated to {} successfully.",
        "✔".green().bold(),
        release.version.green().bold(),
    );

    Ok(())
}

async fn fetch_latest_release() -> Result<ReleaseInfo, UpdateError> {
    let client = crate::http::build_client()
        .map_err(|e| UpdateError::Api(format!("failed to build HTTP client: {e}")))?;
    let resp = client
        .get(constants::GITHUB_RELEASES_LATEST_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| UpdateError::Api(format!("request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(UpdateError::Api(format!(
            "GitHub API returned {}",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| UpdateError::Api(format!("failed to parse response: {e}")))?;

    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| UpdateError::Api("missing tag_name in response".to_string()))?
        .to_string();
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    Ok(ReleaseInfo { tag, version })
}

/// Compare a remote version against [`CURRENT_VERSION`]. Returns `true`
/// when `remote` is strictly newer.
fn is_newer(remote: &str) -> bool {
    match (
        semver::Version::parse(CURRENT_VERSION),
        semver::Version::parse(remote),
    ) {
        (Ok(current), Ok(remote)) => remote > current,
        _ => remote != CURRENT_VERSION,
    }
}

async fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let client = crate::http::build_client()
        .map_err(|e| UpdateError::Download(format!("failed to build HTTP client: {e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| UpdateError::Download(format!("{url}: {e}")))?;

    if !resp.status().is_success() {
        return Err(UpdateError::Download(format!(
            "{url}: HTTP {}",
            resp.status()
        )));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| UpdateError::Download(format!("{url}: {e}")))
}

async fn download_text(url: &str) -> Result<String, UpdateError> {
    let bytes = download_bytes(url).await?;
    String::from_utf8(bytes)
        .map_err(|e| UpdateError::Download(format!("response is not valid UTF-8: {e}")))
}

/// Verify archive bytes against a `SHA256SUMS` file. Lines look like:
/// `<hex-hash>  <filename>`.
fn verify_checksum(data: &[u8], asset_name: &str, checksums_text: &str) -> Result<(), UpdateError> {
    let expected = checksums_text
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let filename = parts.next()?;
            (filename == asset_name).then(|| hash.to_string())
        })
        .ok_or_else(|| UpdateError::ChecksumNotFound(asset_name.to_string()))?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = hex::encode(hasher.finalize());

    if actual != expected {
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }
    Ok(())
}

/// Extract the `chronikl` binary from a `.tar.gz` archive in memory.
fn extract_binary(archive_bytes: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| UpdateError::Extract(format!("failed to read archive entries: {e}")))?
    {
        let mut entry =
            entry.map_err(|e| UpdateError::Extract(format!("corrupt archive entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| UpdateError::Extract(format!("invalid path in archive: {e}")))?;

        let is_binary = path.file_name().is_some_and(|name| name == "chronikl");
        if !is_binary {
            continue;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| {
            UpdateError::Extract(format!("failed to read binary from archive: {e}"))
        })?;

        if buf.is_empty() {
            return Err(UpdateError::Extract(
                "extracted binary is empty".to_string(),
            ));
        }
        return Ok(buf);
    }

    Err(UpdateError::Extract(
        "archive does not contain a 'chronikl' binary".to_string(),
    ))
}

/// Atomically replace `target_path` with `new_binary`. Writes a tmp
/// file alongside, sets executable permissions, then renames.
fn atomic_replace(target_path: &Path, new_binary: &[u8]) -> Result<(), UpdateError> {
    let parent = target_path
        .parent()
        .ok_or_else(|| UpdateError::Replace("cannot determine parent directory".to_string()))?;

    let tmp_path = parent.join(".chronikl-update.tmp");

    let mut tmp_file = fs::File::create(&tmp_path).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied(format!(
                "permission denied writing to {}. Try running with sudo.",
                parent.display()
            ))
        } else {
            UpdateError::Replace(format!("failed to create temp file: {e}"))
        }
    })?;

    tmp_file
        .write_all(new_binary)
        .map_err(|e| UpdateError::Replace(format!("failed to write temp file: {e}")))?;
    tmp_file
        .flush()
        .map_err(|e| UpdateError::Replace(format!("failed to flush temp file: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&tmp_path, perms)
            .map_err(|e| UpdateError::Replace(format!("failed to set permissions: {e}")))?;
    }

    fs::rename(&tmp_path, target_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied(format!(
                "permission denied replacing {}. Try running with sudo.",
                target_path.display()
            ))
        } else {
            UpdateError::Replace(format!("failed to replace binary: {e}"))
        }
    })
}

fn check_write_permission(exe_path: &Path) -> Result<(), UpdateError> {
    let parent = exe_path
        .parent()
        .ok_or_else(|| UpdateError::Replace("cannot determine parent directory".to_string()))?;

    let probe_path = parent.join(".chronikl-write-probe");
    match fs::File::create(&probe_path) {
        Ok(_) => {
            let _ = fs::remove_file(&probe_path);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            Err(UpdateError::PermissionDenied(format!(
                "permission denied: cannot write to {}. Try running with sudo.",
                parent.display()
            )))
        }
        Err(e) => Err(UpdateError::Replace(format!(
            "cannot write to {}: {e}",
            parent.display()
        ))),
    }
}

fn validate_platform() -> Result<(), UpdateError> {
    const SUPPORTED_TARGETS: &[&str] = &[
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ];

    if !SUPPORTED_TARGETS.contains(&TARGET) {
        return Err(UpdateError::UnsupportedPlatform(format!(
            "no pre-built binary available for '{TARGET}'. Supported targets: {}",
            SUPPORTED_TARGETS.join(", ")
        )));
    }
    Ok(())
}

/// Heuristically detect a container environment so we can warn the
/// user that self-update is the wrong tool there.
fn detect_container_environment() -> Option<&'static str> {
    if Path::new("/.dockerenv").exists() {
        return Some("Docker");
    }
    if std::env::var("container").is_ok() {
        return Some("a container");
    }
    if let Ok(cgroup) = fs::read_to_string("/proc/1/cgroup")
        && (cgroup.contains("docker") || cgroup.contains("containerd") || cgroup.contains("lxc"))
    {
        return Some("a container");
    }
    if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() {
        return Some("Kubernetes");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper that isolates version comparison from CURRENT_VERSION.
    fn version_cmp(current: &str, remote: &str) -> bool {
        match (
            semver::Version::parse(current),
            semver::Version::parse(remote),
        ) {
            (Ok(c), Ok(r)) => r > c,
            _ => remote != current,
        }
    }

    #[test]
    fn detects_higher_version() {
        assert!(version_cmp("0.1.0", "0.2.0"));
        assert!(version_cmp("0.1.0", "1.0.0"));
        assert!(version_cmp("0.1.0", "0.1.1"));
    }

    #[test]
    fn rejects_lower_or_equal() {
        assert!(!version_cmp("0.2.0", "0.1.0"));
        assert!(!version_cmp("0.1.0", "0.1.0"));
    }

    #[test]
    fn invalid_version_falls_back_to_string_inequality() {
        assert!(version_cmp("abc", "def"));
        assert!(!version_cmp("abc", "abc"));
    }

    #[test]
    fn verify_checksum_success() {
        let data = b"hello world";
        let hash = hex::encode(Sha256::digest(data));
        let checksums = format!("{hash}  chronikl-x86_64-unknown-linux-gnu.tar.gz\n");
        assert!(
            verify_checksum(data, "chronikl-x86_64-unknown-linux-gnu.tar.gz", &checksums).is_ok()
        );
    }

    #[test]
    fn verify_checksum_mismatch() {
        let data = b"hello world";
        let checksums = "0000000000000000000000000000000000000000000000000000000000000000  chronikl-x86_64-unknown-linux-gnu.tar.gz\n";
        let result = verify_checksum(data, "chronikl-x86_64-unknown-linux-gnu.tar.gz", checksums);
        assert!(matches!(result, Err(UpdateError::ChecksumMismatch { .. })));
    }

    #[test]
    fn verify_checksum_not_found() {
        let data = b"hello world";
        let checksums = "abc123  some-other-file.tar.gz\n";
        let result = verify_checksum(data, "chronikl-x86_64-unknown-linux-gnu.tar.gz", checksums);
        assert!(matches!(result, Err(UpdateError::ChecksumNotFound(_))));
    }

    #[test]
    fn verify_checksum_picks_correct_line() {
        let data = b"target data";
        let hash = hex::encode(Sha256::digest(data));
        let checksums = format!(
            "aaaa  some-other.tar.gz\n\
             {hash}  target.tar.gz\n\
             bbbb  another.tar.gz\n"
        );
        assert!(verify_checksum(data, "target.tar.gz", &checksums).is_ok());
    }

    #[test]
    fn extract_binary_empty_archive_fails() {
        assert!(extract_binary(&[]).is_err());
    }

    #[test]
    fn extract_binary_valid_archive() {
        let mut builder = tar::Builder::new(Vec::new());
        let content = b"fake-binary-content";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "chronikl", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn extract_binary_no_matching_entry() {
        let mut builder = tar::Builder::new(Vec::new());
        let content = b"other-content";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "some-other-file", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not contain"));
    }

    #[test]
    fn extract_binary_nested_path() {
        // Binary at a nested path like "chronikl-v1.0.0/chronikl" still matches.
        let mut builder = tar::Builder::new(Vec::new());
        let content = b"nested-binary";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "chronikl-v1.0.0/chronikl", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn validate_platform_does_not_panic() {
        let _ = validate_platform();
    }

    #[test]
    fn detect_container_does_not_panic() {
        let _ = detect_container_environment();
    }
}
