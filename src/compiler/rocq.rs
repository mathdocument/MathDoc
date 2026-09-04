use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::{
    ensure_complete_machine_output, process_error_result, require_tool, run_process, CompilerReq,
    CompilerRes, CompilerWorkspace, FormalCompilationReceipt,
};
use crate::workspace::FileSnapshot;

const INVENTORY_FILE: &str = ".mdc-module-inventory";

pub(super) fn compile(req: &CompilerReq) -> (CompilerRes, Option<FormalCompilationReceipt>) {
    let timeout_sec = match req.timeout_sec() {
        Ok(timeout) => timeout,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };

    let rocq = match require_tool("rocq") {
        Ok(p) => p,
        Err(e) => return without_receipt(CompilerRes::err_code(e.to_string(), 127)),
    };

    let workspace = match CompilerWorkspace::open(req, "rocq") {
        Ok(workspace) => workspace,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };
    let (_, relative) = match workspace.lib_source(req) {
        Ok(source) => source,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };
    let (inventory, inventory_snapshot) = match ensure_workspace(&workspace) {
        Ok(inventory) => inventory,
        Err(e) => return without_receipt(CompilerRes::err(e.to_string())),
    };
    let source = Path::new("Lib").join(relative);
    let output = Path::new("build")
        .join(source.strip_prefix("Lib").unwrap())
        .with_extension("vo");
    if let Err(error) = ensure_build_parent(&workspace, &output) {
        return without_receipt(CompilerRes::err(error.to_string()));
    }
    if let Err(error) = validate_build_output(&workspace, &output) {
        return without_receipt(CompilerRes::err(error.to_string()));
    }
    let source_path = workspace.root().join(&source);
    let source_snapshot = match workspace.snapshot(&source_path) {
        Ok(snapshot) => snapshot,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };
    let Some(source_content) = source_snapshot.content() else {
        return without_receipt(CompilerRes::err(
            "Rocq source disappeared before compilation",
        ));
    };
    let source_sha256 = crate::formal::status::content_digest(source_content);
    let compiler_identity = match crate::formal::status::capture_compiler_identity(&rocq) {
        Ok(identity) => identity,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };
    let library_roots = match rocq_library_roots(&workspace, &rocq, timeout_sec) {
        Ok(roots) => roots,
        Err(error) => return without_receipt(process_error_result(error, 1)),
    };
    let dependency_evidence =
        match rocq_dependency_evidence(&workspace, &rocq, &library_roots, &source, timeout_sec) {
            Ok(evidence) => evidence,
            Err(error) => return without_receipt(process_error_result(error, 1)),
        };
    let args = vec![
        OsStr::new("compile").to_os_string(),
        OsStr::new("-q").to_os_string(),
        OsStr::new("-Q").to_os_string(),
        OsStr::new("build").to_os_string(),
        std::ffi::OsString::new(),
        OsStr::new("-Q").to_os_string(),
        OsStr::new("Lib").to_os_string(),
        std::ffi::OsString::new(),
        OsStr::new("-noglob").to_os_string(),
        OsStr::new("-o").to_os_string(),
        output.as_os_str().to_os_string(),
        source.as_os_str().to_os_string(),
    ];
    let process_cwd = match workspace.process_cwd() {
        Ok(cwd) => cwd,
        Err(error) => return without_receipt(CompilerRes::err(error.to_string())),
    };

    match run_process(
        &rocq,
        args,
        &format!("rocq compile {}", source.display()),
        timeout_sec,
        Some(process_cwd),
    ) {
        Ok((rtcode, stdout, stderr)) => {
            if rtcode == 0 {
                if let Err(error) =
                    record_module_inventory(&workspace, &inventory, &inventory_snapshot)
                {
                    return without_receipt(CompilerRes::err(error.to_string()));
                }
            }
            let formal_receipt = if rtcode == 0 {
                match collect_formal_receipt(
                    &workspace,
                    &rocq,
                    &library_roots,
                    &source,
                    &output,
                    timeout_sec,
                    &source_snapshot,
                    source_sha256,
                    compiler_identity,
                    dependency_evidence,
                ) {
                    Ok(receipt) => Some(receipt),
                    Err(error) => return without_receipt(process_error_result(error, 1)),
                }
            } else {
                None
            };
            (
                CompilerRes {
                    stdout: stdout.trim().to_string(),
                    stderr: stderr.trim().to_string(),
                    rtcode,
                    interrupted: false,
                },
                formal_receipt,
            )
        }
        Err(e) => without_receipt(process_error_result(e, 1)),
    }
}

