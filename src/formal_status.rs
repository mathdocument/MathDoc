use anyhow::Result;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::core::{FormalCodeStatus, FormalizationStatus};
use crate::mdocnode::MdocNode;
use crate::workspace::FileSnapshotBatch;

pub(crate) fn evaluate_node(
    root: &Path,
    relative: &Path,
    node: &MdocNode,
    snapshots: &mut FileSnapshotBatch,
) -> Result<FormalizationStatus> {
    Ok(FormalizationStatus {
        lean: evaluate_language(
            node.source_block("lean")
                .map(|block| block.content.as_bytes()),
            &lean_source_path(root, relative),
            &lean_artifact_path(root, relative),
            snapshots,
        )?,
        rocq: evaluate_language(
            node.source_block("rocq")
                .map(|block| block.content.as_bytes()),
            &rocq_source_path(root, relative),
            &rocq_artifact_path(root, relative),
            snapshots,
        )?,
    })
}

fn evaluate_language(
    block_content: Option<&[u8]>,
    source: &Path,
    artifact: &Path,
    snapshots: &mut FileSnapshotBatch,
) -> Result<FormalCodeStatus> {
    let Some(block_content) = block_content else {
        return Ok(FormalCodeStatus::NoCode);
    };
    let Some(source) = snapshots.capture_read(source)? else {
        return Ok(FormalCodeStatus::Unverified);
    };
    if source.content() != block_content {
        return Ok(FormalCodeStatus::Unverified);
    }
    let Some(artifact) = snapshots.capture_metadata(artifact)? else {
        return Ok(FormalCodeStatus::Unverified);
    };
    let source_time = (source.metadata().mtime(), source.metadata().mtime_nsec());
    let artifact_time = (artifact.mtime(), artifact.mtime_nsec());
    Ok(if artifact_time > source_time {
        FormalCodeStatus::Verified
    } else {
        FormalCodeStatus::Unverified
    })
}

pub(crate) fn lean_source_path(root: &Path, relative: &Path) -> PathBuf {
    root.join(".mdc")
        .join("lean")
        .join("Lib")
        .join(relative.with_extension("lean"))
}

pub(crate) fn lean_artifact_path(root: &Path, relative: &Path) -> PathBuf {
    root.join(".mdc")
        .join("lean")
        .join(".lake/build/lib/lean/Lib")
        .join(relative.with_extension("olean"))
}

pub(crate) fn rocq_source_path(root: &Path, relative: &Path) -> PathBuf {
    root.join(".mdc")
        .join("rocq")
        .join("Lib")
        .join(relative.with_extension("v"))
}

pub(crate) fn rocq_artifact_path(root: &Path, relative: &Path) -> PathBuf {
    root.join(".mdc")
        .join("rocq")
        .join("build")
        .join(relative.with_extension("vo"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_paths_follow_compiler_layouts() {
        let root = Path::new("/workspace");
        let source = Path::new("nested/node.mdoc");
        assert_eq!(
            lean_source_path(root, source),
            Path::new("/workspace/.mdc/lean/Lib/nested/node.lean")
        );
        assert_eq!(
            lean_artifact_path(root, source),
            Path::new("/workspace/.mdc/lean/.lake/build/lib/lean/Lib/nested/node.olean")
        );
        assert_eq!(
            rocq_source_path(root, source),
            Path::new("/workspace/.mdc/rocq/Lib/nested/node.v")
        );
        assert_eq!(
            rocq_artifact_path(root, source),
            Path::new("/workspace/.mdc/rocq/build/nested/node.vo")
        );
    }
}
