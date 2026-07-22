use crate::config;
use crate::engine_cache;
use crate::git_cache;
use crate::profile::Artifact;
use crate::profile::Profile;
use crate::releases;
use crate::toolchain_meta;
use crate::util::display_path;
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::Path;

/// Download a file with a progress bar
pub(crate) fn download_with_progress(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(url)
        .send()
        .context(format!("Failed to start download from {url}"))?;

    let total_size = resp.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    pb.set_message(format!(
        "Downloading {}",
        url.split('/').next_back().unwrap_or(url)
    ));

    let mut dest_file = BufWriter::new(File::create(dest)?);
    let mut source = resp.take(total_size.max(1));

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 8192];
    loop {
        let n = std::io::Read::read(&mut source, &mut buffer)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut dest_file, &buffer[..n])?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message(format!(
        "Downloaded {}",
        url.split('/').next_back().unwrap_or(url)
    ));
    Ok(())
}

/// Extract a .tar.xz archive
fn extract_tar_xz(archive: &Path, dest: &Path) -> Result<()> {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Extracting Flutter SDK...");

    let file = File::open(archive)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;

    pb.finish_with_message("Extracted Flutter SDK");
    Ok(())
}

/// Extract a .zip archive
fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Extracting Flutter SDK...");

    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    zip.extract(dest)?;

    pb.finish_with_message("Extracted Flutter SDK");
    Ok(())
}

/// Determine the extraction type from the archive URL or path
pub(crate) fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.to_string_lossy();
    if name.ends_with(".tar.xz") {
        extract_tar_xz(archive, dest)
    } else if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else {
        anyhow::bail!("Unsupported archive format: {name}")
    }
}

