use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

pub const BUILTIN_SRCTYPES: [&str; 5] = ["text", "latex", "python", "lean", "rocq"];

pub fn config_template() -> String {
    let mut out = String::from(
        "# MathDoc configuration\n\
         # Uncomment and edit sections below to override built-in defaults.\n",
    );

    for srctype in BUILTIN_SRCTYPES {
        let config = default_for_srctype(srctype);
        out.push('\n');
        out.push_str(&format!("# [src.{srctype}]\n"));
        if let Some(timeout_sec) = config.timeout_sec {
            out.push_str(&format!("# timeout_sec = {timeout_sec}\n"));
        }
        if let Some(setup_timeout_sec) = config.setup_timeout_sec {
            out.push_str(&format!("# setup_timeout_sec = {setup_timeout_sec}\n"));
        }
    }

    out
}

pub fn builtin_srctype(srctype: &str) -> Result<&'static str> {
    BUILTIN_SRCTYPES
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(srctype))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported srctype '{srctype}'; expected one of: {}",
                BUILTIN_SRCTYPES.join(", ")
            )
        })
}

pub fn canonical_srctype(srctype: &str) -> &str {
    BUILTIN_SRCTYPES
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(srctype))
        .unwrap_or(srctype)
}

pub fn validate_srctype_name(srctype: &str) -> Result<()> {
    builtin_srctype(srctype)?;
    Ok(())
}

/// Per-srctype compiler configuration. All fields are optional at the TOML level;
/// `Config::src_config()` always returns a fully-merged value with built-in defaults applied.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SrcConfig {
    pub timeout_sec: Option<u32>,
    pub setup_timeout_sec: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub src: HashMap<String, SrcConfig>,
}

impl SrcConfig {
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
        let parsed: Config = toml::from_str(&text)
            .with_context(|| format!("invalid TOML in {}", config_path.display()))?;
        let mut src = HashMap::new();
        let mut seen = HashSet::new();
        for (srctype, config) in parsed.src {
            let canonical = builtin_srctype(&srctype)?;
            if !seen.insert(srctype.to_ascii_lowercase()) {
                anyhow::bail!("duplicate srctype configuration '{srctype}' ignoring case");
            }
            src.insert(canonical.to_string(), config);
        }
        Ok(Config { src })
    }

    /// Return a fully-merged `SrcConfig` for `srctype`: built-in defaults overlaid by any
    /// user settings from `.mdc/config.toml`. User `Some` values always win.
    pub fn src_config(&self, srctype: &str) -> SrcConfig {
        let srctype = canonical_srctype(srctype);
        let defaults = default_for_srctype(srctype);
        let user = self.src.get(srctype).cloned().unwrap_or_default();
        SrcConfig {
            timeout_sec: user.timeout_sec.or(defaults.timeout_sec),
            setup_timeout_sec: user.setup_timeout_sec.or(defaults.setup_timeout_sec),
        }
    }
}

/// Built-in defaults for the compilers shipped with MathDoc.
pub fn default_for_srctype(srctype: &str) -> SrcConfig {
    match canonical_srctype(srctype) {
        "text" => SrcConfig::default(),
        "latex" => SrcConfig {
            timeout_sec: Some(30),
            ..Default::default()
        },
        "python" => SrcConfig {
            timeout_sec: Some(30),
            ..Default::default()
        },
        "lean" => SrcConfig {
            timeout_sec: Some(300),
            setup_timeout_sec: Some(1800),
        },
        "rocq" => SrcConfig {
            timeout_sec: Some(300),
            ..Default::default()
        },
        _ => SrcConfig::default(),
    }
}

/// Srctype → file extension.
pub fn srctype_ext(srctype: &str) -> &str {
    match canonical_srctype(srctype) {
        "text" => "txt",
        "latex" => "tex",
        "python" => "py",
        "lean" => "lean",
        "rocq" => "v",
        _ => srctype,
    }
}
