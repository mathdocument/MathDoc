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
    pub port: Option<u16>,
    pub public_origin: Option<String>,
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
    pub fn listen_address(&self) -> Result<std::net::Ipv4Addr> {
        let value = std::env::var("MDC_LISTEN_ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
        anyhow::ensure!(
            matches!(value.as_str(), "127.0.0.1" | "0.0.0.0"),
            "MDC_LISTEN_ADDRESS must be 127.0.0.1 or 0.0.0.0"
        );
        Ok(value.parse()?)
    }

    pub fn server_port(&self) -> Result<u16> {
        let port = self.port.unwrap_or(17843);
        anyhow::ensure!(port != 0, "port must be between 1 and 65535");
        Ok(port)
    }

    pub fn public_origin(&self) -> Result<Option<String>> {
        self.public_origin.as_ref().map(|value| {
            let url = reqwest::Url::parse(value).context("invalid public_origin")?;
            anyhow::ensure!(matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some() && url.username().is_empty()
                && url.password().is_none() && url.path() == "/"
                && url.query().is_none() && url.fragment().is_none(),
                "public_origin must be an HTTP(S) origin without credentials, path, query or fragment");
            Ok(url.origin().ascii_serialization())
        }).transpose()
    }

    pub fn server_cache(&self) -> Result<PathBuf> {
        Ok(self
            .cache_root()?
            .join(&crate::store::digest(self.terminus_url().as_bytes())[..12])
            .join(".server"))
    }

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
        let path = match &explicit {
            Some(path) => PathBuf::from(path),
            None => user_dir("XDG_CONFIG_HOME", ".config")?.join("config.toml"),
        };
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
            .map_or_else(|| user_dir("XDG_CACHE_HOME", ".cache"), Ok)?;
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
        assert_eq!(Settings::default().server_port().unwrap(), 17843);
        assert!(toml::from_str::<Settings>("port = 0")
            .unwrap()
            .server_port()
            .is_err());
        let settings: Settings =
            toml::from_str("public_origin = 'https://mdc.example.test/'").unwrap();
        let origin = settings.public_origin().unwrap().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("host", "mdc.example.test".parse().unwrap());
        headers.insert("origin", origin.parse().unwrap());
        assert!(crate::service::allowed_origin(&headers, Some(&origin)));
        assert!(!crate::service::allowed_origin(&headers, None));
        headers.insert("origin", "https://attacker.invalid".parse().unwrap());
        assert!(!crate::service::allowed_origin(&headers, Some(&origin)));
        for invalid in [
            "https://host/path",
            "https://user:secret@host",
            "https://host?query",
            "file:///tmp",
        ] {
            let settings = Settings {
                public_origin: Some(invalid.into()),
                ..Default::default()
            };
            assert!(settings.public_origin().is_err());
        }
    }
}
