use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{HippoError, Result};

pub const GLOBAL_SCOPE: &str = "global";
const SCOPE_MARKER: &str = ".hippo/scope";

pub fn config_path() -> PathBuf {
    home_dir().join(".config/hippo/config.toml")
}

pub fn resolve_home(explicit: Option<&Path>) -> Result<PathBuf> {
    let candidate = explicit
        .map(|path| path.to_path_buf())
        .or_else(|| std::env::var_os("HIPPO_HOME").map(PathBuf::from))
        .or_else(home_from_config);
    let Some(candidate) = candidate else {
        return Err(HippoError::Config(format!(
            "No data home configured. Set --home, HIPPO_HOME, or 'home' in {}.",
            config_path().display()
        )));
    };
    let home = expand_user(candidate);
    if home.is_dir() {
        return Ok(home);
    }
    if home.exists() {
        return Err(HippoError::Config(format!(
            "Data home is not a directory: {}",
            home.display()
        )));
    }
    fs::create_dir_all(&home).map_err(|err| {
        HippoError::Config(format!(
            "Could not create data home {}: {err}",
            home.display()
        ))
    })?;
    Ok(home)
}

/// `--embed`, then `HIPPO_EMBED`, then `embed` in the config file.
/// Callers default to `hash` when this returns `None`.
pub fn resolve_embed(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HIPPO_EMBED").ok())
        .filter(|value| !value.is_empty())
        .or_else(embed_from_config)
        .filter(|value| !value.is_empty())
}

pub fn resolve_scope(explicit: Option<&str>, start: Option<&Path>) -> String {
    let marker = scope_from_marker(start.unwrap_or(&std::env::current_dir().unwrap_or_default()));
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("HIPPO_SCOPE").ok())
        .or(marker)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| GLOBAL_SCOPE.to_string())
}

/// Harness that made a write: `--source`, then `HIPPO_SOURCE`, then the agent environment.
pub fn resolve_source(explicit: Option<String>) -> Option<String> {
    let env_set = |name: &str| std::env::var(name).is_ok_and(|value| !value.is_empty());
    explicit
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HIPPO_SOURCE").ok())
        .filter(|value| !value.is_empty())
        .or_else(|| env_set("PI_CODING_AGENT").then(|| "pi".to_string()))
        .or_else(|| env_set("CURSOR_AGENT").then(|| "cursor".to_string()))
}

fn embed_from_config() -> Option<String> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("embed")
        .and_then(|item| item.as_str())
        .map(str::to_string)
}

fn home_from_config() -> Option<PathBuf> {
    let path = config_path();
    let text = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("home")
        .and_then(|item| item.as_str())
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn scope_from_marker(start: &Path) -> Option<String> {
    let mut directory = Some(start);
    while let Some(current) = directory {
        let marker = current.join(SCOPE_MARKER);
        if marker.is_file() {
            let text = std::fs::read_to_string(marker).ok()?;
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        directory = current.parent();
    }
    None
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_user(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if text == "~" {
        return home_dir();
    }
    path
}
