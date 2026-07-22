use crate::config::{PROJECT_CONFIG_FILE, ProjectConfig};
use crate::types::Version;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Find the project root by looking for .joy.json upward from cwd
fn find_project_config() -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir()?;
    let mut dir = Some(cwd.as_path());

    while let Some(current) = dir {
        let config_path = current.join(PROJECT_CONFIG_FILE);
        if config_path.exists() {
            return Ok(Some(config_path));
        }
        dir = current.parent();
    }

    Ok(None)
}

/// Read the project version if a .joy.json exists
pub fn read_project_version() -> Result<Option<Version>> {
    if let Some(config_path) = find_project_config()? {
        let content = std::fs::read_to_string(&config_path).context("Failed to read .joy.json")?;
        let config: ProjectConfig =
            serde_json::from_str(&content).context("Failed to parse .joy.json")?;
        return Ok(Some(config.version));
    }
    Ok(None)
}
