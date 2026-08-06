use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Error returned when parsing a [`Version`] or [`Channel`] fails.
#[derive(Debug, Clone)]
pub struct ParseVersionError(String);

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ParseVersionError {}

/// A validated Flutter SDK version string.
///
/// **Newtype pattern — Parse, don\'t validate**
/// Validation happens **at construction time** — once you have a `Version`,
/// it is guaranteed to be safe for filesystem operations. This implements
/// the newtype pattern with compile-time enforcement of invariants.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Version(String);

impl Version {
    /// Create a `Version` after validating the input string.
    ///
    /// Rejects:
    /// - Empty strings
    /// - Path separators (`/`, `\`) and null bytes
    /// - Parent-directory references (`..`)
    /// - Strings that don't match `^[a-zA-Z0-9.+_-]+$` — ensures the version
    ///   looks like a valid version string or channel name (e.g. `3.29.0`,
    ///   `3.29.0-1.0.pre`, `v1.12.13+hotfix.9`, `stable`). The `+` is
    ///   required for the Flutter release API's build-metadata syntax (e.g.
    ///   `3.29.3+1`); it is safe for filesystem use on all platforms.
    pub fn new(raw: impl Into<String>) -> Result<Self, ParseVersionError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(ParseVersionError("Version string must not be empty".into()));
        }
        if raw.contains('/') || raw.contains('\\') || raw.contains('\0') {
            return Err(ParseVersionError(format!(
                "Invalid version '{raw}': must not contain path separators or null bytes"
            )));
        }
        if raw.contains("..") {
            return Err(ParseVersionError(format!(
                "Invalid version '{raw}': must not contain parent directory references"
            )));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '+')
        {
            return Err(ParseVersionError(format!(
                "Invalid version '{raw}': must only contain letters, digits, dots, hyphens, underscores, and plus signs"
            )));
        }
        if raw.starts_with('.') || raw.starts_with('-') {
            return Err(ParseVersionError(format!(
                "Invalid version '{raw}': must not start with a dot or hyphen"
            )));
        }
        Ok(Version(raw))
    }

    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the inner `String`.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for Version {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Version {
    type Err = ParseVersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Version::new(s)
    }
}

impl TryFrom<String> for Version {
    type Error = ParseVersionError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Version::new(s)
    }
}

/// A Flutter release channel (e.g. `stable`, `beta`, `master`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Channel(String);

