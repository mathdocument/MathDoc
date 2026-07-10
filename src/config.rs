use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const KNOWN_SRCTYPES: [&str; 5] = ["text", "latex", "python", "lean", "rocq"];

pub fn canonical_srctype(srctype: &str) -> &str {
    KNOWN_SRCTYPES
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(srctype))
        .unwrap_or(srctype)
}

pub fn validate_srctype_name(srctype: &str) -> Result<()> {
    if srctype.is_empty()
        || matches!(srctype, "." | "..")
        || !srctype
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
    {
        anyhow::bail!("unsafe srctype name '{srctype}'");
    }
    Ok(())
}

/// Per-srctype compiler configuration. All fields are optional at the TOML level;
/// `Config::src_config()` always returns a fully-merged value with built-in defaults applied.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SrcConfig {
    pub depens: Option<bool>,
    pub reverse_depens: Option<bool>,
    pub timeout_sec: Option<u32>,
    pub setup_timeout_sec: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub src: HashMap<String, SrcConfig>,
}

impl SrcConfig {
    /// Whether dependency blocks should be merged into this srctype's block.
    pub fn effective_depens(&self) -> bool {
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
            validate_srctype_name(&srctype)?;
            if !seen.insert(srctype.to_ascii_lowercase()) {
                anyhow::bail!("duplicate srctype configuration '{srctype}' ignoring case");
            }
            let canonical = canonical_srctype(&srctype).to_string();
            src.insert(canonical, config);
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
            depens: user.depens.or(defaults.depens),
            reverse_depens: user.reverse_depens.or(defaults.reverse_depens),
            timeout_sec: user.timeout_sec.or(defaults.timeout_sec),
            setup_timeout_sec: user.setup_timeout_sec.or(defaults.setup_timeout_sec),
        }
    }
}

/// Built-in defaults matching the Python DEFAULT_CONFIG.
pub fn default_for_srctype(srctype: &str) -> SrcConfig {
    match canonical_srctype(srctype) {
        "text" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(true),
            ..Default::default()
        },
        "latex" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(true),
            timeout_sec: Some(30),
            ..Default::default()
        },
        "python" => SrcConfig {
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
            ..Default::default()
        },
        "rocq" => SrcConfig {
            depens: Some(true),
            reverse_depens: Some(false),
            timeout_sec: Some(300),
            ..Default::default()
        },
        _ => SrcConfig::default(),
    }
}

// ── Preamble / postamble file management ─────────────────────────────────────

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

/// Hardcoded default preamble per srctype.
pub fn default_preamble(srctype: &str) -> &'static str {
    match canonical_srctype(srctype) {
        "latex" => "\\documentclass{article}\n\\begin{document}\n",
        _ => "",
    }
}

/// Hardcoded default postamble per srctype.
pub fn default_postamble(srctype: &str) -> &'static str {
    match canonical_srctype(srctype) {
        "latex" => "\\end{document}\n",
        _ => "",
    }
}

pub(crate) fn amble_path(mdcroot: &Path, srctype: &str, kind: &str) -> PathBuf {
    let srctype = canonical_srctype(srctype);
    let ext = srctype_ext(srctype);
    mdcroot
        .join(".mdc")
        .join(srctype)
        .join(format!("{kind}.{ext}"))
}

/// Read preamble for `srctype`: file if it exists, else hardcoded default.
pub fn read_preamble(mdcroot: &Path, srctype: &str) -> String {
    let path = amble_path(mdcroot, srctype, "preamble");
    std::fs::read_to_string(&path).unwrap_or_else(|_| default_preamble(srctype).to_string())
}

/// Read postamble for `srctype`: file if it exists, else hardcoded default.
pub fn read_postamble(mdcroot: &Path, srctype: &str) -> String {
    let path = amble_path(mdcroot, srctype, "postamble");
    std::fs::read_to_string(&path).unwrap_or_else(|_| default_postamble(srctype).to_string())
}

/// Write preamble file for `srctype`.
pub fn write_preamble(mdcroot: &Path, srctype: &str, content: &str) -> Result<()> {
    write_amble(mdcroot, srctype, "preamble", content)
}

/// Write postamble file for `srctype`.
pub fn write_postamble(mdcroot: &Path, srctype: &str, content: &str) -> Result<()> {
    write_amble(mdcroot, srctype, "postamble", content)
}

fn write_amble(mdcroot: &Path, srctype: &str, kind: &str, content: &str) -> Result<()> {
    validate_srctype_name(srctype)?;
    let srctype = canonical_srctype(srctype);
    let mdc_dir = mdcroot.join(".mdc");
    match std::fs::symlink_metadata(&mdc_dir) {
        Ok(_) => crate::safe_file::ensure_regular_directory(&mdc_dir)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&mdc_dir)?
        }
        Err(error) => return Err(error.into()),
    }
    let dir = mdc_dir.join(srctype);
    match std::fs::symlink_metadata(&dir) {
        Ok(_) => crate::safe_file::ensure_regular_directory(&dir)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)?,
        Err(error) => return Err(error.into()),
    }
    let path = amble_path(mdcroot, srctype, kind);
    let snapshot = crate::safe_file::FileSnapshot::capture(&path)?;
    crate::safe_file::atomic_replace(&path, &snapshot, content.as_bytes())?;
    Ok(())
}

/// Write default preamble/postamble files for all known srctypes.
/// Called by `mdc init`. Only creates files that don't already exist.
pub fn init_amble_files(mdcroot: &Path) -> Result<()> {
    for srctype in &["text", "latex", "python", "lean", "rocq"] {
        let pre = default_preamble(srctype);
        let post = default_postamble(srctype);
        let pre_path = amble_path(mdcroot, srctype, "preamble");
        let post_path = amble_path(mdcroot, srctype, "postamble");
        if !pre.is_empty() || !post.is_empty() {
            std::fs::create_dir_all(pre_path.parent().unwrap())?;
        }
        if !pre.is_empty() && !pre_path.exists() {
            std::fs::write(&pre_path, pre)?;
        }
        if !post.is_empty() && !post_path.exists() {
            std::fs::write(&post_path, post)?;
        }
    }
    Ok(())
}
