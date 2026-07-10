#[cfg(unix)]
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
    let mut node = MdocNode::new_at_path(root, &root.join("node.mdoc"), "Interrupt");
    node.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: format!(
            "import pathlib, signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\npathlib.Path({:?}).write_text('ready')\ntime.sleep(2)\npathlib.Path({:?}).write_text('survived')\n",
            ready.to_string_lossy(), survived.to_string_lossy()
        ),
        metadata: Default::default(),
    });
    node.save().unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(["work", &node.fnode, "--compile"])
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
