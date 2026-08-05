use crate::profile::Artifact;
use std::sync::OnceLock;

/// Host OS name in Flutter's engine artifact naming (`linux` | `darwin` |
/// `windows`).
pub fn host_os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => "unknown",
    }
}

/// Map a machine string (from `uname -m`, `std::env::consts::ARCH`, …) to
/// Flutter's engine artifact architecture suffix (`x64` | `arm64`).
///
/// Returns `None` for architectures Flutter does not publish host-engine
/// artifacts for.
fn arch_suffix(machine: &str) -> Option<&'static str> {
    match machine {
        "x86_64" | "amd64" | "x64" => Some("x64"),
        "aarch64" | "arm64" => Some("arm64"),
        _ => None,
    }
}

/// Host CPU architecture in Flutter's engine artifact naming (`x64` | `arm64`).
///
/// Detected at runtime via `uname -m` (Linux and macOS) so native installs
/// fetch the correct engine artifacts — Flutter publishes `*-arm64` directories
/// alongside `*-x64`. Falls back to the compile-time target arch when `uname` is
/// unavailable (e.g. stock Windows).
pub fn host_arch() -> &'static str {
    static ARCH: OnceLock<&'static str> = OnceLock::new();
    ARCH.get_or_init(|| {
        let detected = std::process::Command::new("uname")
            .arg("-m")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if let Some(arch) = arch_suffix(&detected) {
            return arch;
        }
        if let Some(arch) = arch_suffix(std::env::consts::ARCH) {
            return arch;
        }
        eprintln!(
            "Warning: unsupported host architecture (uname: {detected:?}, \
            build target: {}), defaulting to x64 engine artifacts",
            std::env::consts::ARCH
        );
        "x64"
    })
}

/// Flutter engine artifact platform name for the host (e.g. `linux-x64`,
/// `darwin-arm64`, `windows-x64`).
pub fn host_platform() -> &'static str {
    static PLATFORM: OnceLock<&'static str> = OnceLock::new();
    PLATFORM.get_or_init(|| {
        let platform = format!("{}-{}", host_os_name(), host_arch());
        Box::leak(platform.into_boxed_str())
    })
}

fn engine_base_url(engine_version: &str) -> String {
    format!("https://storage.googleapis.com/flutter_infra_release/flutter/{engine_version}")
}

fn host_engine_url(engine_version: &str) -> String {
    format!(
        "{}/{}/artifacts.zip",
        engine_base_url(engine_version),
        host_platform()
    )
}

pub fn engine_download_url(engine_version: &str) -> String {
    host_engine_url(engine_version)
}

pub fn artifact_download_url(engine_version: &str, artifact: &Artifact) -> String {
    let base = engine_base_url(engine_version);
    match artifact {
        Artifact::FlutterFramework | Artifact::HostDevTools => String::new(),
        Artifact::HostEngine => host_engine_url(engine_version),
        Artifact::DesktopLinux => format!("{base}/linux-{}/artifacts.zip", host_arch()),
        Artifact::DesktopMacos => format!("{base}/darwin-{}/artifacts.zip", host_arch()),
        Artifact::DesktopWindows => format!("{base}/windows-{}/artifacts.zip", host_arch()),
        Artifact::AndroidEngineArm => format!("{base}/android-arm-release/artifacts.zip"),
        Artifact::AndroidEngineArm64 => format!("{base}/android-arm64-release/artifacts.zip"),
        Artifact::AndroidEngineX64 => format!("{base}/android-x64-release/artifacts.zip"),
        Artifact::AndroidEngineX86 => format!("{base}/android-x86/artifacts.zip"),
        Artifact::IosEngine => format!("{base}/ios/artifacts.zip"),
        Artifact::IosSimulator => format!("{base}/ios/artifacts.zip"),
        Artifact::WebEngineCanvaskit => format!("{base}/flutter-web-sdk.zip"),
        Artifact::WebEngineSkwasm => format!("{base}/flutter-web-sdk.zip"),
        Artifact::WebEngineHtml => format!("{base}/flutter-web-sdk.zip"),
    }
}

/// Cache directory name for a desktop engine platform: `<os>-<host-arch>`.
fn desktop_subdir(os: &str) -> &'static str {
    static LINUX: OnceLock<&'static str> = OnceLock::new();
    static DARWIN: OnceLock<&'static str> = OnceLock::new();
    static WINDOWS: OnceLock<&'static str> = OnceLock::new();
    let slot = match os {
        "linux" => &LINUX,
        "darwin" => &DARWIN,
        "windows" => &WINDOWS,
        _ => return "unknown",
    };
    slot.get_or_init(|| Box::leak(format!("{os}-{}", host_arch()).into_boxed_str()))
}

