use std::fs;

use tempfile::TempDir;

use mathdoc::config::Config;

#[test]
fn config_template_lists_builtin_defaults() {
    let template = mathdoc::config::config_template();

    for srctype in mathdoc::config::builtin_srctypes() {
        assert!(template.contains(&format!("# [src.{srctype}]")));
    }
    assert!(template.contains("# timeout_sec = 30"));
    assert!(template.contains("# setup_timeout_sec = 1800"));
}

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
    assert_eq!(cfg.src.get("latex").unwrap().timeout_sec(), Some(60));
}

#[test]
fn load_config_canonicalizes_known_srctype_case() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(mdc.join("config.toml"), "[src.Python]\ntimeout_sec = 60\n").unwrap();

    let cfg = Config::load(dir.path()).unwrap();

    assert!(cfg.src.contains_key("python"));
    assert!(!cfg.src.contains_key("Python"));
}

#[test]
fn load_config_rejects_case_only_duplicate_srctypes() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(
        mdc.join("config.toml"),
        "[src.Python]\ntimeout_sec = 60\n\n[src.python]\ntimeout_sec = 30\n",
    )
    .unwrap();

    assert!(Config::load(dir.path()).is_err());
}

#[test]
fn load_config_rejects_unknown_srctype() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(
        mdc.join("config.toml"),
        "[src.markdown]\ntimeout_sec = 60\n",
    )
    .unwrap();

    let error = Config::load(dir.path()).unwrap_err().to_string();
    assert!(error.contains("unsupported srctype 'markdown'"));
}

#[test]
fn src_config_returns_defaults_for_known_srctypes() {
    let cfg = Config::default();

    let text = cfg.src_config("text");
    assert_eq!(text.timeout_sec(), None);

    let latex = cfg.src_config("latex");
    assert_eq!(latex.timeout_sec(), Some(30));

    let python = cfg.src_config("python");
    assert_eq!(python.timeout_sec(), Some(30));

    let lean = cfg.src_config("lean");
    assert_eq!(lean.timeout_sec(), Some(300));
    assert_eq!(lean.setup_timeout_sec(), Some(1800));

    let rocq = cfg.src_config("rocq");
    assert_eq!(rocq.timeout_sec(), Some(300));
    assert_eq!(rocq.setup_timeout_sec(), None);
}

#[test]
fn src_config_user_overrides_default() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(mdc.join("config.toml"), "[src.latex]\ntimeout_sec = 120\n").unwrap();
    let cfg = Config::load(dir.path()).unwrap();
    let sc = cfg.src_config("latex");
    assert_eq!(sc.timeout_sec(), Some(120));
}

#[test]
fn src_config_unknown_srctype_has_no_defaults() {
    let cfg = Config::default();
    let sc = cfg.src_config("unknown");
    assert!(sc.timeout_sec().is_none());
}

#[test]
fn load_config_rejects_non_positive_timeout_with_source_key() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(mdc.join("config.toml"), "[src.python]\ntimeout_sec = 0\n").unwrap();

    let error = Config::load(dir.path()).unwrap_err().to_string();
    assert!(error.contains("[src.python]"), "{error}");
    assert!(error.contains("positive integer"), "{error}");
}

#[test]
fn load_config_rejects_non_positive_setup_timeout_with_source_key() {
    let dir = TempDir::new().unwrap();
    let mdc = dir.path().join(".mdc");
    fs::create_dir(&mdc).unwrap();
    fs::write(
        mdc.join("config.toml"),
        "[src.lean]\nsetup_timeout_sec = -1\n",
    )
    .unwrap();

    let error = Config::load(dir.path()).unwrap_err().to_string();
    assert!(error.contains("[src.lean]"), "{error}");
    assert!(error.contains("positive integer"), "{error}");
}

#[test]
fn load_config_rejects_unknown_and_inapplicable_settings() {
    for (section, setting, expected) in [
        ("python", "typo_timeout_sec = 1", "unknown field"),
        ("text", "timeout_sec = 1", "does not support timeout_sec"),
        (
            "python",
            "setup_timeout_sec = 1",
            "does not support setup_timeout_sec",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let mdc = dir.path().join(".mdc");
        fs::create_dir(&mdc).unwrap();
        fs::write(
            mdc.join("config.toml"),
            format!("[src.{section}]\n{setting}\n"),
        )
        .unwrap();

        let error = Config::load(dir.path()).unwrap_err().to_string();

        assert!(error.contains(&format!("[src.{section}]")), "{error}");
        assert!(error.contains(expected), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn load_config_rejects_non_regular_and_symlinked_files() {
    use std::os::unix::fs::symlink;

    let directory_config = TempDir::new().unwrap();
    fs::create_dir(directory_config.path().join(".mdc")).unwrap();
    fs::create_dir(directory_config.path().join(".mdc/config.toml")).unwrap();
    assert!(Config::load(directory_config.path()).is_err());

    let symlinked_config = TempDir::new().unwrap();
    fs::create_dir(symlinked_config.path().join(".mdc")).unwrap();
    let external = TempDir::new().unwrap();
    let target = external.path().join("config.toml");
    fs::write(&target, "[src.python]\ntimeout_sec = 1\n").unwrap();
    symlink(&target, symlinked_config.path().join(".mdc/config.toml")).unwrap();

    assert!(Config::load(symlinked_config.path()).is_err());
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "[src.python]\ntimeout_sec = 1\n"
    );
}
