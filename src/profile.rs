use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

/// A Flutter SDK component that can be selected via a profile.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    /// Flutter SDK framework + dart SDK (always included)
    Sdk,
    /// Flutter engine binary for the current platform
    Engine,
    /// Android platform artifacts
    Android,
    /// iOS platform artifacts
    Ios,
    /// Web platform artifacts
    Web,
    /// Desktop platform artifacts (Linux, Windows, macOS)
    Desktop,
}

/// A specific downloadable Flutter SDK artifact.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Artifact {
    /// Flutter framework source code (from git checkout)
    FlutterFramework,
    /// Host development tools: dart, dartanalyzer, dartfmt, dart2js, formatter, linter
    HostDevTools,
    /// Engine binary for the host platform (linux-x64, darwin-x64, windows-x64)
    HostEngine,
    /// Android engine — ARM 32-bit
    AndroidEngineArm,
    /// Android engine — ARM 64-bit
    AndroidEngineArm64,
    /// Android engine — x86 32-bit
    AndroidEngineX86,
    /// Android engine — x86 64-bit
    AndroidEngineX64,
    /// iOS device engine framework (arm64)
    IosEngine,
    /// iOS simulator engine framework
    IosSimulator,
    /// Web engine — CanvasKit renderer
    WebEngineCanvaskit,
    /// Web engine — Skwasm renderer
    WebEngineSkwasm,
    /// Web engine — HTML renderer
    WebEngineHtml,
    /// Linux desktop engine
    DesktopLinux,
    /// macOS desktop engine
    DesktopMacos,
    /// Windows desktop engine
    DesktopWindows,
}

/// Installation profile, inspired by rustup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", content = "components")]
pub enum Profile {
    /// Core Flutter SDK only — no engine
    #[serde(rename = "minimal")]
    Minimal,
    /// SDK + engine for current platform
    #[serde(rename = "default")]
    Default,
    /// SDK + engine + all platform artifacts
    #[serde(rename = "full")]
    Full,
    /// User-defined component selection
    #[serde(rename = "custom")]
    Custom(HashSet<Component>),
}

impl Profile {
    /// Returns the set of components enabled by this profile.
    pub fn components(&self) -> HashSet<Component> {
        match self {
            Profile::Minimal => HashSet::from([Component::Sdk]),
            Profile::Default => HashSet::from([Component::Sdk, Component::Engine]),
            Profile::Full => HashSet::from([
                Component::Sdk,
                Component::Engine,
                Component::Android,
                Component::Ios,
                Component::Web,
                Component::Desktop,
            ]),
            Profile::Custom(comps) => {
                let mut set = HashSet::from([Component::Sdk]);
                set.extend(comps.iter().cloned());
                set
            }
        }
    }

    /// Whether the profile includes engine download.
    pub fn includes_engine(&self) -> bool {
        self.components().contains(&Component::Engine)
    }

    /// Returns the set of specific artifacts included by this profile.
    pub fn included_artifacts(&self) -> HashSet<Artifact> {
        match self {
            Profile::Minimal => HashSet::from([Artifact::FlutterFramework, Artifact::HostDevTools]),
            Profile::Default => HashSet::from([
                Artifact::FlutterFramework,
                Artifact::HostDevTools,
                Artifact::HostEngine,
            ]),
            Profile::Full => HashSet::from([
                Artifact::FlutterFramework,
                Artifact::HostDevTools,
                Artifact::HostEngine,
                Artifact::AndroidEngineArm,
                Artifact::AndroidEngineArm64,
                Artifact::AndroidEngineX64,
                Artifact::AndroidEngineX86,
                Artifact::IosEngine,
                Artifact::IosSimulator,
                Artifact::WebEngineCanvaskit,
                Artifact::WebEngineSkwasm,
                Artifact::WebEngineHtml,
                Artifact::DesktopLinux,
                Artifact::DesktopMacos,
                Artifact::DesktopWindows,
            ]),
            Profile::Custom(comps) => {
                let mut set = HashSet::from([Artifact::FlutterFramework, Artifact::HostDevTools]);
                if comps.contains(&Component::Engine) {
                    set.insert(Artifact::HostEngine);
                }
                if comps.contains(&Component::Android) {
                    set.insert(Artifact::AndroidEngineArm);
                    set.insert(Artifact::AndroidEngineArm64);
                    set.insert(Artifact::AndroidEngineX64);
                    set.insert(Artifact::AndroidEngineX86);
                }
                if comps.contains(&Component::Ios) {
                    set.insert(Artifact::IosEngine);
                    set.insert(Artifact::IosSimulator);
                }
                if comps.contains(&Component::Web) {
                    set.insert(Artifact::WebEngineCanvaskit);
                    set.insert(Artifact::WebEngineSkwasm);
                    set.insert(Artifact::WebEngineHtml);
                }
                if comps.contains(&Component::Desktop) {
                    set.insert(Artifact::DesktopLinux);
                    set.insert(Artifact::DesktopMacos);
                    set.insert(Artifact::DesktopWindows);
                }
                set
            }
        }
    }
}