impl Channel {
    pub fn new(raw: impl Into<String>) -> Result<Self, ParseVersionError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(ParseVersionError("Channel name must not be empty".into()));
        }
        if raw.contains('/') || raw.contains('\\') || raw.contains('\0') {
            return Err(ParseVersionError(format!(
                "Invalid channel '{raw}': must not contain path separators or null bytes"
            )));
        }
        Ok(Channel(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for Channel {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Channel {
    type Err = ParseVersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Channel::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Version ----

    #[test]
    fn test_version_accepts_normal_versions() {
        assert!(Version::new("3.29.0").is_ok());
        assert!(Version::new("stable").is_ok());
        assert!(Version::new("3.0.0").is_ok());
        assert!(Version::new("3.29.0-1.0.pre").is_ok());
    }

    #[test]
    fn test_version_rejects_empty() {
        assert!(Version::new("").is_err());
    }

    #[test]
    fn test_version_rejects_path_separators() {
        assert!(Version::new("foo/bar").is_err());
        assert!(Version::new("foo\\bar").is_err());
    }

    #[test]
    fn test_version_rejects_parent_ref() {
        assert!(Version::new("foo..bar").is_err());
        assert!(Version::new("..").is_err());
    }

    #[test]
    fn test_version_rejects_null_bytes() {
        assert!(Version::new("foo\0bar").is_err());
    }

    #[test]
    fn test_version_rejects_whitespace() {
        // Whitespace (including tabs/newlines) must stay rejected — it is not
        // filesystem-safe and can hide traversal tricks.
        assert!(
            Version::new("3.29.0 ").is_err(),
            "trailing space should fail"
        );
        assert!(
            Version::new(" 3.29.0").is_err(),
            "leading space should fail"
        );
        assert!(Version::new("3.29\n0").is_err(), "newline should fail");
        assert!(Version::new("3.29\t0").is_err(), "tab should fail");
    }

    #[test]
    fn test_version_rejects_special_characters() {
        assert!(
            Version::new("foo bar").is_err(),
            "spaces should be rejected"
        );
        assert!(
            Version::new("foo!bar").is_err(),
            "exclamation should be rejected"
        );
        assert!(
            Version::new("foo@bar").is_err(),
            "at-sign should be rejected"
        );
        assert!(Version::new("foo#bar").is_err(), "hash should be rejected");
        assert!(
            Version::new("foo$bar").is_err(),
            "dollar should be rejected"
        );
        assert!(
            Version::new("foo%bar").is_err(),
            "percent should be rejected"
        );
        assert!(Version::new("foo^bar").is_err(), "caret should be rejected");
        assert!(
            Version::new("foo&bar").is_err(),
            "ampersand should be rejected"
        );
        assert!(
            Version::new("foo*bar").is_err(),
            "asterisk should be rejected"
        );
        assert!(
            Version::new("foo=bar").is_err(),
            "equals should be rejected"
        );
        assert!(Version::new("foo~bar").is_err(), "tilde should be rejected");
        assert!(Version::new("foo|bar").is_err(), "pipe should be rejected");
        assert!(
            Version::new("foo?bar").is_err(),
            "question mark should be rejected"
        );
    }

    #[test]
    fn test_version_rejects_leading_dot() {
        assert!(
            Version::new(".hidden").is_err(),
            "leading dot should be rejected"
        );
        assert!(
            Version::new(".3.29.0").is_err(),
            "leading dot + version should be rejected"
        );
    }

    #[test]
    fn test_version_rejects_leading_hyphen() {
        assert!(
            Version::new("-foo").is_err(),
            "leading hyphen should be rejected"
        );
        assert!(
            Version::new("-3.29.0").is_err(),
            "leading hyphen + version should be rejected"
        );
    }

    #[test]
    fn test_version_accepts_build_metadata_with_plus() {
        // The Flutter release API emits build metadata using `+` (e.g. hotfix
        // builds and +1 revision bumps). These must not be discarded.
        assert!(
            Version::new("v1.12.13+hotfix.9").is_ok(),
            "old Flutter release with +hotfix must be accepted"
        );
        assert!(
            Version::new("3.29.3+1").is_ok(),
            "revision bump with +1 must be accepted"
        );
        assert!(Version::new("3.29.0+2").is_ok());
        assert!(Version::new("v1.9.1+hotfix.6").is_ok());
    }

    #[test]
    fn test_version_accepts_hyphen_in_middle() {
        // Hyphens are valid in the middle for pre-release versions
        assert!(
            Version::new("3.29.0-1.0.pre").is_ok(),
            "hyphen in the middle is valid"
        );
        assert!(
            Version::new("3.0.0-dev").is_ok(),
            "pre-release suffix is valid"
        );
        assert!(
            Version::new("3.0.0-alpha.1").is_ok(),
            "alpha pre-release is valid"
        );
    }

    #[test]
    fn test_version_accepts_underscore() {
        assert!(
            Version::new("3_29_0").is_ok(),
            "underscores should be accepted"
        );
        assert!(
            Version::new("my_custom_version").is_ok(),
            "underscore in channel is valid"
        );
    }

    #[test]
    fn test_version_display() {
        let v = Version::new("3.29.0").unwrap();
        assert_eq!(v.to_string(), "3.29.0");
        assert_eq!(v.as_str(), "3.29.0");
    }

    #[test]
    fn test_version_eq_str() {
        let v = Version::new("3.29.0").unwrap();
        assert_eq!(v.as_str(), "3.29.0");
    }

    #[test]
    fn test_version_from_str() {
        let v: Version = "3.29.0".parse().unwrap();
        assert_eq!(v.as_str(), "3.29.0");
    }

    #[test]
    fn test_version_try_from_string() {
        let s = String::from("3.29.0");
        let v: Version = s.try_into().unwrap();
        assert_eq!(v.as_str(), "3.29.0");
    }

    #[test]
    fn test_version_into_inner() {
        let v = Version::new("3.29.0").unwrap();
        assert_eq!(v.into_inner(), "3.29.0");
    }

    #[test]
    fn test_version_serde_roundtrip() {
        let v = Version::new("3.29.0").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"3.29.0\"");
        let deserialized: Version = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, v);
    }

    // ---- Channel ----

    #[test]
    fn test_channel_accepts_standard_names() {
        assert!(Channel::new("stable").is_ok());
        assert!(Channel::new("beta").is_ok());
        assert!(Channel::new("master").is_ok());
    }

    #[test]
    fn test_channel_rejects_empty() {
        assert!(Channel::new("").is_err());
    }

    #[test]
    fn test_channel_rejects_path_seps() {
        assert!(Channel::new("foo/bar").is_err());
    }

    #[test]
    fn test_channel_display() {
        let c = Channel::new("stable").unwrap();
        assert_eq!(c.to_string(), "stable");
        assert_eq!(c.as_str(), "stable");
    }

    #[test]
    fn test_channel_eq_str() {
        let c = Channel::new("stable").unwrap();
        assert_eq!(c.as_str(), "stable");
    }

    #[test]
    fn test_channel_serde_roundtrip() {
        let c = Channel::new("beta").unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"beta\"");
        let deserialized: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, c);
    }
}
