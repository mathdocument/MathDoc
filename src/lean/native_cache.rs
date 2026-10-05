//! Guard Lake's finite builds, without pinning the entire pool for an idle LSP.
use super::cache;
use anyhow::{Context, Result};
use std::{
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn executable() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // Native library/integration tests use the sibling CLI built by Cargo.
    if exe
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "deps")
    {
        let binary = exe.parent().unwrap().parent().unwrap().join("mdc");
        anyhow::ensure!(
            binary.is_file(),
            "build mdc before running native Lean tests"
        );
        return Ok(binary);
    }
    Ok(exe)
}

pub(crate) fn run() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next()?;
    if first == "__mdc_lake_server" {
        let result = (|| -> Result<i32> {
            // Initialize the pinned toolchain without loading a possibly invalid Lakefile.
            anyhow::ensure!(
                Command::new("lake")
                    .arg("--version")
                    .stdout(Stdio::null())
                    .status()?
                    .success(),
                "cannot initialize Lake"
            );
            let located = Command::new("elan").args(["which", "lean"]).output()?;
            anyhow::ensure!(located.status.success(), "cannot locate pinned Lean");
            let lean = PathBuf::from(String::from_utf8(located.stdout)?.trim());
            let bin = lean.parent().context("Lean executable directory")?;
            let library = bin
                .parent()
                .context("Lean toolchain directory")?
                .join("lib/lean")
                .join(format!(
                    "libLake_shared.{}",
                    std::env::consts::DLL_EXTENSION
                ));
            Err(Command::new(&lean)
                .arg(format!("--load-dynlib={}", library.display()))
                .arg("--run")
                .args(args)
                .arg(executable()?)
                .env("MATHDOC_LAKE", bin.join("lake"))
                .exec()
                .into())
        })();
        return Some(match result {
            Ok(code) => code,
            Err(e) => {
                eprintln!("MathDoc Lean server: {e:#}");
                1
            }
        });
    }
    let setup = first == "setup-file" && std::env::var_os("MATHDOC_LAKE").is_some();
    if first != "__mdc_lake" && !setup {
        return None;
    }
    let mut forwarded: Vec<_> = args.collect();
    if setup {
        forwarded.insert(0, first);
    }
    let result = guarded_lake(&forwarded, setup);
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("MathDoc Lake cache: {error:#}");
            1
        }
    })
}

fn guarded_lake(args: &[std::ffi::OsString], setup: bool) -> Result<i32> {
    let root = std::env::current_dir()?;
    let pool = cache::workspace_pool(&root)?;
    let _access = pool
        .as_ref()
        .map(|pool| loop {
            match cache::try_access(pool, false) {
                Ok(lease) => break Ok::<_, anyhow::Error>(lease),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(e) => break Err(e.into()),
            }
        })
        .transpose()?;
    let program = if setup {
        std::env::var_os("MATHDOC_LAKE").context("missing native Lake executable")?
    } else {
        "lake".into()
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit());
    // A surviving native child retains the activity lock after wrapper death.
    if let Some(pool) = &pool {
        cache::inherit_access(&mut command, pool)?;
    }
    let code = if setup {
        let output = command.output()?;
        if output.status.success() {
            let mut setup: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            retain_setup(&root, &mut setup)?;
            serde_json::to_writer(std::io::stdout().lock(), &setup)?;
            std::io::stdout().write_all(b"\n")?;
        } else {
            std::io::stdout().write_all(&output.stdout)?;
        }
        output.status.code().unwrap_or(1)
    } else {
        command.status()?.code().unwrap_or(1)
    };
    if let Some(pool) = pool {
        // The server polls this small marker; native helpers have no server credentials.
        let _ = std::fs::write(pool.join("gc.pending"), []);
    }
    Ok(code)
}

fn retain_setup(root: &Path, value: &mut serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                retain_setup(root, value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                retain_setup(root, value)?;
            }
        }
        serde_json::Value::String(text) => {
            let path = root.join(&*text);
            let lake = root.join(".lake/cache");
            // Named outputs are already retained by LAKE_RESTORE_ARTIFACTS.
            // Preserve any remaining cache paths before returning them to Lean.
            let shared = std::fs::canonicalize(&lake).ok();
            if path.starts_with(lake.join("artifacts"))
                || shared
                    .as_ref()
                    .is_some_and(|shared| path.starts_with(shared.join("artifacts")))
            {
                let name = path.file_name().context("invalid native artifact path")?;
                let pins = root.join(".lake/lsp-artifacts");
                std::fs::create_dir_all(&pins)?;
                let target = pins.join(name);
                if !target.exists() {
                    match std::fs::hard_link(&path, &target) {
                        Ok(()) => (),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                        Err(_) => {
                            std::fs::copy(&path, &target)?;
                        }
                    }
                }
                *text = target.to_string_lossy().into_owned();
            }
            if path.extension().is_some_and(|e| e == "olean") {
                if let Ok(bytes) = std::fs::read(path.with_extension("trace")) {
                    if let Ok(trace) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        if let Some(key) = trace["depHash"].as_str() {
                            cache::touch(&lake, key)?;
                        }
                    }
                }
            }
        }
        _ => (),
    }
    Ok(())
}