impl FromStr for Profile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "minimal" => Ok(Profile::Minimal),
            "default" => Ok(Profile::Default),
            "full" => Ok(Profile::Full),
            _ => Err(format!(
                "Unknown profile '{s}'. Expected 'minimal', 'default', or 'full'."
            )),
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Profile::Minimal => write!(f, "minimal"),
            Profile::Default => write!(f, "default"),
            Profile::Full => write!(f, "full"),
            Profile::Custom(comps) => {
                let names: Vec<&str> = comps.iter().map(|c| c.as_str()).collect();
                write!(f, "custom({})", names.join(","))
            }
        }
    }
}

impl Component {
    /// Returns the string representation of this component.
    pub fn as_str(&self) -> &str {
        match self {
            Component::Sdk => "sdk",
            Component::Engine => "engine",
            Component::Android => "android",
            Component::Ios => "ios",
            Component::Web => "web",
            Component::Desktop => "desktop",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Default component sets ----

    #[test]
    fn test_minimal_has_only_sdk() {
        let comps = Profile::Minimal.components();
        assert_eq!(comps.len(), 1);
        assert!(comps.contains(&Component::Sdk));
        assert!(!comps.contains(&Component::Engine));
    }

    #[test]
    fn test_default_has_sdk_and_engine() {
        let comps = Profile::Default.components();
        assert_eq!(comps.len(), 2);
        assert!(comps.contains(&Component::Sdk));
        assert!(comps.contains(&Component::Engine));
    }

    #[test]
    fn test_full_has_all_components() {
        let comps = Profile::Full.components();
        assert_eq!(comps.len(), 6);
        assert!(comps.contains(&Component::Sdk));
        assert!(comps.contains(&Component::Engine));
        assert!(comps.contains(&Component::Android));
        assert!(comps.contains(&Component::Ios));
        assert!(comps.contains(&Component::Web));
        assert!(comps.contains(&Component::Desktop));
    }

    // ---- Custom profiles ----

    #[test]
    fn test_custom_always_includes_sdk() {
        let custom = Profile::Custom(HashSet::from([Component::Engine]));
        let comps = custom.components();
        assert!(comps.contains(&Component::Sdk));
        assert!(comps.contains(&Component::Engine));
    }

    #[test]
    fn test_custom_with_multiple_components() {
        let custom = Profile::Custom(HashSet::from([Component::Engine, Component::Android]));
        let comps = custom.components();
        assert_eq!(comps.len(), 3);
        assert!(comps.contains(&Component::Sdk));
        assert!(comps.contains(&Component::Engine));
        assert!(comps.contains(&Component::Android));
    }

    // ---- Profile flags ----

    #[test]
    fn test_minimal_excludes_engine() {
        assert!(!Profile::Minimal.includes_engine());
    }

    #[test]
    fn test_default_includes_engine() {
        assert!(Profile::Default.includes_engine());
    }

    #[test]
    fn test_full_includes_engine() {
        assert!(Profile::Full.includes_engine());
    }

    // ---- Parsing ----

    #[test]
    fn test_profile_from_str_minimal() {
        assert_eq!("minimal".parse::<Profile>().unwrap(), Profile::Minimal);
    }

    #[test]
    fn test_profile_from_str_default() {
        assert_eq!("default".parse::<Profile>().unwrap(), Profile::Default);
    }

    #[test]
    fn test_profile_from_str_full() {
        assert_eq!("full".parse::<Profile>().unwrap(), Profile::Full);
    }

    #[test]
    fn test_profile_from_str_case_insensitive() {
        assert_eq!("Minimal".parse::<Profile>().unwrap(), Profile::Minimal);
        assert_eq!("DEFAULT".parse::<Profile>().unwrap(), Profile::Default);
    }

    #[test]
    fn test_profile_from_str_invalid() {
        let result = "foo".parse::<Profile>();
        assert!(result.is_err());
    }

    #[test]
    fn test_minimal_artifacts_include_framework_and_tools() {
        let artifacts = Profile::Minimal.included_artifacts();
        assert!(artifacts.contains(&Artifact::FlutterFramework));
        assert!(artifacts.contains(&Artifact::HostDevTools));
        assert_eq!(artifacts.len(), 2);
    }

    #[test]
    fn test_minimal_artifacts_exclude_all_engines() {
        let artifacts = Profile::Minimal.included_artifacts();
        assert!(!artifacts.contains(&Artifact::HostEngine));
        assert!(!artifacts.contains(&Artifact::AndroidEngineArm));
        assert!(!artifacts.contains(&Artifact::AndroidEngineArm64));
        assert!(!artifacts.contains(&Artifact::AndroidEngineX64));
        assert!(!artifacts.contains(&Artifact::AndroidEngineX86));
        assert!(!artifacts.contains(&Artifact::IosEngine));
        assert!(!artifacts.contains(&Artifact::IosSimulator));
        assert!(!artifacts.contains(&Artifact::WebEngineCanvaskit));
        assert!(!artifacts.contains(&Artifact::WebEngineSkwasm));
        assert!(!artifacts.contains(&Artifact::WebEngineHtml));
        assert!(!artifacts.contains(&Artifact::DesktopLinux));
        assert!(!artifacts.contains(&Artifact::DesktopMacos));
        assert!(!artifacts.contains(&Artifact::DesktopWindows));
    }

    #[test]
    fn test_default_artifacts_include_host_engine() {
        let artifacts = Profile::Default.included_artifacts();
        assert!(artifacts.contains(&Artifact::FlutterFramework));
        assert!(artifacts.contains(&Artifact::HostDevTools));
        assert!(artifacts.contains(&Artifact::HostEngine));
        assert!(!artifacts.contains(&Artifact::AndroidEngineArm64));
        assert!(!artifacts.contains(&Artifact::IosEngine));
        assert!(!artifacts.contains(&Artifact::DesktopLinux));
        assert_eq!(artifacts.len(), 3);
    }

    #[test]
    fn test_full_artifacts_include_all_platform_engines() {
        let artifacts = Profile::Full.included_artifacts();
        assert!(artifacts.contains(&Artifact::AndroidEngineArm));
        assert!(artifacts.contains(&Artifact::AndroidEngineArm64));
        assert!(artifacts.contains(&Artifact::AndroidEngineX64));
        assert!(artifacts.contains(&Artifact::AndroidEngineX86));
        assert!(artifacts.contains(&Artifact::IosEngine));
        assert!(artifacts.contains(&Artifact::IosSimulator));
        assert!(artifacts.contains(&Artifact::WebEngineCanvaskit));
        assert!(artifacts.contains(&Artifact::WebEngineSkwasm));
        assert!(artifacts.contains(&Artifact::WebEngineHtml));
        assert!(artifacts.contains(&Artifact::DesktopLinux));
        assert!(artifacts.contains(&Artifact::DesktopMacos));
        assert!(artifacts.contains(&Artifact::DesktopWindows));
        assert_eq!(artifacts.len(), 15);
    }

    #[test]
    fn test_custom_with_component_engine_maps_to_host_engine_artifact() {
        let p = Profile::Custom(HashSet::from([Component::Engine]));
        let artifacts = p.included_artifacts();
        assert!(artifacts.contains(&Artifact::FlutterFramework));
        assert!(artifacts.contains(&Artifact::HostDevTools));
        assert!(artifacts.contains(&Artifact::HostEngine));
        assert!(!artifacts.contains(&Artifact::AndroidEngineArm64));
    }

    #[test]
    fn test_custom_with_android_maps_to_all_android_architectures() {
        let p = Profile::Custom(HashSet::from([Component::Android]));
        let artifacts = p.included_artifacts();
        assert!(artifacts.contains(&Artifact::AndroidEngineArm));
        assert!(artifacts.contains(&Artifact::AndroidEngineArm64));
        assert!(artifacts.contains(&Artifact::AndroidEngineX64));
        assert!(artifacts.contains(&Artifact::AndroidEngineX86));
        assert!(!artifacts.contains(&Artifact::HostEngine));
        assert!(!artifacts.contains(&Artifact::IosEngine));
    }

    #[test]
    fn test_custom_with_desktop_maps_to_all_desktop_artifacts() {
        let p = Profile::Custom(HashSet::from([Component::Desktop]));
        let artifacts = p.included_artifacts();
        assert!(artifacts.contains(&Artifact::DesktopLinux));
        assert!(artifacts.contains(&Artifact::DesktopMacos));
        assert!(artifacts.contains(&Artifact::DesktopWindows));
        assert_eq!(artifacts.len(), 5); // framework + tools + 3 desktop
    }

    // ---- RED: Expanded artifact model (Phase 1) ----

    #[test]
    fn test_full_profile_includes_all_android_architectures() {
        let artifacts = Profile::Full.included_artifacts();
        assert!(
            artifacts.contains(&Artifact::AndroidEngineArm),
            "Full should include AndroidEngineArm"
        );
        assert!(
            artifacts.contains(&Artifact::AndroidEngineArm64),
            "Full should include AndroidEngineArm64"
        );
        assert!(
            artifacts.contains(&Artifact::AndroidEngineX64),
            "Full should include AndroidEngineX64"
        );
        assert!(
            artifacts.contains(&Artifact::AndroidEngineX86),
            "Full should include AndroidEngineX86"
        );
    }

    #[test]
    fn test_full_profile_includes_all_web_renderers() {
        let artifacts = Profile::Full.included_artifacts();
        assert!(
            artifacts.contains(&Artifact::WebEngineCanvaskit),
            "Full should include WebEngineCanvaskit"
        );
        assert!(
            artifacts.contains(&Artifact::WebEngineSkwasm),
            "Full should include WebEngineSkwasm"
        );
        assert!(
            artifacts.contains(&Artifact::WebEngineHtml),
            "Full should include WebEngineHtml"
        );
    }

    #[test]
    fn test_full_profile_includes_ios_simulator() {
        let artifacts = Profile::Full.included_artifacts();
        assert!(
            artifacts.contains(&Artifact::IosSimulator),
            "Full should include IosSimulator"
        );
        assert!(
            artifacts.contains(&Artifact::IosEngine),
            "Full should include IosEngine (device)"
        );
    }

    #[test]
    fn test_custom_with_ios_includes_simulator() {
        let p = Profile::Custom(HashSet::from([Component::Ios]));
        let artifacts = p.included_artifacts();
        assert!(
            artifacts.contains(&Artifact::IosEngine),
            "Custom(ios) should include IosEngine"
        );
        assert!(
            artifacts.contains(&Artifact::IosSimulator),
            "Custom(ios) should include IosSimulator"
        );
    }

    #[test]
    fn test_custom_with_web_maps_to_all_web_renderers() {
        let p = Profile::Custom(HashSet::from([Component::Web]));
        let artifacts = p.included_artifacts();
        assert!(
            artifacts.contains(&Artifact::WebEngineCanvaskit),
            "Custom(web) should include WebEngineCanvaskit"
        );
        assert!(
            artifacts.contains(&Artifact::WebEngineSkwasm),
            "Custom(web) should include WebEngineSkwasm"
        );
        assert!(
            artifacts.contains(&Artifact::WebEngineHtml),
            "Custom(web) should include WebEngineHtml"
        );
        assert!(!artifacts.contains(&Artifact::HostEngine));
        assert!(!artifacts.contains(&Artifact::AndroidEngineArm64));
    }

    // ---- Display ----

    #[test]
    fn test_display_profiles() {
        assert_eq!(Profile::Minimal.to_string(), "minimal");
        assert_eq!(Profile::Default.to_string(), "default");
        assert_eq!(Profile::Full.to_string(), "full");
    }
}
