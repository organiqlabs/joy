use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

/// A Flutter SDK component that can be selected via a profile.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
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

/// Installation profile, inspired by rustup.
#[derive(Debug, Clone, PartialEq)]
pub enum Profile {
    /// Core Flutter SDK only — no engine
    Minimal,
    /// SDK + engine for current platform
    Default,
    /// SDK + engine + all platform artifacts
    Full,
    /// User-defined component selection
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

    /// Whether the profile includes platform-specific artifacts beyond the engine.
    pub fn includes_platform_artifacts(&self) -> bool {
        let comps = self.components();
        comps.contains(&Component::Android)
            || comps.contains(&Component::Ios)
            || comps.contains(&Component::Web)
            || comps.contains(&Component::Desktop)
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

    /// Parse a component from a string.
    pub fn from_str(s: &str) -> Option<Component> {
        match s.to_lowercase().as_str() {
            "sdk" => Some(Component::Sdk),
            "engine" => Some(Component::Engine),
            "android" => Some(Component::Android),
            "ios" => Some(Component::Ios),
            "web" => Some(Component::Web),
            "desktop" => Some(Component::Desktop),
            _ => None,
        }
    }
}

pub fn parse_custom(s: &str) -> Option<Profile> {
    let comps: HashSet<Component> = s.split(',').filter_map(Component::from_str).collect();
    if comps.is_empty() {
        return None;
    }
    Some(Profile::Custom(comps))
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

    #[test]
    fn test_minimal_excludes_platform_artifacts() {
        assert!(!Profile::Minimal.includes_platform_artifacts());
    }

    #[test]
    fn test_default_excludes_platform_artifacts() {
        assert!(!Profile::Default.includes_platform_artifacts());
    }

    #[test]
    fn test_full_includes_platform_artifacts() {
        assert!(Profile::Full.includes_platform_artifacts());
    }

    #[test]
    fn test_custom_engine_excludes_platform_artifacts() {
        let p = Profile::Custom(HashSet::from([Component::Engine]));
        assert!(!p.includes_platform_artifacts());
    }

    #[test]
    fn test_custom_with_android_includes_platform_artifacts() {
        let p = Profile::Custom(HashSet::from([Component::Android]));
        assert!(p.includes_platform_artifacts());
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

    // ---- Component parsing ----

    #[test]
    fn test_component_from_str_all_variants() {
        assert_eq!(Component::from_str("sdk"), Some(Component::Sdk));
        assert_eq!(Component::from_str("engine"), Some(Component::Engine));
        assert_eq!(Component::from_str("android"), Some(Component::Android));
        assert_eq!(Component::from_str("ios"), Some(Component::Ios));
        assert_eq!(Component::from_str("web"), Some(Component::Web));
        assert_eq!(Component::from_str("desktop"), Some(Component::Desktop));
    }

    #[test]
    fn test_component_from_str_invalid() {
        assert_eq!(Component::from_str("foo"), None);
        assert_eq!(Component::from_str(""), None);
    }

    // ---- Custom parsing ----

    #[test]
    fn test_parse_custom_single_component() {
        let p = parse_custom("engine").unwrap();
        assert_eq!(p, Profile::Custom(HashSet::from([Component::Engine])));
    }

    #[test]
    fn test_parse_custom_multiple() {
        let p = parse_custom("engine,android").unwrap();
        assert_eq!(
            p,
            Profile::Custom(HashSet::from([Component::Engine, Component::Android]))
        );
    }

    #[test]
    fn test_parse_custom_empty_returns_none() {
        assert_eq!(parse_custom(""), None);
        assert_eq!(parse_custom("foo,bar"), None);
    }

    // ---- Display ----

    #[test]
    fn test_display_profiles() {
        assert_eq!(Profile::Minimal.to_string(), "minimal");
        assert_eq!(Profile::Default.to_string(), "default");
        assert_eq!(Profile::Full.to_string(), "full");
    }
}
