use crate::config;
use crate::engine_cache;
use crate::git_cache::{self, Fresh, GitCache};
use crate::profile::Artifact;
use crate::profile::Profile;
use crate::releases;
use crate::toolchain_meta;
use crate::types::Version;
use crate::util::display_path;
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default stall timeout: if no bytes arrive for this long, the download is
/// considered hung and aborted. Configurable via `JOY_DOWNLOAD_STALL_TIMEOUT`
/// (in seconds).
const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Resolve the stall/idle timeout from the environment, falling back to
/// [`DEFAULT_STALL_TIMEOUT`].
fn stall_timeout() -> Duration {
    if let Ok(raw) = std::env::var("JOY_DOWNLOAD_STALL_TIMEOUT") {
        match raw.parse::<u64>() {
            Ok(secs) if secs > 0 => return Duration::from_secs(secs),
            _ => eprintln!(
                "Warning: ignoring invalid JOY_DOWNLOAD_STALL_TIMEOUT={raw:?} (expected positive seconds)"
            ),
        }
    }
    DEFAULT_STALL_TIMEOUT
}

/// Download a file with a progress bar.
///
/// There is no total download deadline — slow-but-progressing downloads must
/// never be killed (see `crate::http_client`). Instead, the body is read on a
/// worker thread and the main thread observes progress via `recv_timeout`,
/// which acts as a per-chunk idle/stall timeout: if no data arrives within
/// [`stall_timeout`], the download is considered hung and aborted.
pub(crate) fn download_with_progress(url: &str, dest: &Path) -> Result<()> {
    if crate::is_verbose() {
        eprintln!("[debug] Downloading {url}");
    }
    // Require a successful HTTP status BEFORE creating the destination file:
    // without this, a 404/500 error page would be written to disk as an SDK
    // archive — undetectable with --skip-checksum, and a misleading checksum
    // failure otherwise. error_for_status() surfaces the real HTTP error
    // (e.g. "404 Not Found") instead.
    let resp = crate::http_client()
        .get(url)
        .send()
        .context(format!("Failed to start download from {url}"))?
        .error_for_status()
        .context(format!("Download from {url} failed"))?;

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
    // Only bound the reader when the size is known: with no Content-Length,
    // total_size is 0 and take(0.max(1)) would truncate the download to 1 byte.
    let mut source: Box<dyn Read + Send> = match total_size {
        0 => Box::new(resp),
        n => Box::new(resp.take(n)),
    };

    // Read the body on a worker thread, forwarding each chunk size (or error)
    // over the channel. The main thread applies the stall timeout.
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<usize>>();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut source, &mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if let Err(e) = std::io::Write::write_all(&mut dest_file, &buffer[..n]) {
                        let _ = tx.send(Err(e));
                        break;
                    }
                    if tx.send(Ok(n)).is_err() {
                        break; // main thread gave up (stall detected)
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });

    let stall = stall_timeout();
    let mut downloaded: u64 = 0;
    loop {
        match rx.recv_timeout(stall) {
            Ok(Ok(n)) => {
                downloaded += n as u64;
                pb.set_position(downloaded);
            }
            Ok(Err(e)) => return Err(e).context("Failed while reading download stream"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                anyhow::bail!(
                    "Download stalled: no data received for {}s. Retry, or adjust \
                    JOY_DOWNLOAD_STALL_TIMEOUT.",
                    stall.as_secs()
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // EOF
        }
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

/// A staged-install rollback guard.
///
/// Toolchain installs are transactional: the replacement is fully built in a
/// sibling staging directory and only swapped into place once validated. This
/// guard remembers what the swap did (including the git worktree registration
/// it may overwrite) so that any failure — a bad download, a failed checksum,
/// failed extraction, or a failed final rename — leaves the previous
/// installation exactly as it was.
///
/// Dropping an armed guard restores the previous installation; [`commit`]
/// disarms it and deletes the backup after a fully successful install.
///
/// [`commit`]: InstallRollback::commit
struct InstallRollback {
    env_dir: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
    backup_created: bool,
    staged_in_place: bool,
    /// Bare-repo worktree registration files (path → original content; `None`
    /// means the file did not exist and must be removed on restore).
    worktree_registration: Vec<(PathBuf, Option<String>)>,
    disarmed: bool,
}

impl InstallRollback {
    fn new(env_dir: &Path, staging: &Path) -> Self {
        let name = env_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("toolchain");
        Self {
            env_dir: env_dir.to_path_buf(),
            staging: staging.to_path_buf(),
            backup: env_dir.with_file_name(format!(".joy-backup-{name}-{}", std::process::id())),
            backup_created: false,
            staged_in_place: false,
            worktree_registration: Vec::new(),
            disarmed: false,
        }
    }

    /// Snapshot the previous git worktree registration so it can be restored
    /// if the replacement install fails after overwriting it.
    fn capture_worktree_registration(&mut self, cache: &GitCache<Fresh>, version: &Version) {
        self.worktree_registration = cache.snapshot_worktree_registration(version);
    }

    /// Disarm the guard and delete the backup: the new installation is in
    /// place and has been fully validated.
    fn commit(mut self) {
        self.disarmed = true;
        if self.backup_created {
            let _ = std::fs::remove_dir_all(&self.backup);
        }
    }
}

impl Drop for InstallRollback {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // If the swap completed but a later step failed, remove the new
        // installation so the previous one can take its place.
        if self.staged_in_place
            && self.env_dir.exists()
            && let Err(e) = std::fs::remove_dir_all(&self.env_dir)
        {
            eprintln!(
                "Warning: failed to remove partially-installed {}: {e}",
                self.env_dir.display()
            );
        }
        // Restore the previous installation (only if the new one is gone).
        if self.backup_created
            && self.backup.exists()
            && !self.env_dir.exists()
            && let Err(e) = std::fs::rename(&self.backup, &self.env_dir)
        {
            eprintln!(
                "CRITICAL: failed to restore previous installation from {} to {}: {e}",
                self.backup.display(),
                self.env_dir.display()
            );
        }
        // Discard any leftover staging directory.
        if self.staging.exists() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
        // Put the previous git worktree registration back.
        crate::git_cache::restore_worktree_registration(&self.worktree_registration);
    }
}

/// Unique sibling staging directory for building a replacement installation.
/// Lives inside `envs` so the final rename onto `envs/<version>` stays on the
/// same filesystem and is atomic.
fn staging_dir(envs: &Path, version: &Version) -> PathBuf {
    envs.join(format!(
        ".joy-staging-{}-{}",
        version.as_str(),
        std::process::id()
    ))
}

/// Move a fully-built `staging` directory into place at `env_dir`, moving any
/// existing installation aside to the rollback backup first. Runs `finalize`
/// (e.g. git worktree registration) after the swap; if it fails, the guard
/// restores the previous installation.
fn transactional_replace(
    env_dir: &Path,
    staging: &Path,
    rollback: &mut InstallRollback,
    finalize: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if env_dir.exists() {
        let _ = std::fs::remove_dir_all(&rollback.backup);
        std::fs::rename(env_dir, &rollback.backup).with_context(|| {
            format!(
                "Failed to move existing installation aside to {}",
                rollback.backup.display()
            )
        })?;
        rollback.backup_created = true;
    }
    std::fs::rename(staging, env_dir).with_context(|| {
        format!(
            "Failed to move staged installation into place at {}",
            env_dir.display()
        )
    })?;
    rollback.staged_in_place = true;
    finalize()
}

/// If the archive extracted a single `flutter*/` top-level directory
/// (Flutter's release archive layout), move its contents up into `root` and
/// remove the wrapper. No-op when the archive extracted directly into `root`.
fn flatten_sdk(root: &Path) -> Result<()> {
    let extracted = std::fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().contains("flutter"))
        .map(|e| e.path());
    let Some(extracted) = extracted else {
        return Ok(());
    };
    for entry in std::fs::read_dir(&extracted)? {
        let entry = entry?;
        let dest = root.join(entry.file_name());
        if dest.exists() {
            std::fs::remove_dir_all(&dest).ok();
        }
        std::fs::rename(entry.path(), &dest)?;
    }
    std::fs::remove_dir_all(&extracted)?;
    Ok(())
}

/// Whether an SDK layout has the expected `bin/flutter` entry point.
fn has_flutter_binary(root: &Path) -> bool {
    root.join("bin").join("flutter").exists() || root.join("bin").join("flutter.bat").exists()
}

/// Install a specific Flutter version with a given profile
pub fn install_version(
    version: &Version,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    let envs = config::envs_dir()?;
    let env_dir = envs.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &envs).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Check if already installed
    if has_flutter_binary(&env_dir) {
        if !force {
            println!("Version {version} is already installed. Use --force to reinstall.");
            return Ok(());
        }
        println!("Reinstalling {version}...");
    }

    // Find the release info
    let release = releases::find_release(version.as_str())?;
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
    // Unique per process so two parallel installs cannot clobber each other.
    let archive_path = tmp_dir.join(format!("{}-{archive_name}", std::process::id()));

    // Download
    download_with_progress(download_url, &archive_path)?;

    // Verify SHA256 checksum (unless skipped)
    if !skip_checksum {
        verify_sha256(&archive_path, &release.sha256).context(format!(
            "SHA256 mismatch for {} — downloaded file is corrupted or incomplete",
            release.version
        ))?;
    }

    // ---- Build the replacement in a sibling staging directory ----
    // The existing installation is never touched until the staged SDK has been
    // downloaded, verified, extracted, and validated. On any failure the
    // rollback guard removes the staging dir and the previous SDK stays intact.
    let staging = staging_dir(&envs, version);
    let mut rollback = InstallRollback::new(&env_dir, &staging);
    std::fs::create_dir_all(&staging)?;

    let build = (|| -> Result<()> {
        extract_archive(&archive_path, &staging)?;
        flatten_sdk(&staging)?;

        // Verify the expected Flutter layout before replacing anything.
        if !has_flutter_binary(&staging) {
            anyhow::bail!(
                "Downloaded archive for {} does not contain a Flutter SDK \
                (no bin/flutter after extraction). The existing installation \
                was preserved.",
                version
            );
        }

        if profile.includes_engine() {
            match engine_cache::read_engine_version(&staging) {
                Ok(engine_ver) => {
                    let engine_path = staging.join("bin").join("cache").join("engine");
                    if engine_path.exists() {
                        match engine_cache::adopt_engine_dir(&staging, engine_ver.as_str()) {
                            Ok(()) => {
                                println!(
                                    "Engine {engine_ver} cached globally (shared across versions)"
                                );
                            }
                            Err(e) => eprintln!("Could not adopt engine: {e}"),
                        }
                    } else {
                        eprintln!(
                            "Warning: engine directory not found at {} — engine was not cached",
                            display_path(&engine_path)
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Warning: could not read engine version for caching: {e}");
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = build {
        std::fs::remove_file(&archive_path).ok();
        return Err(e); // guard drop removes staging; existing install untouched
    }

    // ---- Swap: old → backup, staged → in place ----
    transactional_replace(&env_dir, &staging, &mut rollback, || Ok(()))?;
    // Archive cleanup is best-effort — the swap is the point of no return.
    std::fs::remove_file(&archive_path).ok();
    rollback.commit();

    println!(
        "Flutter {version} installed successfully at {}",
        display_path(&env_dir)
    );
    Ok(())
}

/// Install a Flutter SDK version via Git with a specific profile.
///
/// Transactional: the network phase (ref discovery + shallow fetch) runs first
/// and touches nothing locally; the new worktree is then checked out into a
/// sibling staging directory where its engines are resolved. Only after all of
/// that succeeds is the old installation moved aside and the staged one swapped
/// into place (and registered as a worktree). A failure at any point — network,
/// checkout, or engine download — leaves the previous SDK fully intact.
pub fn install_version_git_with_profile(
    version: &Version,
    repo_url: Option<&str>,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    let envs = config::envs_dir()?;
    let env_dir = envs.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &envs).map_err(|e| anyhow::anyhow!("{e}"))?;

    let already_installed = has_flutter_binary(&env_dir);
    let is_broken_worktree = already_installed && !git_cache::worktree_is_valid(version.as_str());

    if already_installed && !force && !is_broken_worktree {
        println!("Version {version} is already installed. Use --force to reinstall.");
        return Ok(());
    }
    if is_broken_worktree {
        println!("Worktree for {version} is broken (git cache was cleared). Reinstalling...");
    } else if force && already_installed {
        println!("Reinstalling {version}...");
    }

    let remote = repo_url.unwrap_or("https://github.com/flutter/flutter.git");
    println!("Creating lightweight toolchain for Flutter {version}...");

    // Typestate: Git operations in sequence:
    // Fresh → discover_ref → RemoteDiscovered → fetch_shallow → Fresh →
    // checkout_worktree (staged) → register_worktree (after the swap)
    let cache = GitCache::<Fresh>::open_or_init()?;
    let cache = cache.discover_ref(remote, version)?; // Fresh → RemoteDiscovered
    let cache = cache.fetch_shallow(remote, version)?; // RemoteDiscovered → Fresh

    // ---- Build the replacement in a sibling staging directory ----
    // The previous worktree (and its registration) stays fully intact until the
    // staged SDK is checked out and its engines resolved. `--force` no longer
    // deletes the previous install up front.
    let staging = staging_dir(&envs, version);
    let mut rollback = InstallRollback::new(&env_dir, &staging);
    rollback.capture_worktree_registration(&cache, version);

    let build = (|| -> Result<()> {
        cache.checkout_worktree(version, &staging)?;

        // Verify the expected Flutter layout before replacing anything.
        if !has_flutter_binary(&staging) {
            anyhow::bail!(
                "Checked-out commit for {} does not contain a Flutter SDK \
                (no bin/flutter). The previous installation was preserved.",
                version
            );
        }

        if let Ok(release) = crate::releases::find_release(version.as_str()) {
            let _ = std::fs::write(
                staging.join("bin").join("internal").join("release_branch"),
                release.channel.as_str(),
            );
        }

        if let Ok(engine_ver) = engine_cache::read_engine_version(&staging) {
            let ev_str = engine_ver.as_str().to_string();
            for artifact in profile.included_artifacts() {
                match artifact {
                    Artifact::FlutterFramework | Artifact::HostDevTools => (),
                    Artifact::HostEngine => {
                        if !engine_cache::engine_dir(&ev_str)
                            .ok()
                            .is_some_and(|d| d.exists())
                        {
                            println!("Downloading engine {ev_str}...");
                            let ec = ev_str.clone();
                            let engine_task = std::thread::spawn(move || {
                                engine_cache::download_engine(&ec, skip_checksum)
                            });
                            let result = engine_task
                                .join()
                                .map_err(|_| anyhow::anyhow!("Engine download thread panicked"))?;
                            if let Err(e) = result {
                                anyhow::bail!(
                                    "Failed to download engine for {version}: {e}. \
                                    Nothing was installed; the previous installation \
                                    was preserved."
                                );
                            }
                        }

                        if let Err(e) = engine_cache::symlink_engine(&staging, &ev_str) {
                            eprintln!("Could not symlink engine: {e}");
                        }
                    }
                    _ => {
                        let subdir = engine_cache::artifact_subdir(&artifact);
                        let target = staging
                            .join("bin")
                            .join("cache")
                            .join("artifacts")
                            .join(subdir);
                        if !target.exists() {
                            match engine_cache::ensure_artifact(&ev_str, &artifact, skip_checksum) {
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
        Ok(())
    })();
    build?; // guard drop removes staging; previous install untouched

    // ---- Swap: old worktree → backup, staged → in place, then register ----
    transactional_replace(&env_dir, &staging, &mut rollback, || {
        cache.register_worktree(version, &env_dir)
    })?;

    // Save profile to sidecar for future update/repair commands
    toolchain_meta::save_profile(version, profile).ok();

    rollback.commit();

    println!(
        "Flutter {version} installed at {} (lightweight worktree)",
        display_path(&env_dir)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // The stall-timeout tests mutate the process-global JOY_DOWNLOAD_STALL_TIMEOUT
    // env var (and the download tests read it via stall_timeout). std::env::set_var
    // is not thread-safe against concurrent access, so these must not run in
    // parallel with each other — otherwise one test can observe another test's
    // value (e.g. read "-5" while the invalid-values test is mid-loop).

    #[test]
    #[serial]
    fn stall_timeout_defaults_to_60_seconds() {
        unsafe {
            std::env::remove_var("JOY_DOWNLOAD_STALL_TIMEOUT");
        }
        assert_eq!(stall_timeout(), Duration::from_secs(60));
    }

    #[test]
    #[serial]
    fn stall_timeout_reads_valid_env_override() {
        unsafe {
            std::env::set_var("JOY_DOWNLOAD_STALL_TIMEOUT", "120");
        }
        assert_eq!(stall_timeout(), Duration::from_secs(120));
        unsafe {
            std::env::remove_var("JOY_DOWNLOAD_STALL_TIMEOUT");
        }
    }

    #[test]
    #[serial]
    fn stall_timeout_ignores_invalid_env_values() {
        for bad in ["0", "-5", "abc", ""] {
            unsafe {
                std::env::set_var("JOY_DOWNLOAD_STALL_TIMEOUT", bad);
            }
            assert_eq!(
                stall_timeout(),
                Duration::from_secs(60),
                "{bad:?} should fall back to the default"
            );
        }
        unsafe {
            std::env::remove_var("JOY_DOWNLOAD_STALL_TIMEOUT");
        }
    }

    /// Serve a single HTTP/1.1 response with the given status code on an
    /// ephemeral local port and return the URL to request it at.
    ///
    /// With `with_length == true` the response carries a `Content-Length` header;
    /// with `false` the body is sent chunked with no Content-Length at all (the
    /// mirror/proxy scenario where the client cannot know the size up front).
    fn serve_once(status: u16, body: &[u8], with_length: bool) -> String {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let body = body.to_vec();

        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Drain the request head, scanning the *accumulated* bytes so a
            // \r\n\r\n terminator spanning two reads cannot be missed.
            let mut head = Vec::with_capacity(1024);
            let mut tmp = [0u8; 1024];
            loop {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&tmp[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() > 8192 {
                    break;
                }
            }

            let reason = match status {
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "Error",
            };
            let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
            if with_length {
                response.push_str(&format!("Content-Length: {}\r\n", body.len()));
                response.push_str("Connection: close\r\n\r\n");
            } else {
                response.push_str("Transfer-Encoding: chunked\r\n");
                response.push_str("Connection: close\r\n\r\n");
            }
            let mut out = response.into_bytes();
            if with_length {
                out.extend_from_slice(&body);
            } else {
                for chunk in body.chunks(16) {
                    out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                    out.extend_from_slice(chunk);
                    out.extend_from_slice(b"\r\n");
                }
                out.extend_from_slice(b"0\r\n\r\n");
            }
            let _ = stream.write_all(&out);
        });

        url
    }

    fn temp_download_file(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "joy_download_test_{tag}_{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn pseudo_random_body(len: usize) -> Vec<u8> {
        (0..len as u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    #[serial]
    fn download_with_progress_reads_full_body_without_content_length() {
        // Chunked response, no Content-Length: the pre-fix code did
        // `take(total_size.max(1))` → take(1), silently writing a 1-byte file.
        let body = pseudo_random_body(100_000);
        let url = serve_once(200, &body, false);
        let dest = temp_download_file("no_content_length");

        download_with_progress(&url, &dest).expect("chunked download should succeed");

        let downloaded = std::fs::read(&dest).unwrap();
        assert_eq!(
            downloaded, body,
            "download without Content-Length must not be truncated"
        );
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    #[serial]
    fn download_with_progress_reads_full_body_with_content_length() {
        let body = pseudo_random_body(100_000);
        let url = serve_once(200, &body, true);
        let dest = temp_download_file("with_content_length");

        download_with_progress(&url, &dest).expect("download with Content-Length should succeed");

        let downloaded = std::fs::read(&dest).unwrap();
        assert_eq!(
            downloaded, body,
            "download with Content-Length must match the body"
        );
        let _ = std::fs::remove_file(&dest);
    }

    /// The regression guard for non-success HTTP statuses: an error page must
    /// never be persisted to the destination, because with --skip-checksum a
    /// saved 404/500 body would flow straight into archive extraction as a
    /// corrupt SDK (and even with checksums, the user deserves the real HTTP
    /// failure rather than a checksum mismatch). `download_with_progress` must
    /// reject the response before `File::create` is reached.
    #[test]
    #[serial]
    fn download_with_progress_rejects_404_without_creating_file() {
        let url = serve_once(404, b"<html>Not Found</html>", true);
        let dest = temp_download_file("http_404");

        let err =
            download_with_progress(&url, &dest).expect_err("a 404 response must fail the download");
        // `{err:#}` prints the anyhow chain including the reqwest source error,
        // which carries the status code.
        assert!(
            format!("{err:#}").contains("404"),
            "error should surface the HTTP status, got: {err:#}"
        );
        assert!(
            !dest.exists(),
            "no destination file may be created for a failed download"
        );
    }

    #[test]
    #[serial]
    fn download_with_progress_rejects_500_without_creating_file() {
        let url = serve_once(500, b"<html>Internal Server Error</html>", true);
        let dest = temp_download_file("http_500");

        let err =
            download_with_progress(&url, &dest).expect_err("a 500 response must fail the download");
        assert!(
            format!("{err:#}").contains("500"),
            "error should surface the HTTP status, got: {err:#}"
        );
        assert!(
            !dest.exists(),
            "no destination file may be created for a failed download"
        );
    }

    // ---- Transactional install: failure-injection tests ----
    //
    // The whole point of staging + atomic swap is that a failed --force install
    // must preserve the previous SDK. These tests inject failures at each phase
    // of the transaction and assert the previous installation survives.

    static TEST_DIR_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let id = TEST_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_install_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_sdk(dir: &Path, marker: &str) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            dir.join("bin").join("flutter"),
            format!("#!/bin/sh\necho {marker}"),
        )
        .unwrap();
        std::fs::write(dir.join("marker.txt"), marker).unwrap();
    }

    fn only_entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn failed_build_preserves_previous_sdk() {
        // Failure BEFORE the swap (e.g. a bad download/checksum/extraction):
        // the guard must discard the staging dir and leave env_dir untouched.
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();
        let env_dir = tmp.join(version.as_str());
        let staging = staging_dir(&tmp, &version);
        fake_sdk(&env_dir, "OLD");
        fake_sdk(&staging, "NEW");

        let rollback = InstallRollback::new(&env_dir, &staging);
        drop(rollback); // simulates an error before the swap

        assert_eq!(
            std::fs::read_to_string(env_dir.join("marker.txt")).unwrap(),
            "OLD",
            "previous SDK must survive a failed build"
        );
        assert!(
            !staging.exists(),
            "staging dir must be removed after a failed build"
        );
        assert_eq!(
            only_entry_names(&tmp),
            vec![version.as_str().to_string()],
            "no staging/backup remnants may remain"
        );
    }

    #[test]
    fn failed_finalize_restores_previous_sdk() {
        // Failure AFTER the swap moved old→backup and staging→env_dir (e.g. a
        // failed git registration): the guard must roll the swap back.
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();
        let env_dir = tmp.join(version.as_str());
        let staging = staging_dir(&tmp, &version);
        fake_sdk(&env_dir, "OLD");
        fake_sdk(&staging, "NEW");

        let mut rollback = InstallRollback::new(&env_dir, &staging);
        let result = transactional_replace(&env_dir, &staging, &mut rollback, || {
            anyhow::bail!("injected finalize failure after the swap")
        });
        assert!(result.is_err(), "injected finalize failure must propagate");
        drop(rollback); // triggers restoration

        assert_eq!(
            std::fs::read_to_string(env_dir.join("marker.txt")).unwrap(),
            "OLD",
            "previous SDK must be restored after a failed finalize"
        );
        assert!(!staging.exists(), "staging dir must be removed");
        assert_eq!(
            only_entry_names(&tmp),
            vec![version.as_str().to_string()],
            "backup must not remain after rollback"
        );
    }

    #[test]
    fn failed_finalize_without_previous_removes_everything() {
        // Fresh install whose finalize fails: nothing may be left behind.
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();
        let env_dir = tmp.join(version.as_str());
        let staging = staging_dir(&tmp, &version);
        fake_sdk(&staging, "NEW");

        let mut rollback = InstallRollback::new(&env_dir, &staging);
        let result = transactional_replace(&env_dir, &staging, &mut rollback, || {
            anyhow::bail!("injected finalize failure after the swap")
        });
        assert!(result.is_err());
        drop(rollback);

        assert!(
            !env_dir.exists(),
            "failed fresh install must not leave an env dir"
        );
        assert!(!staging.exists(), "staging dir must be removed");
        assert!(only_entry_names(&tmp).is_empty(), "nothing may remain");
    }

    #[test]
    fn successful_replace_commits_new_sdk_and_removes_backup() {
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();
        let env_dir = tmp.join(version.as_str());
        let staging = staging_dir(&tmp, &version);
        fake_sdk(&env_dir, "OLD");
        fake_sdk(&staging, "NEW");

        let mut rollback = InstallRollback::new(&env_dir, &staging);
        transactional_replace(&env_dir, &staging, &mut rollback, || Ok(())).unwrap();
        rollback.commit();

        assert_eq!(
            std::fs::read_to_string(env_dir.join("marker.txt")).unwrap(),
            "NEW",
            "new SDK must be in place after a successful swap"
        );
        assert!(!staging.exists(), "staging dir must be gone after commit");
        assert_eq!(
            only_entry_names(&tmp),
            vec![version.as_str().to_string()],
            "backup must be removed after commit"
        );
    }
}