/// Verify a file's SHA256 checksum against the expected hex string.
/// Returns an error if the file doesn't exist or the checksum doesn't match.
pub(crate) fn verify_sha256(path: &Path, expected_hex: &str) -> Result<()> {
    let mut file = File::open(path).with_context(|| {
        format!(
            "Failed to open {} for checksum verification",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read {} for checksum", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let actual_hex = hex::encode(hasher.finalize());
    if actual_hex != expected_hex {
        anyhow::bail!("Expected SHA256 {}, but got {}", expected_hex, actual_hex);
    }
    Ok(())
}

/// Install a specific Flutter version with a given profile
pub fn install_version(
    version: &str,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    crate::util::validate_version(version).map_err(|e| anyhow::anyhow!("{}", e))?;
    let env_dir = config::envs_dir()?.join(version);
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Check if already installed
    if env_dir.join("bin").join("flutter").exists()
        || env_dir.join("bin").join("flutter.bat").exists()
    {
        if !force {
            println!("Version {version} is already installed. Use --force to reinstall.");
            return Ok(());
        }
        println!("Reinstalling {version}...");
        std::fs::remove_dir_all(&env_dir)?;
    }

    // Find the release info
    let release = releases::find_release(version)?;
    let download_url = &release.archive_url;

    println!("Installing Flutter {version} ({})", release.channel);

    // Warn when profile expects a smaller download but archive path always gets the full tarball
    if !profile.includes_engine() {
        println!(
            "Profile doesn't include engine, but the full release archive (~1.44 GiB) \
            will still be downloaded."
        );
        println!(
            "   Tip: Use `joy toolchain install {version} --git --profile minimal` \
            to shallow-clone only the SDK source (~150-200 MiB)."
        );
    }

    // Create temp directory for download
    let tmp_dir = config::tmp_dir()?;
    std::fs::create_dir_all(&tmp_dir)?;

    let archive_name = download_url
        .split('/')
        .next_back()
        .unwrap_or("flutter.tar.xz");
    let archive_path = tmp_dir.join(archive_name);

    // Download
    download_with_progress(download_url, &archive_path)?;

    // Verify SHA256 checksum (unless skipped)
    if !skip_checksum {
        verify_sha256(&archive_path, &release.sha256).context(format!(
            "SHA256 mismatch for {} — downloaded file is corrupted or incomplete",
            release.version
        ))?;
    }

    // Extract
    std::fs::create_dir_all(&env_dir)?;
    extract_archive(&archive_path, &env_dir)?;

    // Find the extracted flutter directory (archives contain a flutter/ or flutter_*/ directory)
    let extracted = std::fs::read_dir(&env_dir)?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().contains("flutter"))
        .map(|e| e.path())
        .unwrap_or_else(|| {
            // If extraction didn't create a subfolder, the env_dir IS the SDK
            env_dir.clone()
        });

    // If the SDK was extracted to a subdirectory, move contents up
    if extracted != env_dir {
        for entry in std::fs::read_dir(&extracted)? {
            let entry = entry?;
            let dest = env_dir.join(entry.file_name());
            if dest.exists() {
                std::fs::remove_dir_all(&dest).ok();
            }
            std::fs::rename(entry.path(), &dest)?;
        }
        std::fs::remove_dir_all(&extracted)?;
    }

    // Cleanup archive
    std::fs::remove_file(&archive_path)?;

    if profile.includes_engine()
        && let Ok(engine_ver) = engine_cache::read_engine_version(&env_dir)
    {
        let engine_path = env_dir.join("bin").join("cache").join("engine");
        if engine_path.exists() {
            match engine_cache::adopt_engine_dir(&env_dir, &engine_ver) {
                Ok(()) => {
                    println!("Engine {engine_ver} cached globally (shared across versions)");
                }
                Err(e) => eprintln!("Could not adopt engine: {e}"),
            }
        }
    }

    println!(
        "Flutter {version} installed successfully at {}",
        display_path(&env_dir)
    );
    Ok(())
}

/// Install a Flutter SDK version via Git with a specific profile.
pub fn install_version_git_with_profile(
    version: &str,
    repo_url: Option<&str>,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    crate::util::validate_version(version).map_err(|e| anyhow::anyhow!("{}", e))?;
    let env_dir = config::envs_dir()?.join(version);
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let already_installed = env_dir.join("bin").join("flutter").exists()
        || env_dir.join("bin").join("flutter.bat").exists();
    let is_broken_worktree = already_installed && !git_cache::worktree_is_valid(version);

    if already_installed {
        if !force && !is_broken_worktree {
            println!("Version {version} is already installed. Use --force to reinstall.");
            return Ok(());
        }
        if is_broken_worktree {
            println!("Worktree for {version} is broken (git cache was cleared). Reinstalling...");
        } else {
            println!("Reinstalling {version}...");
        }
        let cache = git_cache::GitCache::open_or_init()?;
        cache.remove_worktree(version);
        std::fs::remove_dir_all(&env_dir).ok();
    }

    let remote = repo_url.unwrap_or("https://github.com/flutter/flutter.git");
    println!("Creating lightweight toolchain for Flutter {version}...");

    let cache = git_cache::GitCache::open_or_init()?;
    let ref_kind = cache.discover_ref(remote, version)?;
    cache.fetch_shallow(remote, version, ref_kind)?;
    cache.create_worktree(version, &env_dir)?;

    // Verify the worktree is lightweight (.git is a file, not a dir)
    let git_link = env_dir.join(".git");
    if !git_link.is_file() {
        eprintln!("Toolchain is not a lightweight worktree (.git is a directory)");
    }

    if let Ok(release) = crate::releases::find_release(version) {
        let _ = std::fs::write(
            env_dir.join("bin").join("internal").join("release_branch"),
            release.channel,
        );
    }

    if let Ok(engine_ver) = engine_cache::read_engine_version(&env_dir) {
        for artifact in profile.included_artifacts() {
            match artifact {
                Artifact::FlutterFramework | Artifact::HostDevTools => (),
                Artifact::HostEngine => {
                    if !engine_cache::engine_dir(&engine_ver)?.exists() {
                        println!("Downloading engine {engine_ver}...");
                        let engine_clone = engine_ver.clone();
                        let engine_task = std::thread::spawn(move || {
                            engine_cache::download_engine(&engine_clone, skip_checksum)
                        });
                        let result = engine_task
                            .join()
                            .map_err(|_| anyhow::anyhow!("Engine download thread panicked"))??;
                        println!("Engine cached at {}", display_path(&result));
                    }

                    if let Err(e) = engine_cache::symlink_engine(&env_dir, &engine_ver) {
                        eprintln!("Could not symlink engine: {e}");
                    }
                }
                _ => {
                    let subdir = engine_cache::artifact_subdir(&artifact);
                    let target = env_dir
                        .join("bin")
                        .join("cache")
                        .join("artifacts")
                        .join(subdir);
                    if !target.exists() {
                        match engine_cache::ensure_artifact(&engine_ver, &artifact, skip_checksum) {
                            Ok(cached) => {
                                if let Some(parent) = target.parent() {
                                    std::fs::create_dir_all(parent).ok();
                                }
                                engine_cache::symlink_dir(&cached, &target).ok();
                            }
                            Err(e) => {
                                eprintln!("Could not download {:?}: {e}", artifact);
                            }
                        }
                    }
                }
            }
        }
    }

    // Save profile to sidecar for future update/repair commands
    toolchain_meta::save_profile(version, profile).ok();

    println!(
        "Flutter {version} installed at {} (lightweight worktree)",
        display_path(&env_dir)
    );
    Ok(())
}
