//! User-level settings. Environment variables override this optional TOML file.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub url: Option<String>,
    pub terminus_url: Option<String>,
    pub terminus_user: Option<String>,
    pub terminus_password: Option<String>,
    pub cache_dir: Option<PathBuf>,
    pub lean_timeout_seconds: Option<u64>,
}

fn user_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path).join("mdc"));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
            .join(fallback)
            .join("mdc"),
    )
}

impl Settings {
    pub fn load() -> Result<Self> {
        let explicit = std::env::var_os("MDC_CONFIG");
        let path = explicit
            .clone()
            .map(PathBuf::from)
            .unwrap_or(user_dir("XDG_CONFIG_HOME", ".config")?.join("config.toml"));
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && explicit.is_none() => {
                Ok(Self::default())
            }
            Err(error) => Err(error).with_context(|| format!("read config {}", path.display())),
        }
    }
    pub fn cache_root(&self) -> Result<PathBuf> {
        let path = std::env::var_os("MDC_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| self.cache_dir.clone())
            .unwrap_or(user_dir("XDG_CACHE_HOME", ".cache")?);
        anyhow::ensure!(
            path.is_absolute(),
            "cache_dir / MDC_CACHE_DIR must be an absolute path"
        );
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_strict_and_accepts_user_settings() {
        let settings: Settings =
            toml::from_str("terminus_password = 'private'\nlean_timeout_seconds = 600").unwrap();
        assert_eq!(settings.lean_timeout_seconds, Some(600));
        assert!(toml::from_str::<Settings>("database = 'accidental-shared-project'").is_err());
    }
}
