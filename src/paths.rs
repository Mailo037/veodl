use std::env;
use std::path::{Component, Path, PathBuf};

/// Node-style platform name: win32, darwin, linux or android (Termux).
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "android") || termux() {
        "android"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// A Linux build running inside Termux behaves like the Android package.
fn termux() -> bool {
    cfg!(target_os = "linux") && (env::var_os("TERMUX_VERSION").is_some() || env::var("PREFIX").is_ok_and(|prefix| prefix.starts_with("/data/data/com.termux")))
}

/// Node-style architecture name: x64, arm64, ia32 or arm.
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        "arm" => "arm",
        other => other,
    }
}

pub fn is_windows() -> bool {
    cfg!(windows)
}

#[allow(deprecated)]
pub fn home() -> PathBuf {
    env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// Single source of truth for veo's per-user cache root. An invalid relative OS
/// cache setting must never write into the output directory.
pub fn cache_base() -> PathBuf {
    let home = home();
    let base = match platform() {
        "win32" => absolute_env("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local")),
        "darwin" => home.join("Library").join("Caches"),
        _ => absolute_env("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")),
    };
    let base = if base.is_absolute() { base } else { home.join(".cache") };
    base.join("veo")
}

/// Per-user configuration directory, following each platform's own convention.
pub fn config_base() -> PathBuf {
    let home = home();
    let base = match platform() {
        "win32" => absolute_env("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming")),
        "darwin" => home.join("Library").join("Application Support"),
        _ => absolute_env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
    };
    let base = if base.is_absolute() { base } else { home.join(".config") };
    base.join("veo")
}

/// Lexical normalization of an absolute path: `.` and `..` are resolved and
/// trailing separators dropped, like Node's path.resolve.
pub fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let root = result.parent().is_none();
                if !root {
                    result.pop();
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

/// Absolute, normalized form of `path` relative to the current directory.
pub fn resolve(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        return normalize(path);
    }
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize(&cwd.join(path))
}

pub fn resolve_from(base: impl AsRef<Path>, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&resolve(base).join(path))
    }
}

pub fn display(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

/// Directory part of a path string, as Node's path.dirname.
pub fn dirname(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) => PathBuf::from("."),
        None => path.to_path_buf(),
    }
}

pub fn basename(path: impl AsRef<Path>) -> String {
    path.as_ref().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Extension including the dot, or empty, like Node's path.extname.
pub fn extname(path: impl AsRef<Path>) -> String {
    let name = basename(path);
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(index) => name[index..].to_string(),
    }
}

pub fn exe_suffix() -> &'static str {
    if is_windows() {
        ".exe"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parent_components() {
        let base = if cfg!(windows) { PathBuf::from(r"C:\a\b") } else { PathBuf::from("/a/b") };
        let expected = if cfg!(windows) { PathBuf::from(r"C:\a\c") } else { PathBuf::from("/a/c") };
        assert_eq!(normalize(&base.join("..").join("c").join(".")), expected);
    }

    #[test]
    fn extension_matches_node() {
        assert_eq!(extname("video.mp4"), ".mp4");
        assert_eq!(extname(".hidden"), "");
        assert_eq!(extname("archive.tar.gz"), ".gz");
        assert_eq!(extname("plain"), "");
    }
}