fn without_receipt(result: CompilerRes) -> (CompilerRes, Option<FormalCompilationReceipt>) {
    (result, None)
}

#[allow(clippy::too_many_arguments)]
fn collect_formal_receipt(
    workspace: &CompilerWorkspace,
    rocq: &Path,
    library_roots: &RocqLibraryRoots,
    source: &Path,
    output: &Path,
    timeout_sec: u64,
    source_snapshot: &crate::workspace::FileSnapshot,
    source_sha256: String,
    compiler_identity: crate::formal::status::CompilerIdentityEvidence,
    dependency_evidence: DependencyEvidence,
) -> anyhow::Result<FormalCompilationReceipt> {
    let current_dependencies =
        rocq_dependency_evidence(workspace, rocq, library_roots, source, timeout_sec)
            .context("revalidating Rocq dependencies")?;
    dependency_evidence.ensure_matches(&current_dependencies)?;
    if !workspace.snapshot_unchanged(source_snapshot, &workspace.root().join(source))? {
        anyhow::bail!("Rocq source changed during compilation");
    }
    let environment = crate::formal::status::capture_environment(workspace.mdcroot(), "rocq")
        .context("capturing Rocq compiler environment")?
        .ok_or_else(|| anyhow::anyhow!("Rocq compiler environment is incomplete"))?;
    compiler_identity.ensure_current()?;
    environment.ensure_current()?;
    let target_module = crate::formal::status::module_key(source.strip_prefix("Lib")?)?;
    Ok(FormalCompilationReceipt {
        evidence_scheme_version: crate::formal::EVIDENCE_SCHEME_VERSION,
        language: "rocq".to_string(),
        target_module,
        source_sha256,
        artifact_sha256: workspace
            .file_digest(&workspace.root().join(output))
            .context("hashing selected Rocq artifact")?,
        environment_sha256: environment.digest().to_string(),
        compiler_path: compiler_identity.path().to_string(),
        compiler_sha256: compiler_identity.digest().to_string(),
        direct_dependencies: dependency_evidence.direct_dependencies,
        external_dependencies: dependency_evidence.external_dependencies,
    })
}

struct DependencyEvidence {
    direct_dependencies: BTreeMap<String, String>,
    external_dependencies: BTreeMap<String, String>,
    guards: Vec<(PathBuf, FileSnapshot)>,
}

struct RocqLibraryRoots {
    core: PathBuf,
    user_contrib: PathBuf,
    prelude: PathBuf,
}

