use crate::profile::Artifact;

pub fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux-x64",
        "macos" => "darwin-x64",
        "windows" => "windows-x64",
        _ => "unknown",
    }
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
        Artifact::DesktopLinux => format!("{base}/linux-x64/artifacts.zip"),
        Artifact::DesktopMacos => format!("{base}/darwin-x64/artifacts.zip"),
        Artifact::DesktopWindows => format!("{base}/windows-x64/artifacts.zip"),
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

pub fn artifact_subdir(artifact: &Artifact) -> &'static str {
    match artifact {
        Artifact::FlutterFramework | Artifact::HostDevTools => "",
        Artifact::HostEngine | Artifact::DesktopLinux => "linux-x64",
        Artifact::DesktopMacos => "darwin-x64",
        Artifact::DesktopWindows => "windows-x64",
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
            format!("{BASE}/linux-x64/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::DesktopMacos),
            format!("{BASE}/darwin-x64/artifacts.zip")
        );
        assert_eq!(
            url(&Artifact::DesktopWindows),
            format!("{BASE}/windows-x64/artifacts.zip")
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
    fn artifact_subdirs_match_download_urls() {
        // Web artifacts extract into per-renderer dirs but share one download.
        let web = [
            Artifact::WebEngineCanvaskit,
            Artifact::WebEngineSkwasm,
            Artifact::WebEngineHtml,
        ];
        let cases = [
            (Artifact::HostEngine, host_platform()),
            (Artifact::DesktopLinux, "linux-x64"),
            (Artifact::DesktopMacos, "darwin-x64"),
            (Artifact::DesktopWindows, "windows-x64"),
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
