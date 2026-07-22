use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn compute_file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {} for SHA256", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = std::io::Read::read(&mut file, &mut buffer)
            .with_context(|| format!("Failed to read {} for SHA256", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn verify_or_save_sha256(
    archive_path: &Path,
    sidecar: &Path,
    label: &str,
    skip_checksum: bool,
) -> Result<()> {
    if skip_checksum {
        return Ok(());
    }
    let hash = compute_file_sha256(archive_path)?;

    if sidecar.exists() {
        let expected = std::fs::read_to_string(sidecar)
            .with_context(|| format!("Failed to read SHA256 sidecar {}", sidecar.display()))?;
        let expected = expected.trim();
        if expected.is_empty() {
            anyhow::bail!("SHA256 sidecar is empty: {}", sidecar.display());
        }
        if hash != expected {
            anyhow::bail!(
                "{} archive corrupt or tampered: expected SHA256 {}, got {}",
                label,
                expected,
                hash
            );
        }
    } else {
        if let Some(parent) = sidecar.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(sidecar, &hash)
            .with_context(|| format!("Failed to write SHA256 sidecar {}", sidecar.display()))?;
    }

    Ok(())
}
