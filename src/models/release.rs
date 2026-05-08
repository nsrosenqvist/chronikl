//! Version + release classification used by the prose pass, audit, and
//! telemetry.
//!
//! Two output types:
//!
//! - [`ReleaseKind`] — Stable / Prerelease(label) / Untagged. Drives
//!   the prerelease addendum and the Markdown header.
//! - [`VersionBump`] — Initial / Patch / Minor / Major / Unknown.
//!   Drives the bump-specific prose addendum (semver only).
//!
//! Two version schemes recognised:
//!
//! - [`VersionScheme::Semver`] — strict `semver` crate parsing.
//! - [`VersionScheme::Calver`] — `YYYY.MM[.DD]` or `YY.MM[.DD]` with an
//!   optional `-suffix` prerelease component. Year is the first
//!   component (4-digit ≥ 2000, or 2-digit 10..=99); month is 1..=12;
//!   day is 1..=99 (CalVer projects sometimes use this slot as a
//!   serial-within-month).
//!
//! Anything else returns `None` from parsing — the prose pass falls
//! back to neutral framing in that case.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionScheme {
    Semver,
    Calver,
}

impl VersionScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semver => "semver",
            Self::Calver => "calver",
        }
    }
}

/// Parsed view of a version string. `parts` is always 3-wide for
/// uniform comparison: [major, minor, patch] for semver,
/// [year, month, day] for CalVer (day defaults to 0 when absent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    pub parts: [u32; 3],
    pub scheme: VersionScheme,
    pub prerelease: Option<String>,
}

impl ParsedVersion {
    /// Parse a tag/ref string. Strips a leading `v`. Tries strict
    /// semver first, then CalVer.
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);

        // 1) Strict semver — covers `1.2.3`, `1.2.3-rc.1`, `1.2.3+build`.
        if let Ok(v) = semver::Version::parse(stripped) {
            let pre = if v.pre.is_empty() {
                None
            } else {
                Some(v.pre.to_string())
            };
            return Some(Self {
                parts: [v.major as u32, v.minor as u32, v.patch as u32],
                scheme: VersionScheme::Semver,
                prerelease: pre,
            });
        }

        // 2) CalVer — split off any `-suffix` first, then parse the base.
        let (base, prerelease) = match stripped.split_once('-') {
            Some((b, p)) => (b, Some(p.to_string())),
            None => (stripped, None),
        };
        if let Some(parts) = parse_calver(base) {
            return Some(Self {
                parts,
                scheme: VersionScheme::Calver,
                prerelease,
            });
        }

        None
    }
}

fn parse_calver(s: &str) -> Option<[u32; 3]> {
    let parts: Vec<&str> = s.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let y_str = parts[0];
    let m_str = parts[1];

    let year: u32 = y_str.parse().ok()?;
    let month: u32 = m_str.parse().ok()?;

    // Year shape: 4-digit ≥ 2000, or 2-digit (10..=99).
    let valid_year = match y_str.len() {
        4 => year >= 2000,
        2 => (10..=99).contains(&year),
        _ => false,
    };
    if !valid_year {
        return None;
    }
    if !(1..=12).contains(&month) {
        return None;
    }

    let day = if parts.len() == 3 {
        let d: u32 = parts[2].parse().ok()?;
        // CalVer projects sometimes use the third slot as a
        // serial-within-month, so we accept 1..=99 (not just 1..=31).
        if !(1..=99).contains(&d) {
            return None;
        }
        d
    } else {
        0
    };
    Some([year, month, day])
}

/// What kind of version bump the release represents.
///
/// Despite the historical name, this works for any scheme where
/// `parts` are positional (semver: major/minor/patch; CalVer:
/// year/month/day). The variant-meaning mapping is identical across
/// schemes — it's the prose addendum that's scheme-aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionBump {
    /// No `from` version available (first release).
    Initial,
    Patch,
    Minor,
    Major,
    /// Either ref isn't a recognisable version, or `to` is older or
    /// equal to `from`, or the two refs are in different schemes.
    Unknown,
}

