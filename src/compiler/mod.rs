mod latex;
mod lean;
mod python;
mod text;

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct CompilerReq {
    pub mdcroot: PathBuf,
    pub srctype: String,
    pub content: String,
    pub preamble: String,
    pub postamble: String,
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
            Box::new(text::CompilerText) as Box<dyn SrcCompiler>,
            Box::new(python::CompilerPython),
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

fn cfg_positive_int(
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

fn require_tool(name: &str) -> Result<String> {
    which::which(name)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| anyhow::anyhow!("{name} not found in PATH"))
}

/// Run a subprocess and wait, with a polling timeout. Returns `(rtcode, stdout, stderr)`.
///
/// stdout and stderr are drained in background threads immediately after spawn.
/// Without this, a child that writes more than the OS pipe buffer (~64 KB) would
/// block on its write end while the parent polls `try_wait()`, causing a deadlock
/// that looks like a timeout even when the child is not actually slow.
fn run_process(
    args: &[&str],
    tool_name: &str,
    timeout_sec: u64,
    cwd: Option<&Path>,
) -> Result<(i32, String, String)> {
    use std::process::Stdio;
    use std::thread;

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

    // Take the pipes before entering the wait loop so the drain threads hold
    // the only read ends; the child can write freely without blocking.
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stdout_thread = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_sec);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Join drain threads to avoid leaking OS resources.
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    bail!("{tool_name} timed out after {timeout_sec} seconds");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(e.into());
            }
        }
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a subprocess that writes more than the OS pipe buffer (~64 KB) must
    /// not be reported as a timeout. Previously stdout/stderr were read only *after*
    /// try_wait() returned — so a large-output child blocked on write while the parent
    /// spun until the deadline, producing a spurious "timed out" error.
    ///
    /// Fix: both pipes are drained in background threads immediately after spawn.
    #[cfg(unix)]
    #[test]
    fn test_large_output_does_not_false_timeout() {
        if which::which("python3").is_err() {
            return; // skip if python3 is unavailable in this environment
        }
        // 2 MB stdout — well beyond the typical 64 KB pipe buffer.
        let (code, stdout, _stderr) = run_process(
            &[
                "python3",
                "-c",
                "import sys; sys.stdout.write('x' * 2_000_000)",
            ],
            "python3",
            10,
            None,
        )
        .expect("large output must not be misreported as timeout");
        assert_eq!(code, 0);
        assert_eq!(stdout.len(), 2_000_000, "all output must be captured");
    }

    /// Regression: a genuinely slow process must still be killed and reported as timed out.
    #[cfg(unix)]
    #[test]
    fn test_real_timeout_is_reported() {
        let (cmd, name) = if which::which("sleep").is_ok() {
            (vec!["sleep", "60"], "sleep")
        } else {
            return; // skip if no suitable blocking command
        };
        let result = run_process(&cmd, name, 1, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("timed out after"),
            "timed-out process must produce a timed-out error"
        );
    }
}

fn is_timeout_error(e: &anyhow::Error) -> bool {
    e.to_string().contains("timed out after")
}

fn emit_progress(progress: &Option<Box<dyn Fn(&str)>>, msg: &str) {
    if let Some(p) = progress {
        p(msg);
    }
}
