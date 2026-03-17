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

    let natl = cfg.src_config("natl");
    assert_eq!(natl.depens, Some(true));
    assert_eq!(natl.reverse_depens, Some(true));

    let latex = cfg.src_config("latex");
    assert_eq!(latex.depens, Some(true));
    assert_eq!(latex.timeout_sec, Some(30));
    assert!(latex
        .preamble
        .as_deref()
        .unwrap_or("")
        .contains(r"\documentclass"));
    assert!(latex
        .postamble
        .as_deref()
        .unwrap_or("")
        .contains(r"\end{document}"));

    let py = cfg.src_config("py");
    assert_eq!(py.depens, Some(false));
    assert_eq!(py.reverse_depens, Some(false));
    assert_eq!(py.timeout_sec, Some(30));

    let lean = cfg.src_config("lean");
    assert_eq!(lean.depens, Some(true));
    assert_eq!(lean.reverse_depens, Some(false));
    assert_eq!(lean.timeout_sec, Some(300));
    assert_eq!(lean.setup_timeout_sec, Some(1800));
    assert_eq!(
        lean.imports.as_deref(),
        Some(vec!["Mathlib".to_string()].as_slice())
    );
}

#[test]
fn src_config_user_overrides_default() {
    let cfg: Config = toml::from_str("[src.latex]\ntimeout_sec = 120\n").unwrap();
    let sc = cfg.src_config("latex");
    assert_eq!(sc.timeout_sec, Some(120));
    // Built-in preamble still present (user didn't override it)
    assert!(sc.preamble.is_some());
}

#[test]
fn src_config_unknown_srctype_has_no_defaults() {
    let cfg = Config::default();
    let sc = cfg.src_config("unknown");
    assert!(sc.timeout_sec.is_none());
    assert!(sc.depens.is_none());
}
