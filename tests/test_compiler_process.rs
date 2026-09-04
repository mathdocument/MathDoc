#[test]
fn ctrl_c_escalates_and_returns_signal_exit_code() {
    use mathdoc::mdocnode::{MdocNode, SrcBlock};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let survived = root.join("compiler-survived");
    let ready = root.join("compiler-ready");
    let mut node = MdocNode::new_at_path(&root.join("node.mdoc"), "Interrupt");
    node.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: format!(
            "import pathlib, signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\npathlib.Path({:?}).write_text('ready')\ntime.sleep(2)\npathlib.Path({:?}).write_text('survived')\n",
            ready.to_string_lossy(), survived.to_string_lossy()
        ),
        metadata: Default::default(),
    });
    std::fs::write(&node.path, node.render().unwrap()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(["work", &node.fnode])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "mdc did not reach compiler startup");

    // SAFETY: child.id() is the live mdc subprocess owned by this test.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) };
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));

    std::thread::sleep(Duration::from_millis(2200));
    assert!(!survived.exists(), "compiler survived Ctrl-C escalation");
}

#[test]
fn work_releases_node_mutation_lock_while_compiler_runs() {
    use mathdoc::mdocnode::{MdocNode, SrcBlock};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let ready = root.join("compiler-ready");
    let mut node = MdocNode::new_at_path(&root.join("node.mdoc"), "Long compile");
    node.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: format!(
            "import pathlib, time\npathlib.Path({:?}).write_text('ready')\ntime.sleep(10)\n",
            ready.to_string_lossy()
        ),
        metadata: Default::default(),
    });
    std::fs::write(&node.path, node.render().unwrap()).unwrap();

    let mut work = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(["work", &node.fnode])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < ready_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !ready.exists() {
        let status = work.try_wait().unwrap();
        let _ = work.kill();
        let _ = work.wait();
        panic!("mdc did not reach compiler startup; status={status:?}");
    }

    let mut mutation = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(["new", "--title", "Concurrent", "--file", "concurrent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mutation_deadline = Instant::now() + Duration::from_secs(2);
    let mutation_status = loop {
        if let Some(status) = mutation.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= mutation_deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let work_was_running = work.try_wait().unwrap().is_none();

    let _ = work.kill();
    let _ = work.wait();
    if mutation_status.is_none() {
        let _ = mutation.kill();
        let _ = mutation.wait();
    }

    let mutation_status = mutation_status.expect("node mutation waited for compiler completion");
    assert!(mutation_status.success());
    assert!(
        work_was_running,
        "compiler exited before concurrent mutation"
    );
    assert!(root.join("concurrent.mdoc").is_file());
}

fn python_workspace() -> (tempfile::TempDir, String) {
    use mathdoc::mdocnode::{MdocNode, SrcBlock};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let mut node = MdocNode::new_at_path(&root.join("node.mdoc"), "Exit Status");
    node.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: "print('compile')\n".to_string(),
        metadata: Default::default(),
    });
    let fnode = node.fnode.clone();
    std::fs::write(&node.path, node.render().unwrap()).unwrap();
    (dir, fnode)
}

#[test]
fn tool_missing_returns_127_from_cli() {
    use std::process::Command;

    let (dir, fnode) = python_workspace();
    let empty_path = dir.path().join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(dir.path())
        .env("PATH", &empty_path)
        .args(["work", &fnode])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(127));
}

#[test]
fn compiler_timeout_returns_124_from_cli() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let (dir, fnode) = python_workspace();
    std::fs::write(
        dir.path().join(".mdc/config.toml"),
        "[src.python]\ntimeout_sec = 1\n",
    )
    .unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let python = bin.join("python3");
    std::fs::write(&python, "#!/bin/sh\n/bin/sleep 10\n").unwrap();
    let mut permissions = std::fs::metadata(&python).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&python, permissions).unwrap();

    let started = Instant::now();
    let status = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(dir.path())
        .env("PATH", &bin)
        .args(["work", &fnode])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(124));
    assert!(started.elapsed() < Duration::from_secs(5));
}
