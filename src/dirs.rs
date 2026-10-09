use std::env;
use std::path::PathBuf;

pub fn state() -> Option<PathBuf> {
    state_base().map(|base| base.join("tokendraw"))
}

pub fn config() -> Option<PathBuf> {
    config_base().map(|base| base.join("tokendraw"))
}

#[cfg(windows)]
fn state_base() -> Option<PathBuf> {
    absolute_var("LOCALAPPDATA")
}

#[cfg(windows)]
fn config_base() -> Option<PathBuf> {
    absolute_var("APPDATA")
}

#[cfg(target_os = "macos")]
fn state_base() -> Option<PathBuf> {
    application_support()
}

#[cfg(target_os = "macos")]
fn config_base() -> Option<PathBuf> {
    application_support()
}

#[cfg(target_os = "macos")]
fn application_support() -> Option<PathBuf> {
    env::home_dir().map(|home| home.join("Library").join("Application Support"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn state_base() -> Option<PathBuf> {
    absolute_var("XDG_STATE_HOME")
        .or_else(|| env::home_dir().map(|home| home.join(".local").join("state")))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn config_base() -> Option<PathBuf> {
    absolute_var("XDG_CONFIG_HOME").or_else(|| env::home_dir().map(|home| home.join(".config")))
}

/// The XDG specification says to ignore relative paths, which would
/// otherwise depend on the current directory.
#[cfg(not(target_os = "macos"))]
fn absolute_var(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}
