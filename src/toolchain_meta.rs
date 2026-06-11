use crate::config;
use crate::profile::Profile;
use anyhow::Result;

/// Path to the profile sidecar file for a given toolchain version.
fn profile_sidecar_path(version: &str) -> std::path::PathBuf {
    config::envs_dir().join(version).join(".profile")
}

/// Save the installation profile to a sidecar JSON file.
pub fn save_profile(version: &str, profile: &Profile) -> Result<()> {
    let path = profile_sidecar_path(version);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(profile)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Component;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_version() -> String {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("test-ver-{n}")
    }

    #[test]
    fn test_save_profile_full_writes_json() {
        let ver = tmp_version();
        let path = profile_sidecar_path(&ver);
        // Ensure clean state
        std::fs::remove_file(&path).ok();

        save_profile(&ver, &Profile::Full).unwrap();
        assert!(path.exists(), "sidecar file should exist");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains(r#""profile""#),
            "should have 'profile' key"
        );
        assert!(content.contains(r#""full""#), "should have 'full' value");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_save_profile_custom_writes_components() {
        let ver = tmp_version();
        let path = profile_sidecar_path(&ver);
        std::fs::remove_file(&path).ok();

        let custom = Profile::Custom(HashSet::from([Component::Engine, Component::Android]));
        save_profile(&ver, &custom).unwrap();
        assert!(path.exists(), "sidecar file should exist");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains(r#""custom""#),
            "should have 'custom' value"
        );
        assert!(content.contains(r#""engine""#), "should contain 'engine'");
        assert!(content.contains(r#""android""#), "should contain 'android'");

        std::fs::remove_file(&path).ok();
    }
}
