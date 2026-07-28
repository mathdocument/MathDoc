use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{de::Error as _, Deserialize, Deserializer};

#[derive(Clone, Copy)]
struct BuiltinSrctype {
    name: &'static str,
    extension: &'static str,
    defaults: SrcConfig,
}

const BUILTIN_SRCTYPES: [BuiltinSrctype; 5] = [
    BuiltinSrctype {
        name: "text",
        extension: "txt",
        defaults: SrcConfig {
            timeout_sec: None,
            setup_timeout_sec: None,
        },
    },
    BuiltinSrctype {
        name: "latex",
        extension: "tex",
        defaults: SrcConfig {
            timeout_sec: NonZeroU64::new(30),
            setup_timeout_sec: None,
        },
    },
    BuiltinSrctype {
        name: "python",
        extension: "py",
        defaults: SrcConfig {
            timeout_sec: NonZeroU64::new(30),
            setup_timeout_sec: None,
        },
    },
    BuiltinSrctype {
        name: "lean",
        extension: "lean",
        defaults: SrcConfig {
            timeout_sec: NonZeroU64::new(300),
            setup_timeout_sec: NonZeroU64::new(1800),
        },
    },
    BuiltinSrctype {
        name: "rocq",
        extension: "v",
        defaults: SrcConfig {
            timeout_sec: NonZeroU64::new(300),
            setup_timeout_sec: None,
        },
    },
];

fn builtin_descriptor(srctype: &str) -> Option<&'static BuiltinSrctype> {
    BUILTIN_SRCTYPES
        .iter()
        .find(|known| known.name.eq_ignore_ascii_case(srctype))
}

pub fn builtin_srctypes() -> impl ExactSizeIterator<Item = &'static str> {
    BUILTIN_SRCTYPES.iter().map(|known| known.name)
}

pub fn config_template() -> String {
    let mut out = String::from(
        "# MathDoc configuration\n\
         # Uncomment and edit sections below to override built-in defaults.\n",
    );

    for srctype in builtin_srctypes() {
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
    builtin_descriptor(srctype)
        .map(|known| known.name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported srctype '{srctype}'; expected one of: {}",
                builtin_srctypes().collect::<Vec<_>>().join(", ")
            )
        })
}

pub fn canonical_srctype(srctype: &str) -> &str {
    builtin_descriptor(srctype)
        .map(|known| known.name)
        .unwrap_or(srctype)
}

/// Per-srctype compiler configuration. All fields are optional at the TOML level;
/// `Config::src_config()` always returns a fully-merged value with built-in defaults applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SrcConfig {
    #[serde(default, deserialize_with = "deserialize_positive_seconds")]
    timeout_sec: Option<NonZeroU64>,
    #[serde(default, deserialize_with = "deserialize_positive_seconds")]
    setup_timeout_sec: Option<NonZeroU64>,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub src: HashMap<String, SrcConfig>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    src: HashMap<String, toml::Value>,
}

impl SrcConfig {
    pub fn timeout_sec(self) -> Option<u64> {
        self.timeout_sec.map(NonZeroU64::get)
    }

    pub fn setup_timeout_sec(self) -> Option<u64> {
        self.setup_timeout_sec.map(NonZeroU64::get)
    }
}

fn deserialize_positive_seconds<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<NonZeroU64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<i64>::deserialize(deserializer)?;
    value
        .map(|value| {
            u64::try_from(value)
                .ok()
                .and_then(NonZeroU64::new)
                .ok_or_else(|| D::Error::custom("must be a positive integer"))
        })
        .transpose()
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
        let parsed: ConfigFile = toml::from_str(&text).map_err(|error| {
            anyhow::anyhow!("invalid TOML in {}: {error}", config_path.display())
        })?;
        let mut src = HashMap::new();
        let mut seen = HashSet::new();
        for (srctype, value) in parsed.src {
            let canonical = builtin_srctype(&srctype)?;
            if !seen.insert(srctype.to_ascii_lowercase()) {
                anyhow::bail!("duplicate srctype configuration '{srctype}' ignoring case");
            }
            let config = value.try_into::<SrcConfig>().map_err(|error| {
                anyhow::anyhow!(
                    "invalid compiler configuration [src.{srctype}] in {}: {error}",
                    config_path.display()
                )
            })?;
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
    builtin_descriptor(srctype)
        .map(|known| known.defaults)
        .unwrap_or_default()
}

/// Srctype → file extension.
pub fn srctype_ext(srctype: &str) -> &str {
    builtin_descriptor(srctype)
        .map(|known| known.extension)
        .unwrap_or(srctype)
}