pub fn artifact_subdir(artifact: &Artifact) -> &'static str {
    match artifact {
        Artifact::FlutterFramework | Artifact::HostDevTools => "",
        Artifact::HostEngine => host_platform(),
        Artifact::DesktopLinux => desktop_subdir("linux"),
        Artifact::DesktopMacos => desktop_subdir("darwin"),
        Artifact::DesktopWindows => desktop_subdir("windows"),
        Artifact::AndroidEngineArm => "android-arm-release",
        Artifact::AndroidEngineArm64 => "android-arm64-release",
        Artifact::AndroidEngineX64 => "android-x64-release",
        Artifact::AndroidEngineX86 => "android-x86",
        Artifact::IosEngine => "ios",
        Artifact::IosSimulator => "ios",
        Artifact::WebEngineCanvaskit => "web-canvaskit",
        Artifact::WebEngineSkwasm => "web-skwasm",
        Artifact::WebEngineHtml => "web-html",
    }
}

/// Directory names inside the `flutter-web-sdk.zip` archive and the cache
/// subdirectories they are extracted to.
///
/// Kept in this module — next to [`artifact_download_url`] — because the
/// archive's internal layout and the URL that fetches it are coupled: the
/// canvaskit/skwasm/html assets live *inside* `flutter-web-sdk.zip`, not under
/// `web-canvaskit/…`-style paths on the CDN. The web extraction step
/// (`engine_cache::extract_web_sdk`) consumes this table rather than duplicating
/// the mapping.
pub fn web_sdk_entries() -> &'static [(&'static str, &'static str)] {
    &[
        ("canvaskit", "web-canvaskit"),
        ("skwasm", "web-skwasm"),
        ("html", "web-html"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Artifact;

    /// A real stable engine revision (Flutter stable, 2026-07).
    const ENGINE: &str = "0cd610717bde95fd88343c64f81c11ba4e5c0010";
    const BASE: &str = "https://storage.googleapis.com/flutter_infra_release/flutter/0cd610717bde95fd88343c64f81c11ba4e5c0010";

    fn url(artifact: &Artifact) -> String {
        artifact_download_url(ENGINE, artifact)
    }

    #[test]
    fn host_engine_uses_artifacts_zip() {
        assert_eq!(
            engine_download_url(ENGINE),
            format!("{BASE}/{}/artifacts.zip", host_platform())
        );
        assert_eq!(
            url(&Artifact::HostEngine),
            format!("{BASE}/{}/artifacts.zip", host_platform())
        );
    }

    #[test]
    fn desktop_artifacts_use_their_own_platform_dirs() {
        assert_eq!(
            url(&Artifact::DesktopLinux),
            format!("{BASE}/linux-{}/artifacts.zip", host_arch())
        );
        assert_eq!(
            url(&Artifact::DesktopMacos),
            format!("{BASE}/darwin-{}/artifacts.zip", host_arch())
        );
        assert_eq!(
            url(&Artifact::DesktopWindows),
            format!("{BASE}/windows-{}/artifacts.zip", host_arch())
        );
    }

    #[test]
    fn android_artifacts_use_artifacts_zip() {
        assert_eq!(
            url(&Artifact::AndroidEngineArm),
            format!("{BASE}/android-arm-release/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::AndroidEngineArm64),
            format!("{BASE}/android-arm64-release/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::AndroidEngineX64),
            format!("{BASE}/android-x64-release/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::AndroidEngineX86),
            format!("{BASE}/android-x86/artifacts.zip")
        );
    }

    #[test]
    fn ios_artifacts_share_the_ios_artifacts_zip() {
        assert_eq!(
            url(&Artifact::IosEngine),
            format!("{BASE}/ios/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::IosSimulator),
            format!("{BASE}/ios/artifacts.zip")
        );
    }

    #[test]
    fn web_artifacts_all_download_flutter_web_sdk() {
        for artifact in [
            Artifact::WebEngineCanvaskit,
            Artifact::WebEngineSkwasm,
            Artifact::WebEngineHtml,
        ] {
            assert_eq!(url(&artifact), format!("{BASE}/flutter-web-sdk.zip"));
        }
    }

    #[test]
    fn platform_artifact_urls_point_at_artifacts_zip() {
        // Shape-level guard: if infra ever renames the per-platform archives,
        // this fails loudly instead of silently downloading from a new path.
        for artifact in [
            Artifact::HostEngine,
            Artifact::DesktopLinux,
            Artifact::DesktopMacos,
            Artifact::DesktopWindows,
            Artifact::AndroidEngineArm,
            Artifact::AndroidEngineArm64,
            Artifact::AndroidEngineX64,
            Artifact::AndroidEngineX86,
            Artifact::IosEngine,
            Artifact::IosSimulator,
        ] {
            let u = url(&artifact);
            assert!(
                u.ends_with("/artifacts.zip"),
                "{artifact:?} URL should point at <platform>/artifacts.zip: {u}"
            );
        }
        for artifact in [
            Artifact::WebEngineCanvaskit,
            Artifact::WebEngineSkwasm,
            Artifact::WebEngineHtml,
        ] {
            assert!(url(&artifact).ends_with("flutter-web-sdk.zip"));
        }
        // Non-downloadable artifacts carry no URL at all.
        assert!(url(&Artifact::FlutterFramework).is_empty());
        assert!(url(&Artifact::HostDevTools).is_empty());
    }

    #[test]
    fn web_sdk_entries_match_artifact_subdirs() {
        // The in-archive layout table and artifact_subdir must stay in sync,
        // or web extraction would rename into subdirs the URL logic disagrees
        // with.
        let cases = [
            (Artifact::WebEngineCanvaskit, "canvaskit"),
            (Artifact::WebEngineSkwasm, "skwasm"),
            (Artifact::WebEngineHtml, "html"),
        ];
        for (artifact, in_zip) in cases {
            let (_, subdir) = web_sdk_entries()
                .iter()
                .find(|(old, _)| old == &in_zip)
                .unwrap_or_else(|| panic!("{in_zip} missing from web_sdk_entries"));
            assert_eq!(
                artifact_subdir(&artifact),
                *subdir,
                "{artifact:?} subdir must match the web SDK archive layout"
            );
        }
    }

    #[test]
    fn no_downloadable_artifact_uses_engine_zip() {
        for artifact in [
            Artifact::HostEngine,
            Artifact::DesktopLinux,
            Artifact::DesktopMacos,
            Artifact::DesktopWindows,
            Artifact::AndroidEngineArm,
            Artifact::AndroidEngineArm64,
            Artifact::AndroidEngineX64,
            Artifact::AndroidEngineX86,
            Artifact::IosEngine,
            Artifact::IosSimulator,
            Artifact::WebEngineCanvaskit,
            Artifact::WebEngineSkwasm,
            Artifact::WebEngineHtml,
        ] {
            let u = url(&artifact);
            assert!(
                !u.ends_with("engine.zip"),
                "{artifact:?} still uses the dead engine.zip suffix: {u}"
            );
        }
    }

    #[test]
    fn arch_suffix_maps_uname_machine_strings() {
        assert_eq!(arch_suffix("x86_64"), Some("x64"));
        assert_eq!(arch_suffix("amd64"), Some("x64"));
        assert_eq!(arch_suffix("x64"), Some("x64"));
        assert_eq!(arch_suffix("aarch64"), Some("arm64"));
        assert_eq!(arch_suffix("arm64"), Some("arm64"));
        assert_eq!(arch_suffix("riscv64"), None);
        assert_eq!(arch_suffix(""), None);
    }

    #[test]
    fn host_platform_follows_flutter_os_arch_naming() {
        let platform = host_platform();
        let (os, arch) = platform
            .split_once('-')
            .expect("host platform must be <os>-<arch>");
        assert!(
            matches!(os, "linux" | "darwin" | "windows"),
            "os part: {os}"
        );
        assert!(matches!(arch, "x64" | "arm64"), "arch part: {arch}");
        assert_eq!(os, host_os_name());
        assert_eq!(arch, host_arch());
        assert_eq!(desktop_subdir(os), platform);
    }

    #[test]
    fn artifact_subdirs_match_download_urls() {
        // Web artifacts extract into per-renderer dirs but share one download.
        let web = [
            Artifact::WebEngineCanvaskit,
            Artifact::WebEngineSkwasm,
            Artifact::WebEngineHtml,
        ];
        let cases = [
            (Artifact::HostEngine, host_platform()),
            (Artifact::DesktopLinux, desktop_subdir("linux")),
            (Artifact::DesktopMacos, desktop_subdir("darwin")),
            (Artifact::DesktopWindows, desktop_subdir("windows")),
            (Artifact::AndroidEngineArm, "android-arm-release"),
            (Artifact::AndroidEngineArm64, "android-arm64-release"),
            (Artifact::AndroidEngineX64, "android-x64-release"),
            (Artifact::AndroidEngineX86, "android-x86"),
            (Artifact::IosEngine, "ios"),
            (Artifact::IosSimulator, "ios"),
            (Artifact::WebEngineCanvaskit, "web-canvaskit"),
            (Artifact::WebEngineSkwasm, "web-skwasm"),
            (Artifact::WebEngineHtml, "web-html"),
        ];
        for (artifact, subdir) in cases {
            assert_eq!(artifact_subdir(&artifact), subdir, "{artifact:?}");
            let u = url(&artifact);
            if web.contains(&artifact) {
                assert_eq!(u, format!("{BASE}/flutter-web-sdk.zip"));
            } else {
                assert!(
                    u.contains(subdir),
                    "{artifact:?} URL {u} does not contain its subdir {subdir}"
                );
            }
        }
    }
}
