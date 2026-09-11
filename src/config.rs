//! User-level settings. Environment variables override this optional TOML file.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
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
    pub fn terminus_url(&self) -> String {
        std::env::var("MDC_TERMINUS_URL")
            .ok()
            .or_else(|| self.terminus_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:6363".into())
            .trim_end_matches('/')
            .into()
    }

    pub fn project_cache(&self, project: &str) -> Result<PathBuf> {
        let (database, branch) = project_parts(project)?;
        Ok(self
            .cache_root()?
            .join(&crate::store::digest(self.terminus_url().as_bytes())[..12])
            .join(database)
            .join(branch))
    }

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

pub(crate) fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "database and branch names allow letters, digits, hyphens and underscores"
    );
    Ok(())
}

pub(crate) fn project_parts(project: &str) -> Result<(&str, &str)> {
    let (database, branch) = project
        .split_once('/')
        .context("use DATABASE/BRANCH, for example etp/main")?;
    validate_name(database)?;
    validate_name(branch)?;
    Ok((database, branch))
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
        assert!(toml::from_str::<Settings>("url = 'http://127.0.0.1:7599'").is_err());
    }
}