fn rocq_library_roots(
    workspace: &CompilerWorkspace,
    rocq: &Path,
    timeout_sec: u64,
) -> anyhow::Result<RocqLibraryRoots> {
    let (rtcode, stdout, stderr) = run_process(
        rocq,
        ["compile", "-where"],
        "rocq compile -where",
        timeout_sec,
        Some(workspace.process_cwd()?),
    )?;
    if rtcode != 0 {
        anyhow::bail!(
            "failed to locate the Rocq library: {}",
            [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    ensure_complete_machine_output(&stdout, &stderr)?;
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        anyhow::bail!("Rocq library query returned unexpected output");
    }
    let root = Path::new(lines[0])
        .canonicalize()
        .with_context(|| format!("resolving Rocq library directory {}", lines[0]))?;
    let core = root.join("theories");
    let user_contrib = root.join("user-contrib");
    let prelude = core.join("Init/Prelude.vo");
    if !core.is_dir() || !user_contrib.is_dir() || !prelude.is_file() {
        anyhow::bail!("Rocq library layout is incomplete under {}", root.display());
    }
    Ok(RocqLibraryRoots {
        core,
        user_contrib,
        prelude,
    })
}

impl DependencyEvidence {
    fn ensure_matches(&self, current: &Self) -> anyhow::Result<()> {
        if self.direct_dependencies != current.direct_dependencies
            || self.external_dependencies != current.external_dependencies
        {
            anyhow::bail!("Rocq dependencies changed during compilation");
        }
        for (path, snapshot) in &self.guards {
            if !snapshot.file_generation_unchanged(path)? {
                anyhow::bail!(
                    "Rocq dependency changed during compilation: {}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

fn rocq_dependency_evidence(
    workspace: &CompilerWorkspace,
    rocq: &Path,
    library_roots: &RocqLibraryRoots,
    source: &Path,
    timeout_sec: u64,
) -> anyhow::Result<DependencyEvidence> {
    let workspace_root = workspace.root();
    let args = vec![
        OsStr::new("dep").to_os_string(),
        OsStr::new("-dyndep").to_os_string(),
        OsStr::new("both").to_os_string(),
        OsStr::new("-Q").to_os_string(),
        library_roots.core.as_os_str().to_os_string(),
        OsStr::new("Corelib").to_os_string(),
        OsStr::new("-Q").to_os_string(),
        library_roots.user_contrib.as_os_str().to_os_string(),
        std::ffi::OsString::new(),
        OsStr::new("-Q").to_os_string(),
        OsStr::new("build").to_os_string(),
        std::ffi::OsString::new(),
        OsStr::new("-Q").to_os_string(),
        OsStr::new("Lib").to_os_string(),
        std::ffi::OsString::new(),
        OsStr::new("-noglob").to_os_string(),
        source.as_os_str().to_os_string(),
    ];
    let (rtcode, stdout, stderr) = run_process(
        rocq,
        args,
        "rocq dep",
        timeout_sec,
        Some(workspace.process_cwd()?),
    )?;
    if rtcode != 0 {
        anyhow::bail!(
            "failed to inspect Rocq dependencies: {}",
            [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    ensure_complete_machine_output(&stdout, &stderr)?;
    if stderr.contains("[module-not-found") || stderr.contains("has not been found in the loadpath")
    {
        anyhow::bail!(
            "Rocq dependency inspection left a module unresolved: {}",
            stderr.trim()
        );
    }
    let build_root = workspace_root.join("build");
    let source_root = workspace_root.join("Lib");
    let selected_source = workspace_root.join(source);
    let mut direct_dependencies = BTreeMap::new();
    let mut external_dependencies = BTreeMap::new();
    let mut guards = Vec::new();
    let prelude = library_roots.prelude.canonicalize()?;
    external_dependencies.insert(
        prelude
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Rocq Prelude path is not valid UTF-8"))?
            .to_string(),
        guarded_digest(&prelude, &mut guards)?,
    );
    for token in parse_dependency_output(&stdout)? {
        let path = PathBuf::from(token);
        let path = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        if path.extension().and_then(|value| value.to_str()) == Some("v") {
            if path != selected_source {
                anyhow::bail!(
                    "Rocq Load dependencies are unsupported; use Require for {}",
                    path.display()
                );
            }
            continue;
        }
        let (relative, artifact) = if let Ok(relative) = path.strip_prefix(&build_root) {
            (relative, path.clone())
        } else if let Ok(relative) = path.strip_prefix(&source_root) {
            (relative, build_root.join(relative))
        } else if path.is_absolute() && path.is_file() {
            let canonical = path.canonicalize().with_context(|| {
                format!("resolving Rocq dependency artifact {}", path.display())
            })?;
            let key = canonical
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Rocq dependency path is not valid UTF-8"))?
                .to_string();
            external_dependencies.insert(key, guarded_digest(&canonical, &mut guards)?);
            continue;
        } else {
            continue;
        };
        if relative.extension().and_then(|value| value.to_str()) != Some("vo") {
            continue;
        }
        let module = crate::formal::status::module_key(relative)?;
        let digest = guarded_digest(&artifact, &mut guards)?;
        if let Some(existing) = direct_dependencies.get(&module) {
            if existing != &digest {
                anyhow::bail!("Rocq dependency inspection returned ambiguous module {module}");
            }
        } else {
            direct_dependencies.insert(module, digest);
        }
    }
    Ok(DependencyEvidence {
        direct_dependencies,
        external_dependencies,
        guards,
    })
}

fn parse_dependency_output(stdout: &str) -> anyhow::Result<Vec<String>> {
    let separator = unescaped_colon(stdout)
        .ok_or_else(|| anyhow::anyhow!("Rocq dependency output has no target separator"))?;
    let (targets, dependencies) = stdout.split_at(separator);
    let dependencies = &dependencies[1..];
    if unescaped_colon(dependencies).is_some() {
        anyhow::bail!("Rocq dependency output contains multiple target rules");
    }
    if !makefile_tokens(targets)?.iter().any(|target| {
        Path::new(target)
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("vo")
    }) {
        anyhow::bail!("Rocq dependency output has no .vo target");
    }
    makefile_tokens(dependencies)
}

fn unescaped_colon(value: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ':' {
            return Some(index);
        }
    }
    None
}

fn makefile_tokens(value: &str) -> anyhow::Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('\n') => {}
                Some('\r') if characters.peek() == Some(&'\n') => {
                    characters.next();
                }
                Some(escaped) => token.push(escaped),
                None => anyhow::bail!("Rocq dependency output ends with an incomplete escape"),
            },
            whitespace if whitespace.is_whitespace() => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            character => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Ok(tokens)
}

fn guarded_digest(
    path: &Path,
    guards: &mut Vec<(PathBuf, FileSnapshot)>,
) -> anyhow::Result<String> {
    let snapshot = FileSnapshot::capture(path)?;
    let content = snapshot.content().ok_or_else(|| {
        anyhow::anyhow!("Rocq dependency artifact is missing: {}", path.display())
    })?;
    let digest = crate::formal::status::content_digest(content);
    if !snapshot.file_generation_unchanged(path)? {
        anyhow::bail!(
            "Rocq dependency artifact changed while reading: {}",
            path.display()
        );
    }
    guards.push((path.to_path_buf(), snapshot));
    Ok(digest)
}

fn ensure_workspace(
    workspace: &CompilerWorkspace,
) -> anyhow::Result<(Vec<u8>, crate::workspace::FileSnapshot)> {
    let root = workspace.root();
    let project_path = root.join("_CoqProject");
    let snapshot = workspace.snapshot(&project_path)?;
    if matches!(snapshot, crate::workspace::FileSnapshot::Missing)
        || snapshot.content() == Some(b"")
    {
        workspace.replace_generated(&project_path, &snapshot, b"-Q build \"\"\n-Q Lib \"\"\n")?;
    }
    let clean_marker = root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME);
    let clean_snapshot = workspace.snapshot(&clean_marker)?;
    let inventory = source_tree_digest(&root.join("Lib"), "v")?;
    let inventory_path = root.join(INVENTORY_FILE);
    let inventory_snapshot = workspace.snapshot(&inventory_path)?;
    let build = root.join("build");
    let build_exists = match std::fs::symlink_metadata(&build) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let inventory_changed =
        build_exists && inventory_snapshot.content() != Some(inventory.as_slice());
    if !matches!(clean_snapshot, crate::workspace::FileSnapshot::Missing) || inventory_changed {
        workspace.remove_directory_tree(&build)?;
        let _ = workspace.remove_file(&clean_marker, &clean_snapshot)?;
    }
    Ok((inventory, inventory_snapshot))
}

fn record_module_inventory(
    workspace: &CompilerWorkspace,
    expected: &[u8],
    snapshot: &crate::workspace::FileSnapshot,
) -> anyhow::Result<()> {
    let root = workspace.root();
    let current = source_tree_digest(&root.join("Lib"), "v")?;
    if current != expected {
        let marker = root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME);
        let marker_snapshot = workspace.snapshot(&marker)?;
        if marker_snapshot.content() != Some(crate::compiler::ROCQ_CLEAN_MARKER_CONTENT) {
            workspace.replace_generated(
                &marker,
                &marker_snapshot,
                crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
            )?;
        }
        anyhow::bail!("Rocq Lib source tree changed during compilation; retry the build");
    }
    let path = root.join(INVENTORY_FILE);
    if snapshot.content() != Some(expected) {
        workspace.replace_generated(&path, snapshot, expected)?;
    }
    Ok(())
}

fn ensure_build_parent(workspace: &CompilerWorkspace, output: &Path) -> anyhow::Result<()> {
    let mut current = workspace.root().to_path_buf();
    let Some(parent) = output.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid Rocq build output path");
        };
        current.push(name);
    }
    workspace.ensure_directory_tree(&current)?;
    Ok(())
}

fn validate_build_output(workspace: &CompilerWorkspace, output: &Path) -> anyhow::Result<()> {
    workspace.snapshot(&workspace.root().join(output))?;
    Ok(())
}

fn source_tree_digest(root: &Path, extension: &str) -> anyhow::Result<Vec<u8>> {
    let mut files = Vec::new();
    if !root.is_dir() {
        return Ok(format!("{:x}\n", Sha256::digest([])).into_bytes());
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "refusing symlink in compiler source tree: {}",
                entry.path().display()
            );
        }
        if !entry.file_type().is_file() || entry.path().extension() != Some(OsStr::new(extension)) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("walk entry belongs to source root");
        files.push((
            relative.as_os_str().as_bytes().to_vec(),
            entry.path().to_path_buf(),
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(&relative);
        let content = std::fs::read(&path)
            .with_context(|| format!("reading compiler source {}", path.display()))?;
        digest.update((content.len() as u64).to_le_bytes());
        digest.update(content);
    }
    Ok(format!("{:x}\n", digest.finalize()).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_workspace(directory: &tempfile::TempDir) -> CompilerWorkspace {
        let mdcroot = directory.path().canonicalize().unwrap();
        let root = mdcroot.join(".mdc/rocq");
        std::fs::create_dir_all(&root).unwrap();
        let root_generation =
            crate::workspace::DirectoryGeneration::open_beneath(&mdcroot, &root).unwrap();
        CompilerWorkspace {
            mdcroot,
            root,
            srctype: "rocq".to_string(),
            root_generation,
        }
    }

    #[test]
    fn dependency_output_requires_a_target_rule() {
        assert!(parse_dependency_output("malformed output\n").is_err());
        assert!(parse_dependency_output("target.glob: Lib/Target.v\n").is_err());
        assert!(parse_dependency_output("Target.vo:\nOther.vo: Other.v\n").is_err());
        assert_eq!(
            parse_dependency_output("Lib/Target.vo: Lib/Target.v \\\n build/Dependency.vo\n")
                .unwrap(),
            ["Lib/Target.v", "build/Dependency.vo"]
        );
        assert_eq!(
            parse_dependency_output("Lib/Target.vo: Lib/Target.v /Rocq\\ Platform/Init/Logic.vo\n")
                .unwrap(),
            ["Lib/Target.v", "/Rocq Platform/Init/Logic.vo"]
        );
    }

    #[test]
    fn rejects_linked_build_outputs_before_compilation() {
        use std::os::unix::fs::symlink;

        for hard_link in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(outside.path(), "outside").unwrap();
            let workspace = test_workspace(&directory);
            let output = Path::new("build/Target.vo");
            std::fs::create_dir_all(workspace.root().join("build")).unwrap();
            if hard_link {
                std::fs::hard_link(outside.path(), workspace.root().join(output)).unwrap();
            } else {
                symlink(outside.path(), workspace.root().join(output)).unwrap();
            }

            assert!(validate_build_output(&workspace, output).is_err());
            assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "outside");
        }
    }

    #[test]
    fn clean_marker_removes_stale_build_artifacts() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(root.join("build/Data/Stale.vo"), "stale").unwrap();
        std::fs::write(
            root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

        assert!(!root.join("build").exists());
        assert!(!root
            .join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME)
            .exists());
    }

    #[test]
    fn inventory_change_removes_stale_build_artifacts_without_sync_marker() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("Lib")).unwrap();
        std::fs::write(root.join("Lib/Data.v"), "Definition value := 1.\n").unwrap();
        let inventory = source_tree_digest(&root.join("Lib"), "v").unwrap();
        std::fs::write(root.join(INVENTORY_FILE), inventory).unwrap();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(root.join("build/Data/Stale.vo"), "stale").unwrap();
        std::fs::write(root.join("Lib/Data.v"), "Definition value := 2.\n").unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

        assert!(!root.join("build").exists());
        assert!(!root
            .join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME)
            .exists());
    }

    #[test]
    fn rejects_dangling_coqproject_symlink_without_creating_target() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        let target = outside.path().join("created-outside");
        symlink(&target, root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(!target.exists());
        assert!(std::fs::symlink_metadata(root.join("_CoqProject"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn rejects_coqproject_symlink_to_existing_file() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "outside").unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        symlink(outside.path(), root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "outside");
    }

    #[test]
    fn cleanup_rejects_symlinked_build_tree() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::write(outside.path().join("keep.vo"), "outside").unwrap();
        symlink(outside.path(), root.join("build")).unwrap();
        std::fs::write(
            root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();

        assert!(error.to_string().contains("non-directory tree"));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.vo")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn cleanup_rejects_regular_file_build_path() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::write(root.join("build"), "not a directory").unwrap();
        std::fs::write(
            root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();

        assert!(error.to_string().contains("non-directory tree"));
        assert_eq!(
            std::fs::read_to_string(root.join("build")).unwrap(),
            "not a directory"
        );
    }

    #[test]
    fn cleanup_unlinks_nested_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(outside.path().join("keep.vo"), "outside").unwrap();
        symlink(outside.path(), root.join("build/Data/link")).unwrap();
        std::fs::write(
            root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

        assert!(!root.join("build").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.vo")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn cleanup_cannot_follow_replaced_workspace_ancestor() {
        use std::os::unix::fs::symlink;

        for point in [
            crate::workspace::TestHookPoint::RemoveTreeBeforeDirectoryBinding,
            crate::workspace::TestHookPoint::RemoveTreeAfterDirectoryBinding,
        ] {
            let workspace = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let compiler_workspace = test_workspace(&workspace);
            let root = compiler_workspace.root().to_path_buf();
            std::fs::create_dir_all(root.join("build/Data")).unwrap();
            std::fs::write(root.join("build/Data/stale.vo"), "stale").unwrap();
            std::fs::write(
                root.join(crate::formal::ROCQ_CLEAN_MARKER_FILENAME),
                crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
            )
            .unwrap();
            std::fs::create_dir_all(outside.path().join("build/Data")).unwrap();
            std::fs::write(outside.path().join("build/Data/outside.vo"), "outside").unwrap();
            let displaced = compiler_workspace.mdcroot.join(".mdc/displaced-rocq");
            let hook_root = root.clone();
            let hook_displaced = displaced.clone();
            let hook_outside = outside.path().to_path_buf();
            crate::workspace::set_test_hook(point, move || {
                std::fs::rename(hook_root, hook_displaced).unwrap();
                symlink(hook_outside, root).unwrap();
            });

            let error = ensure_workspace(&compiler_workspace).unwrap_err();

            assert!(crate::workspace::error_has_file_conflict(&error));
            assert_eq!(
                std::fs::read_to_string(outside.path().join("build/Data/outside.vo")).unwrap(),
                "outside",
                "outside tree changed at {point:?}"
            );
            assert_eq!(
                std::fs::read_to_string(displaced.join("build/Data/stale.vo")).unwrap(),
                "stale",
                "displaced tree changed at {point:?}"
            );
        }
    }
}
