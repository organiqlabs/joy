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
        "{}/{}/engine.zip",
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
        Artifact::HostEngine
        | Artifact::DesktopLinux
        | Artifact::DesktopMacos
        | Artifact::DesktopWindows => host_engine_url(engine_version),
        Artifact::AndroidEngineArm => format!("{base}/android-arm-release/engine.zip"),
        Artifact::AndroidEngineArm64 => format!("{base}/android-arm64-release/engine.zip"),
        Artifact::AndroidEngineX64 => format!("{base}/android-x64-release/engine.zip"),
        Artifact::AndroidEngineX86 => format!("{base}/android-x86-release/engine.zip"),
        Artifact::IosEngine => format!("{base}/ios-release/engine.zip"),
        Artifact::IosSimulator => format!("{base}/ios-sim-release/engine.zip"),
        Artifact::WebEngineCanvaskit => format!("{base}/web-canvaskit/engine.zip"),
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
        Artifact::AndroidEngineX86 => "android-x86-release",
        Artifact::IosEngine => "ios-release",
        Artifact::IosSimulator => "ios-sim-release",
        Artifact::WebEngineCanvaskit => "web-canvaskit",
        Artifact::WebEngineSkwasm => "web-skwasm",
        Artifact::WebEngineHtml => "web-html",
    }
}