impl VersionBump {
    /// Compare two parsed versions by their positional parts. Refuses
    /// to compare across schemes (returns [`Self::Unknown`]).
    pub fn from_parsed(from: Option<&ParsedVersion>, to: Option<&ParsedVersion>) -> Self {
        let to = match to {
            Some(v) => v,
            None => return Self::Unknown,
        };
        let from = match from {
            Some(v) => v,
            None => return Self::Initial,
        };
        if from.scheme != to.scheme {
            return Self::Unknown;
        }
        let [tm, tn, tp] = to.parts;
        let [fm, fn_, fp] = from.parts;
        if tm > fm {
            Self::Major
        } else if tm == fm && tn > fn_ {
            Self::Minor
        } else if tm == fm && tn == fn_ && tp > fp {
            Self::Patch
        } else {
            Self::Unknown
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReleaseKind {
    Stable,
    Prerelease { label: String },
    Untagged,
}

impl ReleaseKind {
    pub fn is_prerelease(&self) -> bool {
        matches!(self, Self::Prerelease { .. })
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Prerelease { label } => Some(label.as_str()),
            _ => None,
        }
    }

    /// Try to parse a tag/ref as a release kind. Returns `None` for
    /// strings that aren't recognisable as semver or CalVer.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let parsed = ParsedVersion::parse(tag)?;
        Some(match parsed.prerelease {
            Some(label) => Self::Prerelease { label },
            None => Self::Stable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ParsedVersion {
        ParsedVersion::parse(s).unwrap()
    }

    // ── ReleaseKind ────────────────────────────────────────────────

    #[test]
    fn parses_stable_semver() {
        assert_eq!(ReleaseKind::from_tag("v1.2.3"), Some(ReleaseKind::Stable));
        assert_eq!(ReleaseKind::from_tag("0.1.0"), Some(ReleaseKind::Stable));
    }

    #[test]
    fn parses_prerelease_semver() {
        let k = ReleaseKind::from_tag("v1.0.0-rc.1").unwrap();
        assert_eq!(
            k,
            ReleaseKind::Prerelease {
                label: "rc.1".into()
            }
        );
    }

    #[test]
    fn parses_stable_calver_padded() {
        // Padded CalVer fails semver but should be recognised as stable.
        let k = ReleaseKind::from_tag("v2024.05.08").unwrap();
        assert_eq!(k, ReleaseKind::Stable);
    }

    #[test]
    fn parses_calver_two_component() {
        let k = ReleaseKind::from_tag("2024.10").unwrap();
        assert_eq!(k, ReleaseKind::Stable);
    }

    #[test]
    fn parses_short_calver_year() {
        let k = ReleaseKind::from_tag("v24.05.08").unwrap();
        assert_eq!(k, ReleaseKind::Stable);
    }

    #[test]
    fn parses_calver_with_suffix_as_prerelease() {
        let k = ReleaseKind::from_tag("v2024.05.08-rc1").unwrap();
        assert_eq!(
            k,
            ReleaseKind::Prerelease {
                label: "rc1".into()
            }
        );
    }

    #[test]
    fn rejects_non_version() {
        assert_eq!(ReleaseKind::from_tag("HEAD"), None);
        assert_eq!(ReleaseKind::from_tag("not-a-tag"), None);
        assert_eq!(ReleaseKind::from_tag("nightly-2024-05-08"), None);
        assert_eq!(ReleaseKind::from_tag("v1.2"), None);
    }

    // ── VersionScheme ──────────────────────────────────────────────

    #[test]
    fn detects_scheme_semver() {
        assert_eq!(parse("v1.2.3").scheme, VersionScheme::Semver);
        assert_eq!(parse("0.1.0").scheme, VersionScheme::Semver);
        assert_eq!(parse("v2024.5.8").scheme, VersionScheme::Semver);
    }

    #[test]
    fn detects_scheme_calver() {
        // Padded forms aren't valid semver → must be recognised as CalVer.
        assert_eq!(parse("v2024.05.08").scheme, VersionScheme::Calver);
        assert_eq!(parse("2024.10").scheme, VersionScheme::Calver);
        assert_eq!(parse("v24.05.01").scheme, VersionScheme::Calver);
    }

    // ── VersionBump (semver) ───────────────────────────────────────

    #[test]
    fn bump_initial_when_no_from() {
        let to = parse("1.0.0");
        assert_eq!(
            VersionBump::from_parsed(None, Some(&to)),
            VersionBump::Initial
        );
    }

    #[test]
    fn bump_unknown_when_no_to() {
        let from = parse("1.0.0");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), None),
            VersionBump::Unknown
        );
    }

    #[test]
    fn bump_major_minor_patch_semver() {
        let from = parse("1.5.3");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&parse("2.0.0"))),
            VersionBump::Major
        );
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&parse("1.6.0"))),
            VersionBump::Minor
        );
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&parse("1.5.4"))),
            VersionBump::Patch
        );
    }

    #[test]
    fn bump_unknown_for_same_or_downgrade() {
        let from = parse("1.0.0");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&parse("1.0.0"))),
            VersionBump::Unknown
        );
        assert_eq!(
            VersionBump::from_parsed(Some(&parse("2.0.0")), Some(&parse("1.5.0"))),
            VersionBump::Unknown
        );
    }

    #[test]
    fn bump_ignores_prerelease_label() {
        let from = parse("1.5.0");
        let to = parse("2.0.0-rc.1");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&to)),
            VersionBump::Major
        );
    }

    // ── VersionBump (calver) ───────────────────────────────────────

    #[test]
    fn bump_calver_year_is_major() {
        let from = parse("2024.05.08");
        let to = parse("2025.01.15");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&to)),
            VersionBump::Major
        );
    }

    #[test]
    fn bump_calver_month_is_minor() {
        let from = parse("2024.05.08");
        let to = parse("2024.06.01");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&to)),
            VersionBump::Minor
        );
    }

    #[test]
    fn bump_calver_day_is_patch() {
        let from = parse("2024.05.08");
        let to = parse("2024.05.09");
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&to)),
            VersionBump::Patch
        );
    }

    // ── Cross-scheme refusal ───────────────────────────────────────

    #[test]
    fn cross_scheme_returns_unknown() {
        let from = parse("1.5.0"); // semver
        let to = parse("2024.05.08"); // calver
        assert_eq!(
            VersionBump::from_parsed(Some(&from), Some(&to)),
            VersionBump::Unknown
        );
    }
}
