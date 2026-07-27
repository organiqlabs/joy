pub mod cache;
pub mod cli;
pub mod completions;
pub mod config;
pub mod doctor;
pub mod engine_cache;
pub mod environment;
pub mod git_cache;
pub mod install;
pub mod profile;
pub mod project;
pub mod releases;
pub mod toolchain;
pub mod toolchain_meta;
pub mod types;
pub mod util;

use std::sync::OnceLock;

static VERBOSE: OnceLock<bool> = OnceLock::new();

/// Set the global verbose flag. Should be called once at startup.
pub fn set_verbose(v: bool) {
    let _ = VERBOSE.set(v);
}

/// Check whether verbose debug output is enabled.
pub fn is_verbose() -> bool {
    VERBOSE.get().copied().unwrap_or(false)
}

static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// Return a shared `reqwest::blocking::Client` with sensible defaults for CLI operations.
pub fn http_client() -> &'static reqwest::blocking::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("joy/0.1.0")
            .build()
            .expect("Failed to build HTTP client")
    })
}
