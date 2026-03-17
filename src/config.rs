use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Per-srctype compiler configuration. All fields are optional at the TOML level;
/// `Config::src_config()` always returns a fully-merged value with built-in defaults applied.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SrcConfig {
    pub depens: Option<bool>,
    pub reverse_depens: Option<bool>,
    pub timeout_sec: Option<u32>,
    pub setup_timeout_sec: Option<u32>,
    pub preamble: Option<String>,
    pub postamble: Option<String>,
    pub imports: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub src: HashMap<String, SrcConfig>,
}

impl SrcConfig {
    /// Whether dependency blocks should be merged into this srctype's block.
    pub fn effective_depens(&self, _srctype: &str) -> bool {
        self.depens.unwrap_or(false)
    }

    /// Whether merged content places root first (`true`) or last (`false`).
    pub fn effective_reverse_depens(&self) -> bool {
        self.reverse_depens.unwrap_or(true)
    }

    /// Convert to the `HashMap<String, toml::Value>` expected by `CompilerReq.compcfg`.
    pub fn to_compiler_cfg(&self) -> HashMap<String, toml::Value> {
        let mut m = HashMap::new();
        if let Some(v) = self.timeout_sec {
            m.insert("timeout_sec".to_string(), toml::Value::Integer(v as i64));
        }
        if let Some(v) = self.setup_timeout_sec {
            m.insert(
                "setup_timeout_sec".to_string(),
                toml::Value::Integer(v as i64),
            );
        }
        if let Some(ref v) = self.preamble {
            m.insert("preamble".to_string(), toml::Value::String(v.clone()));
        }
        if let Some(ref v) = self.postamble {
            m.insert("postamble".to_string(), toml::Value::String(v.clone()));
        }
        if let Some(ref v) = self.imports {
            m.insert(
                "imports".to_string(),
                toml::Value::Array(v.iter().map(|s| toml::Value::String(s.clone())).collect()),
            );
        }
        m
    }
}

impl Config {
    pub fn load(mdcroot: &Path) -> Result<Self> {
        let config_path = mdcroot.join(".mdc").join("config.toml");
        if !config_path.is_file() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        if text.trim().is_empty() {
            return Ok(Config::default());
        }
        toml::from_str(&text).with_context(|| format!("invalid TOML in {}", config_path.display()))
    }

    /// Return a fully-merged `SrcConfig` for `srctype`: built-in defaults overlaid by any
    /// user settings from `.mdc/config.toml`. User `Some` values always win.
    pub fn src_config(&self, srctype: &str) -> SrcConfig {
        let defaults = default_for_srctype(srctype);
        let user = self
            .src
            .get(srctype)
            .or_else(|| self.src.get(&srctype.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_default();
        SrcConfig {
            depens: user.depens.or(defaults.depens),
            reverse_depens: user.reverse_depens.or(defaults.reverse_depens),
            timeout_sec: user.timeout_sec.or(defaults.timeout_sec),
            setup_timeout_sec: user.setup_timeout_sec.or(defaults.setup_timeout_sec),
            preamble: user.preamble.or(defaults.preamble),
            postamble: user.postamble.or(defaults.postamble),
            imports: user.imports.or(defaults.imports),
        }
    }
}

/// Built-in defaults matching the Python DEFAULT_CONFIG.
pub fn default_for_srctype(srctype: &str) -> SrcConfig {
    match srctype.to_ascii_lowercase().as_str() {
        "natl" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(true),
            ..Default::default()
        },
        "latex" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(true),
            timeout_sec: Some(30),
            preamble: Some("\\documentclass{article}\n\\begin{document}\n".to_string()),
            postamble: Some("\\end{document}\n".to_string()),
            ..Default::default()
        },
        "py" => SrcConfig {
            depens: Some(false),
            reverse_depens: Some(false),
            timeout_sec: Some(30),
            ..Default::default()
        },
        "lean" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(false),
            timeout_sec: Some(300),
            setup_timeout_sec: Some(1800),
            imports: Some(vec!["Mathlib".to_string()]),
            preamble: Some(String::new()),
            ..Default::default()
        },
        _ => SrcConfig::default(),
    }
}
