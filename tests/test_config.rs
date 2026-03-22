use std::fs;

use tempfile::TempDir;

use mathdoc::config::Config;

#[test]
fn load_missing_config() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".mdc")).unwrap();
    let cfg = Config::load(dir.path()).unwrap();
    assert!(cfg.src.is_empty());
}

#[test]
fn load_config_with_srctype() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(mdc.join("config.toml"), "[src.latex]\ntimeout_sec = 60\n").unwrap();
    let cfg = Config::load(dir.path()).unwrap();
    assert_eq!(cfg.src.get("latex").unwrap().timeout_sec, Some(60));
}

#[test]
fn src_config_returns_defaults_for_known_srctypes() {
    let cfg = Config::default();

    let text = cfg.src_config("text");
    assert_eq!(text.depens, Some(true));
    assert_eq!(text.reverse_depens, Some(true));

    let latex = cfg.src_config("latex");
    assert_eq!(latex.depens, Some(true));
    assert_eq!(latex.timeout_sec, Some(30));

    let python = cfg.src_config("python");
    assert_eq!(python.depens, Some(false));
    assert_eq!(python.reverse_depens, Some(false));
    assert_eq!(python.timeout_sec, Some(30));

    let lean = cfg.src_config("lean");
    assert_eq!(lean.depens, Some(true));
    assert_eq!(lean.reverse_depens, Some(false));
    assert_eq!(lean.timeout_sec, Some(300));
    assert_eq!(lean.setup_timeout_sec, Some(1800));

    let rocq = cfg.src_config("rocq");
    assert_eq!(rocq.depens, Some(true));
    assert_eq!(rocq.reverse_depens, Some(false));
    assert_eq!(rocq.timeout_sec, Some(300));
    assert_eq!(rocq.setup_timeout_sec, Some(1800));
}

#[test]
fn src_config_user_overrides_default() {
    let cfg: Config = toml::from_str("[src.latex]\ntimeout_sec = 120\n").unwrap();
    let sc = cfg.src_config("latex");
    assert_eq!(sc.timeout_sec, Some(120));
    // depens still gets built-in default
    assert_eq!(sc.depens, Some(true));
}

#[test]
fn src_config_unknown_srctype_has_no_defaults() {
    let cfg = Config::default();
    let sc = cfg.src_config("unknown");
    assert!(sc.timeout_sec.is_none());
    assert!(sc.depens.is_none());
}

#[test]
fn preamble_postamble_defaults_from_compiler() {
    use mathdoc::config::{default_postamble, default_preamble};

    assert!(default_preamble("latex").contains(r"\documentclass"));
    assert!(default_postamble("latex").contains(r"\end{document}"));
    assert!(default_preamble("python").is_empty());
    assert!(default_postamble("python").is_empty());
}

#[test]
fn preamble_postamble_file_roundtrip() {
    use mathdoc::config::{
        default_preamble, read_postamble, read_preamble, write_postamble, write_preamble,
    };

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".mdc")).unwrap();

    // Before any file exists, defaults are returned.
    assert!(read_preamble(root, "latex").contains(r"\documentclass"));

    // Write custom preamble.
    write_preamble(root, "latex", "\\documentclass{book}\n\\begin{document}\n").unwrap();
    let pre = read_preamble(root, "latex");
    assert!(pre.contains("book"));
    assert!(!pre.contains(default_preamble("latex")));

    // Write custom postamble.
    write_postamble(root, "latex", "\\end{document}\n").unwrap();
    let post = read_postamble(root, "latex");
    assert!(post.contains(r"\end{document}"));
}
