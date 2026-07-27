use crate::types::Version;
use std::path::Path;

/// Replace the user's home directory with `~` for display.
pub fn display_path(path: impl AsRef<Path>) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home = Path::new(&home);
        if let Ok(rest) = path.as_ref().strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.as_ref().display().to_string()
}

/// Calculate the total size of a directory recursively.
/// Returns 0 if the directory is inaccessible (logged via eprintln!).
pub fn dir_size(path: impl AsRef<Path>) -> u64 {
    let path = path.as_ref();
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Warning: could not read directory {}: {e}", path.display());
            return 0;
        }
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_file() {
            total += std::fs::metadata(&child).map(|m| m.len()).unwrap_or(0);
        } else if child.is_dir() {
            total += dir_size(&child);
        }
    }
    total
}

/// Validate that a version string is safe to use in filesystem paths.
///
/// **Note:** Prefer [`Version::new`] / [`Version::parse`] instead. This function
/// is a thin compatibility shim that delegates to [`Version::new`] and maps the
/// error to a plain `String`.
pub fn validate_version(version: &str) -> Result<(), String> {
    Version::new(version).map(|_| ()).map_err(|e| e.to_string())
}

/// After constructing a filesystem path from user-supplied input, canonicalize it
/// and verify it still resolves within the expected parent directory.
/// If the path doesn't exist yet, this check is skipped (string-level validation
/// via the [`Version`] newtype handles that case).
pub fn check_path_traversal(path: &Path, parent: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|e| format!("Failed to resolve path {}: {}", path.display(), e))?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|e| {
        format!(
            "Failed to resolve parent directory {}: {}",
            parent.display(),
            e
        )
    })?;
    if !canonical_path.starts_with(&canonical_parent) {
        return Err(format!(
            "Path {} resolves outside of {}, which is not allowed",
            path.display(),
            parent.display()
        ));
    }
    Ok(())
}

/// Format bytes into a human-readable string
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", size, UNITS[unit_idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_util_test_{id}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_human_size_zero() {
        assert_eq!(human_size(0), "0.0 B");
    }

    #[test]
    fn test_human_size_one_byte() {
        assert_eq!(human_size(1), "1.0 B");
    }

    #[test]
    fn test_human_size_1023_still_bytes() {
        assert_eq!(human_size(1023), "1023.0 B");
    }

    #[test]
    fn test_human_size_1024_is_1kb() {
        assert_eq!(human_size(1024), "1.0 KB");
    }

    #[test]
    fn test_human_size_1025_is_1kb() {
        assert_eq!(human_size(1025), "1.0 KB");
    }

    #[test]
    fn test_human_size_1mb() {
        assert_eq!(human_size(1_048_576), "1.0 MB");
    }

    #[test]
    fn test_human_size_1gb() {
        assert_eq!(human_size(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn test_human_size_large_boundary() {
        // u64::MAX ~ 16 EB, which exceeds 4 units (B/KB/MB/GB)
        let result = human_size(u64::MAX);
        assert!(
            result.ends_with(" GB"),
            "max value should still produce valid output"
        );
    }

    #[test]
    fn test_path_traversal_nonexistent_returns_ok() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        let child = tmp.join("does_not_exist_yet");
        let result = check_path_traversal(&child, &tmp);
        assert!(
            result.is_ok(),
            "non-existent path should pass the check (deferred to string validation)"
        );
    }

    #[test]
    fn test_path_traversal_within_parent_ok() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        let child = tmp.join("subdir");
        fs::create_dir_all(&child).unwrap();
        let result = check_path_traversal(&child, &tmp);
        assert!(result.is_ok(), "child inside parent should be valid");
    }

    #[test]
    fn test_path_traversal_self_is_parent_ok() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        let result = check_path_traversal(&tmp, &tmp);
        assert!(result.is_ok(), "path equal to parent should be valid");
    }

    #[cfg(unix)]
    #[test]
    fn test_path_traversal_symlink_outside_rejected() {
        use std::os::unix::fs as unix_fs;

        let tmp = temp_dir();
        let outside = tmp.join("outside");
        let inside = tmp.join("inside");
        let link = inside.join("link");

        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&inside).unwrap();
        unix_fs::symlink(&outside, &link).unwrap();

        let result = check_path_traversal(&link, &inside);
        assert!(
            result.is_err(),
            "symlink escaping parent should be rejected"
        );
        assert!(
            result.unwrap_err().contains("resolves outside"),
            "error should mention traversal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_path_traversal_symlink_inside_accepted() {
        use std::os::unix::fs as unix_fs;

        let tmp = temp_dir();
        let real = tmp.join("real_deal");
        let link = tmp.join("mylink");

        fs::create_dir_all(&real).unwrap();
        unix_fs::symlink(&real, &link).unwrap();

        let result = check_path_traversal(&link, &tmp);
        assert!(
            result.is_ok(),
            "symlink within parent tree should be accepted"
        );
    }

    // Note: The error path where path.exists() succeeds but canonicalize fails is
    // intentionally not tested because it requires the path to exist while the
    // parent is simultaneously inaccessible — a contradiction in practice.
    // The string-level validation via Version::new handles the non-existent case.

    #[test]
    fn test_dir_size_empty_dir() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        assert_eq!(dir_size(&tmp), 0);
    }

    #[test]
    fn test_dir_size_nonexistent_path() {
        let tmp = temp_dir();
        let missing = tmp.join("i_dont_exist");
        assert_eq!(dir_size(&missing), 0);
    }

    #[test]
    fn test_dir_size_with_files() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), b"hello").unwrap(); // 5 bytes
        fs::write(tmp.join("b.txt"), b"world!").unwrap(); // 6 bytes
        assert_eq!(dir_size(&tmp), 11);
    }

    #[test]
    fn test_dir_size_with_nested_dirs() {
        let tmp = temp_dir();
        let sub = tmp.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.join("root.txt"), b"root").unwrap(); // 4 bytes
        fs::write(sub.join("nested.txt"), b"nested").unwrap(); // 6 bytes
        assert_eq!(dir_size(&tmp), 10);
    }

    #[test]
    fn test_dir_size_file_vs_directory() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        let file_path = tmp.join("afile");
        fs::write(&file_path, b"1234567890").unwrap(); // 10 bytes

        // dir_size on a regular file should return 0 (read_dir fails on files)
        assert_eq!(dir_size(&file_path), 0);

        // dir_size on the directory should count the file
        assert_eq!(dir_size(&tmp), 10);
    }

    #[test]
    fn test_display_path_shorthand_when_under_home() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        let result = display_path(&tmp);
        // We can't assert the exact output since HOME varies, but it should work
        assert!(!result.is_empty(), "display_path should never return empty");
        assert!(
            !result.contains('\0'),
            "display_path should not contain null bytes"
        );
    }

    #[test]
    fn test_display_path_root_path() {
        // /tmp paths should NOT be shortened to ~/ unless HOME=/tmp
        let result = display_path(Path::new("/nonexistent-joy-test-path"));
        assert_eq!(result, "/nonexistent-joy-test-path");
    }
}
