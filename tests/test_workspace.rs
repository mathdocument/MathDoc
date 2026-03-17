use std::fs;
use std::path::Path;

use tempfile::TempDir;

use mathdoc::workspace::{find_mdcroot, find_nested_mdcroot, iter_mdoc_files, to_rel_path};

fn setup_workspace(dir: &TempDir) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    fs::create_dir(root.join(".mdc")).unwrap();
    root
}

#[test]
fn find_mdcroot_from_child() {
    let dir = TempDir::new().unwrap();
    let root = setup_workspace(&dir);
    let subdir = root.join("sub");
    fs::create_dir(&subdir).unwrap();
    assert_eq!(find_mdcroot(&subdir).unwrap(), root);
}

#[test]
fn find_mdcroot_not_found() {
    let dir = TempDir::new().unwrap();
    assert!(find_mdcroot(dir.path()).is_none());
}

#[test]
fn find_nested_mdcroot_detects_nested() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let nested = root.join("nested");
    fs::create_dir_all(nested.join(".mdc")).unwrap();
    assert_eq!(find_nested_mdcroot(root, &nested), Some(nested));
}

#[test]
fn find_nested_mdcroot_none_when_root() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    assert!(find_nested_mdcroot(root, root).is_none());
}

#[test]
fn find_nested_mdcroot_none_when_outside_root() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("other");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    assert!(find_nested_mdcroot(&root, &outside).is_none());
}

#[test]
fn iter_mdoc_files_basic() {
    let dir = TempDir::new().unwrap();
    let root = setup_workspace(&dir);
    fs::write(root.join("a.mdoc"), "").unwrap();
    fs::write(root.join("b.txt"), "").unwrap();
    let sub = root.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("c.mdoc"), "").unwrap();

    let mut files: Vec<_> = iter_mdoc_files(&root).collect();
    files.sort();
    assert_eq!(files.len(), 2);
    assert!(files[0].ends_with("a.mdoc"));
    assert!(files[1].ends_with("c.mdoc"));
}

#[test]
fn iter_mdoc_files_skips_nested_root() {
    let dir = TempDir::new().unwrap();
    let root = setup_workspace(&dir);
    fs::write(root.join("a.mdoc"), "").unwrap();
    let nested = root.join("nested");
    fs::create_dir_all(nested.join(".mdc")).unwrap();
    fs::write(nested.join("b.mdoc"), "").unwrap();

    let files: Vec<_> = iter_mdoc_files(&root).collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.mdoc"));
}

#[test]
fn to_rel_path_basic() {
    let root = Path::new("/workspace");
    let path = Path::new("/workspace/sub/file.mdoc");
    assert_eq!(to_rel_path(root, path), "sub/file.mdoc");
}

#[test]
fn to_rel_path_root_file() {
    let root = Path::new("/workspace");
    let path = Path::new("/workspace/file.mdoc");
    assert_eq!(to_rel_path(root, path), "file.mdoc");
}
