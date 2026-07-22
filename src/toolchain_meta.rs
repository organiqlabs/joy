use crate::config;
use crate::profile::Profile;
use anyhow::Result;

/// Path to the profile sidecar file for a given toolchain version.
fn profile_sidecar_path(version: &str) -> Result<std::path::PathBuf> {
    Ok(config::envs_dir()?.join(version).join(".profile"))
}

/// Load the installation profile from a sidecar JSON file.
pub fn load_profile(version: &str) -> Option<Profile> {
    crate::util::validate_version(version).ok()?;
    let path = profile_sidecar_path(version).ok()?;
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save the installation profile to a sidecar JSON file.
pub fn save_profile(version: &str, profile: &Profile) -> Result<()> {
    crate::util::validate_version(version).map_err(|e| anyhow::anyhow!("{e}"))?;
    let path = profile_sidecar_path(version)?;
    crate::util::check_path_traversal(&path, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
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
    use serial_test::serial;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_version() -> String {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("test-ver-{n}")
    }

    fn temp_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_toolchain_meta_test_{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct XdgGuard(PathBuf);

    impl Drop for XdgGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("XDG_DATA_HOME");
                std::env::remove_var("XDG_CACHE_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup_xdg() -> (XdgGuard, PathBuf, PathBuf) {
        let tmp = temp_dir();
        let data_home = tmp.join("xdg").join("data");
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&data_home).unwrap();
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        (XdgGuard(tmp), data_home, cache_home)
    }

    #[test]
    #[serial]
    fn test_save_profile_full_writes_json() {
        let (_guard, _data, _cache) = setup_xdg();
        let ver = tmp_version();
        let envs = config::envs_dir().unwrap();
        let path = envs.join(&ver).join(".profile");
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
    #[serial]
    fn test_save_profile_custom_writes_components() {
        let (_guard, _data, _cache) = setup_xdg();
        let ver = tmp_version();
        let envs = config::envs_dir().unwrap();
        let path = envs.join(&ver).join(".profile");
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
