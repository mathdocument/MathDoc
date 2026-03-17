//! Block compilation: request/response types, compiler trait, registry, and implementations.

mod latex;
mod lean;
mod natl;
mod py;

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct CompilerReq {
    pub mdcroot: PathBuf,
    pub srctype: String,
    pub content: String,
    /// Config values from `.mdc/config.toml` `[src.<srctype>]` section.
    pub compcfg: HashMap<String, toml::Value>,
    pub progress: Option<Box<dyn Fn(&str)>>,
}

pub struct CompilerRes {
    pub result: bool,
    pub stdout: String,
    pub stderr: String,
    pub rtcode: i32,
}

impl CompilerRes {
    pub fn err(stderr: impl Into<String>) -> Self {
        CompilerRes {
            result: false,
            stdout: String::new(),
            stderr: stderr.into(),
            rtcode: 1,
        }
    }
    pub fn err_code(stderr: impl Into<String>, rtcode: i32) -> Self {
        CompilerRes {
            result: false,
            stdout: String::new(),
            stderr: stderr.into(),
            rtcode,
        }
    }
    pub fn ok(stdout: impl Into<String>) -> Self {
        CompilerRes {
            result: true,
            stdout: stdout.into(),
            stderr: String::new(),
            rtcode: 0,
        }
    }
}

/// One compiled block, as returned by `DepGraph::eval_blocks`.
pub struct BlockResult {
    pub node_fnode: String,
    pub srctype: String,
    pub res: CompilerRes,
}

pub trait SrcCompiler: Send + Sync {
    fn srctype(&self) -> &str;
    fn compile(&self, req: &CompilerReq) -> CompilerRes;
}

// ── Registry ──────────────────────────────────────────────────────────────────

pub struct CompilerRegistry {
    compilers: HashMap<String, Box<dyn SrcCompiler>>,
}

impl CompilerRegistry {
    pub fn default_registry() -> Self {
        let mut m: HashMap<String, Box<dyn SrcCompiler>> = HashMap::new();
        for c in [
            Box::new(natl::CompilerNatl) as Box<dyn SrcCompiler>,
            Box::new(py::CompilerPy),
            Box::new(latex::CompilerLatex),
            Box::new(lean::CompilerLean),
        ] {
            m.insert(c.srctype().to_ascii_lowercase(), c);
        }
        CompilerRegistry { compilers: m }
    }

    pub fn resolve(&self, srctype: &str) -> Option<&dyn SrcCompiler> {
        self.compilers
            .get(&srctype.to_ascii_lowercase())
            .map(|b| b.as_ref())
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub(crate) fn cfg_positive_int(
    compcfg: &HashMap<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<u64> {
    match compcfg.get(key) {
        None => bail!("config key '{full_key}' is required"),
        Some(v) => match v.as_integer() {
            Some(n) if n > 0 => Ok(n as u64),
            _ => bail!("config key '{full_key}' must be a positive integer"),
        },
    }
}

pub(crate) fn cfg_str(
    compcfg: &HashMap<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<String> {
    match compcfg.get(key) {
        None => bail!("config key '{full_key}' is required"),
        Some(v) => match v.as_str() {
            Some(s) => Ok(s.to_string()),
            None => bail!("config key '{full_key}' must be a string"),
        },
    }
}

pub(crate) fn cfg_str_opt(
    compcfg: &HashMap<String, toml::Value>,
    key: &str,
    full_key: &str,
) -> Result<String> {
    match compcfg.get(key) {
        None => Ok(String::new()),
        Some(v) => match v.as_str() {
            Some(s) => Ok(s.to_string()),
            None => bail!("config key '{full_key}' must be a string"),
        },
    }
}

pub(crate) fn require_tool(name: &str) -> Result<String> {
    which::which(name)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| anyhow::anyhow!("{name} not found in PATH"))
}

/// Run a subprocess and wait, with a polling timeout. Returns `(rtcode, stdout, stderr)`.
pub(crate) fn run_process(
    args: &[&str],
    tool_name: &str,
    timeout_sec: u64,
    cwd: Option<&Path>,
) -> Result<(i32, String, String)> {
    use std::process::Stdio;

    let mut cmd = std::process::Command::new(args[0]);
    cmd.args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to run {tool_name}: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(timeout_sec);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                let rtcode = status.code().unwrap_or(-1);
                return Ok((rtcode, stdout, stderr));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("{tool_name} timed out after {timeout_sec} seconds");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub(crate) fn is_timeout_error(e: &anyhow::Error) -> bool {
    e.to_string().contains("timed out after")
}

pub(crate) fn emit_progress(progress: &Option<Box<dyn Fn(&str)>>, msg: &str) {
    if let Some(p) = progress {
        p(msg);
    }
}
