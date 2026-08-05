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
use std::path::Path;
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
    let resp = crate::http_client()
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

/// Install a specific Flutter version with a given profile
pub fn install_version(
    version: &Version,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
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

    if profile.includes_engine() {
        match engine_cache::read_engine_version(&env_dir) {
            Ok(engine_ver) => {
                let engine_path = env_dir.join("bin").join("cache").join("engine");
                if engine_path.exists() {
                    match engine_cache::adopt_engine_dir(&env_dir, engine_ver.as_str()) {
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

    println!(
        "Flutter {version} installed successfully at {}",
        display_path(&env_dir)
    );
    Ok(())
}

/// Install a Flutter SDK version via Git with a specific profile.
pub fn install_version_git_with_profile(
    version: &Version,
    repo_url: Option<&str>,
    force: bool,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let already_installed = env_dir.join("bin").join("flutter").exists()
        || env_dir.join("bin").join("flutter.bat").exists();
    let is_broken_worktree = already_installed && !git_cache::worktree_is_valid(version.as_str());

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
        let cache = GitCache::<Fresh>::open_or_init()?;
        cache.remove_worktree(version);
        std::fs::remove_dir_all(&env_dir).ok();
    }

    let remote = repo_url.unwrap_or("https://github.com/flutter/flutter.git");
    println!("Creating lightweight toolchain for Flutter {version}...");

    // Typestate: Git operations in sequence:
    // Fresh → discover_ref → RemoteDiscovered → fetch_shallow → Fresh → create_worktree
    let cache = GitCache::<Fresh>::open_or_init()?;
    let cache = cache.discover_ref(remote, version)?; // Fresh → RemoteDiscovered
    let cache = cache.fetch_shallow(remote, version)?; // RemoteDiscovered → Fresh
    cache.create_worktree(version, &env_dir)?;

    // Verify the worktree is lightweight (.git is a file, not a dir)
    let git_link = env_dir.join(".git");
    if !git_link.is_file() {
        eprintln!("Toolchain is not a lightweight worktree (.git is a directory)");
    }

    if let Ok(release) = crate::releases::find_release(version.as_str()) {
        let _ = std::fs::write(
            env_dir.join("bin").join("internal").join("release_branch"),
            release.channel.as_str(),
        );
    }

    if let Ok(engine_ver) = engine_cache::read_engine_version(&env_dir) {
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
                                The SDK source is available at {}, but the engine \
                                was not cached. Use --force to retry.",
                                display_path(&env_dir)
                            );
                        }
                    }

                    if let Err(e) = engine_cache::symlink_engine(&env_dir, &ev_str) {
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

    // Save profile to sidecar for future update/repair commands
    toolchain_meta::save_profile(version, profile).ok();

    println!(
        "Flutter {version} installed at {} (lightweight worktree)",
        display_path(&env_dir)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_timeout_defaults_to_60_seconds() {
        unsafe {
            std::env::remove_var("JOY_DOWNLOAD_STALL_TIMEOUT");
        }
        assert_eq!(stall_timeout(), Duration::from_secs(60));
    }

    #[test]
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

    /// Serve a single HTTP/1.1 response on an ephemeral local port and return
    /// the URL to request it at.
    ///
    /// With `with_length == true` the response carries a `Content-Length` header;
    /// with `false` the body is sent chunked with no Content-Length at all (the
    /// mirror/proxy scenario where the client cannot know the size up front).
    fn serve_once(body: &[u8], with_length: bool) -> String {
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

            let mut response = String::from("HTTP/1.1 200 OK\r\n");
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
    fn download_with_progress_reads_full_body_without_content_length() {
        // Chunked response, no Content-Length: the pre-fix code did
        // `take(total_size.max(1))` → take(1), silently writing a 1-byte file.
        let body = pseudo_random_body(100_000);
        let url = serve_once(&body, false);
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
    fn download_with_progress_reads_full_body_with_content_length() {
        let body = pseudo_random_body(100_000);
        let url = serve_once(&body, true);
        let dest = temp_download_file("with_content_length");

        download_with_progress(&url, &dest).expect("download with Content-Length should succeed");

        let downloaded = std::fs::read(&dest).unwrap();
        assert_eq!(
            downloaded, body,
            "download with Content-Length must match the body"
        );
        let _ = std::fs::remove_file(&dest);
    }
}
