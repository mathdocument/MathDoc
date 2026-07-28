use anyhow::{bail, Result};
use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::config::srctype_ext;
use crate::workspace::{ensure_regular_directory, ensure_regular_directory_exists};

pub(super) struct MirrorState<'a> {
    pub(super) content: Cow<'a, [u8]>,
    pub(super) present: bool,
}

pub(super) fn back_state(raw: Option<&[u8]>, baseline_present: bool) -> MirrorState<'_> {
    match raw {
        None => MirrorState {
            content: Cow::Borrowed(&[]),
            present: false,
        },
        Some(content) if content.is_empty() || content.ends_with(b"\n") => MirrorState {
            content: Cow::Borrowed(content),
            present: baseline_present || !content.is_empty(),
        },
        Some(content) => {
            let mut normalized = Vec::with_capacity(content.len() + 1);
            normalized.extend_from_slice(content);
            normalized.push(b'\n');
            MirrorState {
                content: Cow::Owned(normalized),
                present: true,
            }
        }
    }
}

pub(super) fn validate_source_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.extension() != Some(OsStr::new("mdoc"))
    {
        bail!("invalid source path in source block manifest");
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("invalid source path in source block manifest");
        };
        if name == ".mdc" {
            bail!("source block manifest path cannot enter .mdc");
        }
    }
    Ok(())
}

pub(super) fn output_path(mdcroot: &Path, source: &Path, srctype: &str) -> PathBuf {
    mdcroot
        .join(".mdc")
        .join(srctype)
        .join("Lib")
        .join(output_relative(source, srctype))
}

pub(super) fn prepare_output_path(mdcroot: &Path, source: &Path, srctype: &str) -> Result<PathBuf> {
    let relative = output_relative(source, srctype);
    let mut parent = mdcroot.join(".mdc");
    ensure_regular_directory(&parent)?;
    parent.push(srctype);
    ensure_regular_directory_exists(&parent)?;
    parent.push("Lib");
    ensure_regular_directory_exists(&parent)?;
    if let Some(relative_parent) = relative.parent() {
        for component in relative_parent.components() {
            let Component::Normal(name) = component else {
                bail!("invalid source block output path");
            };
            parent.push(name);
            ensure_regular_directory_exists(&parent)?;
        }
    }
    let file_name = relative
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid source block output path"))?;
    Ok(parent.join(file_name))
}

pub(super) fn existing_output_path(
    mdcroot: &Path,
    source: &Path,
    srctype: &str,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let relative = output_relative(source, srctype);
    let mut parent = mdcroot.join(".mdc");
    ensure_regular_directory(&parent)?;
    parent.push(srctype);
    if !existing_regular_directory(&parent)? {
        return Ok(None);
    }
    parent.push("Lib");
    let type_root = parent.clone();
    if !existing_regular_directory(&parent)? {
        return Ok(None);
    }
    if let Some(relative_parent) = relative.parent() {
        for component in relative_parent.components() {
            let Component::Normal(name) = component else {
                bail!("invalid source block output path");
            };
            parent.push(name);
            if !existing_regular_directory(&parent)? {
                return Ok(None);
            }
        }
    }
    let file_name = relative
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid source block output path"))?;
    Ok(Some((parent.join(file_name), type_root)))
}

pub(super) fn remove_empty_parents(path: &Path, type_root: &Path) {
    let Some(mut parent) = path.parent() else {
        return;
    };
    while parent != type_root && parent.starts_with(type_root) {
        match std::fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
        let Some(next) = parent.parent() else {
            break;
        };
        parent = next;
    }
}

fn output_relative(source: &Path, srctype: &str) -> PathBuf {
    source.with_extension(srctype_ext(srctype))
}

fn existing_regular_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_regular_directory(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
