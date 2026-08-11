use std::path::Path;
use std::process::{Command, Output};

fn run_mdc(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn write(root: &Path, relative: &str, content: &str) {
    std::fs::write(root.join(relative), content).unwrap();
}

#[test]
fn ior_uses_valid_source_degrees_and_prints_only_the_value() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    write(
        root,
        "focus.mdoc",
        "@fnode: target-metric-node\n@title: Focus\n\n@dep:\nleaf-node\nmissing-node\n@end\n",
    );
    write(root, "leaf.mdoc", "@fnode: leaf-node\n@title: Leaf\n");
    write(
        root,
        "referrer.mdoc",
        "@fnode: referrer-node\n@title: Referrer\n\n@dep:\ntarget-metric-node\n@end\n",
    );
    for relative in ["duplicate-a.mdoc", "duplicate-b.mdoc"] {
        write(
            root,
            relative,
            "@fnode: duplicate-referrer\n@title: Duplicate Referrer\n\n@dep:\ntarget-metric-node\n@end\n",
        );
    }

    let expected = 2.0f64.ln() - 3.0f64.ln();
    for source in ["target-metric-node", "target-m", "focus.mdoc"] {
        let output = run_mdc(root, &["metric", "ior", source]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: f64 = stdout.trim().parse().unwrap();
        let difference = (value - expected).abs();
        assert!(
            difference < 1e-12,
            "IOR mismatch: value={value}, expected={expected}, difference={difference}"
        );
        assert_eq!(stdout.lines().count(), 1);
    }

    let profiled = run_mdc(root, &["metric", "ior", "target-m", "--prof"]);
    assert!(profiled.status.success());
    assert_eq!(
        String::from_utf8(profiled.stdout).unwrap().lines().count(),
        1
    );
    let stderr = String::from_utf8(profiled.stderr).unwrap();
    assert!(stderr.contains("profile (inclusive elapsed):"));
    assert!(stderr.contains("IndCache::open_refreshed"));
    assert!(stderr.contains("IndCache::refresh_all"));
    assert!(stderr.contains("refresh::scan_workspace"));
}

#[test]
fn ior_rejects_a_structurally_invalid_source() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    write(
        root,
        "invalid.mdoc",
        "@fnode: invalid-node\n@title: Invalid\n@unknown: value\n",
    );

    let output = run_mdc(root, &["metric", "ior", "invalid-node"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("metric source must be one valid, uniquely indexed node"));
}
