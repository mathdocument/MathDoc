use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use crate::compiler::FormalCompilationReceipt;
use crate::core::FormalCodeStatus;
use crate::mdocnode::MdocNode;
use crate::workspace::{FileSnapshot, FileSnapshotBatch, ReadFileSnapshot};

use super::attestation::{
    self as formal_attestation, FormalAttestation, FormalAttestationManifest,
};

pub(crate) const FORMAL_LANGUAGES: [&str; 2] = ["lean", "rocq"];

struct IndexedNode {
    fnode: String,
    rel_path: String,
    node: MdocNode,
}

struct CurrentEvidence {
    source_sha256: String,
    artifact_sha256: String,
    environment_sha256: String,
    dependencies: BTreeSet<String>,
}

struct Candidate {
    token: String,
    artifact_sha256: String,
    dependencies: BTreeMap<String, String>,
}

struct LanguageState {
    status: FormalCodeStatus,
    candidate: Option<Candidate>,
}

struct EvaluatedNode {
    lean: LanguageState,
    rocq: LanguageState,
}

#[derive(Default)]
struct EvaluationCaches {
    external_input_digests: HashMap<String, String>,
    environment_digests: HashMap<String, Option<String>>,
}

struct WorkspaceEvaluation {
    root: PathBuf,
    nodes: Vec<IndexedNode>,
    states: Vec<EvaluatedNode>,
    index_by_fnode: HashMap<String, usize>,
    module_by_fnode: HashMap<String, String>,
    guards: Vec<InputGuard>,
}

#[derive(Default)]
pub(crate) struct FormalStatusValidation {
    evidence: Option<FormalStatusEvidence>,
}

struct FormalStatusEvidence {
    root: PathBuf,
    guards: Vec<InputGuard>,
    manifest_path: PathBuf,
    manifest_snapshot: FileSnapshot,
}

enum InputGuard {
    Workspace {
        path: PathBuf,
        snapshot: Option<ReadFileSnapshot>,
    },
    External {
        path: PathBuf,
        snapshot: FileSnapshot,
    },
}

pub(crate) struct FormalEnvironmentEvidence {
    root: PathBuf,
    digest: String,
    guards: Vec<InputGuard>,
}

impl FormalEnvironmentEvidence {
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    pub(crate) fn ensure_current(&self) -> Result<()> {
        ensure_guards(&self.root, &self.guards)
    }
}

pub(crate) struct CompilerIdentityEvidence {
    path: PathBuf,
    path_string: String,
    digest: String,
    snapshot: FileSnapshot,
}

impl CompilerIdentityEvidence {
    pub(crate) fn path(&self) -> &str {
        &self.path_string
    }

    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    pub(crate) fn ensure_current(&self) -> Result<()> {
        if self.snapshot.file_generation_unchanged(&self.path)? {
            Ok(())
        } else {
            bail!("formal compiler changed during compilation")
        }
    }
}

impl WorkspaceEvaluation {
    fn ensure_current(&self) -> Result<()> {
        ensure_guards(&self.root, &self.guards)
    }
}

impl FormalStatusValidation {
    pub(crate) fn ensure_current(&self) -> Result<()> {
        let Some(evidence) = &self.evidence else {
            return Ok(());
        };
        ensure_guards(&evidence.root, &evidence.guards)?;
        ensure_unchanged_beneath(
            &evidence.manifest_snapshot,
            &evidence.root,
            &evidence.manifest_path,
        )
    }
}

pub(crate) fn refresh_index_statuses(
    conn: &Connection,
    root: &Path,
) -> Result<FormalStatusValidation> {
    let _profile = crate::profile::scope("formal_status::refresh_index_statuses");
    let loaded = match formal_attestation::load_for_status(root) {
        Ok(loaded) => loaded,
        Err(_) => {
            downgrade_verified_statuses(conn)?;
            return Ok(FormalStatusValidation::default());
        }
    };
    if !loaded.manifest.has_attestations() {
        downgrade_verified_statuses(conn)?;
        return Ok(FormalStatusValidation::default());
    }
    let evaluation =
        match evaluate_workspace(conn, root, &loaded.manifest, None).and_then(|evaluation| {
            evaluation.ensure_current()?;
            ensure_unchanged_beneath(&loaded.snapshot, root, &loaded.path)?;
            Ok(evaluation)
        }) {
            Ok(evaluation) => evaluation,
            Err(error) if error.chain().any(|cause| cause.is::<rusqlite::Error>()) => {
                return Err(error)
            }
            Err(_) => {
                downgrade_verified_statuses(conn)?;
                return Ok(FormalStatusValidation::default());
            }
        };

    downgrade_verified_statuses(conn)?;
    let mut update = conn.prepare(
        "UPDATE mdoc_files SET lean_status = ?, rocq_status = ?
         WHERE path = ? AND (lean_status <> ? OR rocq_status <> ?)",
    )?;
    for (node, state) in evaluation.nodes.iter().zip(&evaluation.states) {
        let lean = status_value(state.lean.status);
        let rocq = status_value(state.rocq.status);
        update.execute(rusqlite::params![lean, rocq, node.rel_path, lean, rocq,])?;
    }
    let evidence = if evaluation.states.iter().any(|state| {
        state.lean.status == FormalCodeStatus::Verified
            || state.rocq.status == FormalCodeStatus::Verified
    }) {
        Some(FormalStatusEvidence {
            root: evaluation.root,
            guards: evaluation.guards,
            manifest_path: loaded.path,
            manifest_snapshot: loaded.snapshot,
        })
    } else {
        None
    };
    crate::workspace::run_test_hook(crate::workspace::TestHookPoint::FormalStatusAfterEvaluation);
    Ok(FormalStatusValidation { evidence })
}

pub(crate) fn downgrade_verified_statuses(conn: &Connection) -> Result<()> {
    // Keep the index usable without retaining any previously verified state.
    conn.execute(
        "UPDATE mdoc_files SET lean_status = 1 WHERE lean_status = 2",
        [],
    )?;
    conn.execute(
        "UPDATE mdoc_files SET rocq_status = 1 WHERE rocq_status = 2",
        [],
    )?;
    Ok(())
}

pub(crate) fn prepare_attestation(
    conn: &Connection,
    root: &Path,
    manifest: &FormalAttestationManifest,
    fnode: &str,
    language: &str,
    receipt: &FormalCompilationReceipt,
) -> Result<FormalAttestation> {
    let _profile = crate::profile::scope("formal_status::prepare_attestation");
    let evaluation = evaluate_workspace(conn, root, manifest, Some(fnode))?;
    let index = evaluation
        .index_by_fnode
        .get(fnode)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("formal source is not one valid indexed node: {fnode}"))?;
    let node = &evaluation.nodes[index];
    if node.node.source_block(language).is_none() {
        bail!("node has no archived {language} source block");
    }
    let mut guards = Vec::new();
    let mut environment_digests = HashMap::new();
    let evidence = current_evidence(root, node, language, &mut environment_digests, &mut guards)?
        .map_err(anyhow::Error::msg)?;
    let target_module = module_key(Path::new(&node.rel_path))?;
    if receipt.language != language
        || receipt.target_module != target_module
        || receipt.source_sha256 != evidence.source_sha256
        || receipt.artifact_sha256 != evidence.artifact_sha256
        || receipt.environment_sha256 != evidence.environment_sha256
    {
        bail!("{language} compiler receipt does not match the current formal generation");
    }
    if !receipt_inputs_current(receipt, &mut guards)? {
        bail!("{language} compiler or external dependency changed after compilation");
    }
    let workspace_modules = expected_workspace_modules(
        &evidence.dependencies,
        &evaluation.module_by_fnode,
        language,
    )?;
    if receipt
        .direct_dependencies
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != workspace_modules
    {
        bail!("{language} workspace imports must exactly match direct @dep entries");
    }
    let mut dependencies = BTreeMap::new();
    for dependency in &evidence.dependencies {
        if manifest.get(dependency, language).is_none() {
            bail!("formal dependency is not verified for {language}: {dependency}");
        }
        let dependency_index = evaluation
            .index_by_fnode
            .get(dependency)
            .copied()
            .ok_or_else(|| {
                anyhow::anyhow!("formal dependency is missing or invalid: {dependency}")
            })?;
        let dependency_state = language_state(&evaluation.states[dependency_index], language)?;
        if dependency_state.status != FormalCodeStatus::Verified {
            bail!("formal dependency is not verified for {language}: {dependency}");
        }
        let token = dependency_state
            .candidate
            .as_ref()
            .map(|candidate| candidate.token.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("verified dependency has no attestation: {dependency}")
            })?;
        let module = evaluation.module_by_fnode.get(dependency).ok_or_else(|| {
            anyhow::anyhow!("formal dependency has no workspace module: {dependency}")
        })?;
        let compiled_artifact = receipt.direct_dependencies.get(module).ok_or_else(|| {
            anyhow::anyhow!("compiler receipt omitted formal dependency: {dependency}")
        })?;
        let attested_artifact = dependency_state
            .candidate
            .as_ref()
            .map(|candidate| candidate.artifact_sha256.as_str())
            .ok_or_else(|| anyhow::anyhow!("verified dependency has no artifact: {dependency}"))?;
        if compiled_artifact != attested_artifact {
            bail!("compiled dependency artifact changed for {language}: {dependency}");
        }
        dependencies.insert(dependency.clone(), token);
    }
    let attestation = FormalAttestation {
        fnode: node.fnode.clone(),
        rel_path: node.rel_path.clone(),
        target_module,
        source_sha256: evidence.source_sha256,
        artifact_sha256: evidence.artifact_sha256,
        environment_sha256: evidence.environment_sha256,
        compiler_path: receipt.compiler_path.clone(),
        compiler_sha256: receipt.compiler_sha256.clone(),
        workspace_modules,
        dependencies,
        external_dependencies: receipt.external_dependencies.clone(),
    };
    evaluation.ensure_current()?;
    ensure_guards(root, &guards)?;
    Ok(attestation)
}

fn evaluate_workspace(
    conn: &Connection,
    root: &Path,
    manifest: &FormalAttestationManifest,
    required_fnode: Option<&str>,
) -> Result<WorkspaceEvaluation> {
    let _profile = crate::profile::scope("formal_status::evaluate_workspace");
    let mut guards = Vec::new();
    let nodes = load_indexed_nodes(conn, root, manifest, required_fnode, &mut guards)?;
    let module_by_fnode = module_by_fnode(conn, &nodes)?;
    let mut states = Vec::with_capacity(nodes.len());
    let mut caches = EvaluationCaches::default();
    for node in &nodes {
        states.push(EvaluatedNode {
            lean: evaluate_language(
                root,
                node,
                "lean",
                &module_by_fnode,
                manifest,
                &mut caches,
                &mut guards,
            )?,
            rocq: evaluate_language(
                root,
                node,
                "rocq",
                &module_by_fnode,
                manifest,
                &mut caches,
                &mut guards,
            )?,
        });
    }
    let index_by_fnode = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.fnode.clone(), index))
        .collect::<HashMap<_, _>>();
    propagate_verified(&mut states, &index_by_fnode, "lean")?;
    propagate_verified(&mut states, &index_by_fnode, "rocq")?;
    Ok(WorkspaceEvaluation {
        root: root.to_path_buf(),
        nodes,
        states,
        index_by_fnode,
        module_by_fnode,
        guards,
    })
}

fn load_indexed_nodes(
    conn: &Connection,
    root: &Path,
    manifest: &FormalAttestationManifest,
    required_fnode: Option<&str>,
    guards: &mut Vec<InputGuard>,
) -> Result<Vec<IndexedNode>> {
    let _profile = crate::profile::scope("formal_status::load_indexed_nodes");
    let mut required = manifest.nodes.keys().cloned().collect::<BTreeSet<_>>();
    if let Some(fnode) = required_fnode {
        required.insert(fnode.to_string());
    }
    if required.is_empty() {
        return Ok(Vec::new());
    }

    let required = required.into_iter().collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(required.len());
    for chunk in required.chunks(512) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT m.path, m.fnode
             FROM mdocs m
             WHERE m.fnode IN ({placeholders})
               AND NOT EXISTS (
                   SELECT 1 FROM mdoc_issues i
                   WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
               )"
        );
        let mut stmt = conn.prepare(&sql)?;
        rows.extend(
            stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let worker_count = formal_worker_count(rows.len());
    if worker_count == 0 {
        return Ok(Vec::new());
    }
    let chunk_size = rows.len().div_ceil(worker_count);
    let loaded = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in rows.chunks(chunk_size) {
            workers.push(
                scope.spawn(move || -> Result<Vec<(IndexedNode, InputGuard)>> {
                    let mut snapshots = FileSnapshotBatch::new(root)?;
                    let mut loaded = Vec::with_capacity(chunk.len());
                    for (rel_path, expected_fnode) in chunk {
                        let path = crate::workspace::resolve_mdoc_path(root, Path::new(rel_path))?;
                        let snapshot = snapshots.capture_read(&path)?.ok_or_else(|| {
                            anyhow::anyhow!(
                            "indexed mdoc disappeared during formal status evaluation: {rel_path}"
                        )
                        })?;
                        let node = MdocNode::load_bytes(&path, snapshot.content())?;
                        if node.fnode != *expected_fnode {
                            bail!(
                            "indexed mdoc identity changed: expected {expected_fnode}, found {}",
                            node.fnode
                        );
                        }
                        loaded.push((
                            IndexedNode {
                                fnode: expected_fnode.clone(),
                                rel_path: rel_path.clone(),
                                node,
                            },
                            InputGuard::Workspace {
                                path,
                                snapshot: Some(snapshot),
                            },
                        ));
                    }
                    snapshots.finish()?;
                    Ok(loaded)
                }),
            );
        }
        join_formal_workers(workers)
    })?;
    let mut nodes = Vec::with_capacity(loaded.len());
    for (node, guard) in loaded {
        nodes.push(node);
        guards.push(guard);
    }
    Ok(nodes)
}

fn evaluate_language(
    root: &Path,
    node: &IndexedNode,
    language: &str,
    module_by_fnode: &HashMap<String, String>,
    manifest: &FormalAttestationManifest,
    caches: &mut EvaluationCaches,
    guards: &mut Vec<InputGuard>,
) -> Result<LanguageState> {
    if node.node.source_block(language).is_none() {
        return Ok(LanguageState {
            status: FormalCodeStatus::NoCode,
            candidate: None,
        });
    }
    let Some(attestation) = manifest.get(&node.fnode, language) else {
        return Ok(LanguageState {
            status: FormalCodeStatus::Unverified,
            candidate: None,
        });
    };
    let evidence = match current_evidence(
        root,
        node,
        language,
        &mut caches.environment_digests,
        guards,
    ) {
        Ok(Ok(evidence)) => evidence,
        Ok(Err(_)) | Err(_) => {
            return Ok(LanguageState {
                status: FormalCodeStatus::Unverified,
                candidate: None,
            })
        }
    };
    let inputs_current =
        attested_inputs_current(attestation, &mut caches.external_input_digests, guards)
            .unwrap_or_default();
    if !inputs_current {
        return Ok(LanguageState {
            status: FormalCodeStatus::Unverified,
            candidate: None,
        });
    }
    let attested_dependencies = attestation
        .dependencies
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let workspace_modules =
        match expected_workspace_modules(&evidence.dependencies, module_by_fnode, language) {
            Ok(modules) => modules,
            Err(_) => {
                return Ok(LanguageState {
                    status: FormalCodeStatus::Unverified,
                    candidate: None,
                })
            }
        };
    if attestation.fnode != node.fnode
        || attestation.rel_path != node.rel_path
        || attestation.target_module != module_key(Path::new(&node.rel_path))?
        || attestation.source_sha256 != evidence.source_sha256
        || attestation.artifact_sha256 != evidence.artifact_sha256
        || attestation.environment_sha256 != evidence.environment_sha256
        || attestation.workspace_modules != workspace_modules
        || attested_dependencies != evidence.dependencies
    {
        return Ok(LanguageState {
            status: FormalCodeStatus::Unverified,
            candidate: None,
        });
    }
    Ok(LanguageState {
        status: FormalCodeStatus::Unverified,
        candidate: Some(Candidate {
            token: formal_attestation::token(language, attestation)?,
            artifact_sha256: attestation.artifact_sha256.clone(),
            dependencies: attestation.dependencies.clone(),
        }),
    })
}

fn current_evidence(
    root: &Path,
    node: &IndexedNode,
    language: &str,
    environment_digests: &mut HashMap<String, Option<String>>,
    guards: &mut Vec<InputGuard>,
) -> Result<std::result::Result<CurrentEvidence, String>> {
    let block = node
        .node
        .source_block(language)
        .ok_or_else(|| anyhow::anyhow!("indexed formal block presence is stale"))?;
    let relative = Path::new(&node.rel_path);
    let source_path = source_path(root, relative, language)?;
    let Some(source) = guarded_content(root, &source_path, guards)? else {
        return Ok(Err(format!("{language} source mirror is missing")));
    };
    if source != block.content.as_bytes() {
        return Ok(Err(format!(
            "{language} source mirror differs from the archived .mdoc block"
        )));
    }
    let expected_dependencies = node.node.depens.iter().cloned().collect::<BTreeSet<_>>();
    let artifact_path = artifact_path(root, relative, language)?;
    let Some(artifact) = guarded_content(root, &artifact_path, guards)? else {
        return Ok(Err(format!("{language} compiler artifact is missing")));
    };
    if !environment_digests.contains_key(language) {
        let digest = environment_digest_internal(root, language, Some(guards))?;
        environment_digests.insert(language.to_string(), digest);
    }
    let Some(environment_sha256) = environment_digests.get(language).cloned().flatten() else {
        return Ok(Err(format!(
            "{language} compiler environment is incomplete or stale"
        )));
    };
    Ok(Ok(CurrentEvidence {
        source_sha256: digest(&source),
        artifact_sha256: digest(&artifact),
        environment_sha256,
        dependencies: expected_dependencies,
    }))
}

fn propagate_verified(
    states: &mut [EvaluatedNode],
    index_by_fnode: &HashMap<String, usize>,
    language: &str,
) -> Result<()> {
    let mut remaining = vec![None; states.len()];
    let mut referrers = vec![Vec::new(); states.len()];
    for (index, state) in states.iter().enumerate() {
        let Some(candidate) = language_state(state, language)?.candidate.as_ref() else {
            continue;
        };
        let mut dependencies = Vec::with_capacity(candidate.dependencies.len());
        let valid = candidate.dependencies.iter().all(|(fnode, token)| {
            let Some(dependency_index) = index_by_fnode.get(fnode).copied() else {
                return false;
            };
            let token_matches = language_state(&states[dependency_index], language)
                .ok()
                .and_then(|state| state.candidate.as_ref())
                .is_some_and(|dependency| dependency.token == *token);
            if token_matches {
                dependencies.push(dependency_index);
            }
            token_matches
        });
        if valid {
            remaining[index] = Some(dependencies.len());
            for dependency in dependencies {
                referrers[dependency].push(index);
            }
        }
    }

    let mut queue = std::collections::VecDeque::new();
    for (index, count) in remaining.iter().enumerate() {
        if *count == Some(0) {
            queue.push_back(index);
        }
    }
    while let Some(index) = queue.pop_front() {
        language_state_mut(&mut states[index], language)?.status = FormalCodeStatus::Verified;
        for &referrer in &referrers[index] {
            let Some(count) = &mut remaining[referrer] else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                queue.push_back(referrer);
            }
        }
    }
    Ok(())
}

fn language_state<'a>(state: &'a EvaluatedNode, language: &str) -> Result<&'a LanguageState> {
    match language {
        "lean" => Ok(&state.lean),
        "rocq" => Ok(&state.rocq),
        _ => bail!("unsupported formal language: {language}"),
    }
}

fn language_state_mut<'a>(
    state: &'a mut EvaluatedNode,
    language: &str,
) -> Result<&'a mut LanguageState> {
    match language {
        "lean" => Ok(&mut state.lean),
        "rocq" => Ok(&mut state.rocq),
        _ => bail!("unsupported formal language: {language}"),
    }
}

fn module_by_fnode(conn: &Connection, nodes: &[IndexedNode]) -> Result<HashMap<String, String>> {
    let mut modules = nodes
        .iter()
        .map(|node| Ok((node.fnode.clone(), module_key(Path::new(&node.rel_path))?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let missing = nodes
        .iter()
        .flat_map(|node| node.node.depens.iter())
        .filter(|fnode| !modules.contains_key(*fnode))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for chunk in missing.chunks(512) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT m.path, m.fnode
             FROM mdocs m
             WHERE m.fnode IN ({placeholders})
               AND NOT EXISTS (
                   SELECT 1 FROM mdoc_issues i
                   WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
               )"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (path, fnode) in rows {
            modules.insert(fnode, module_key(Path::new(&path))?);
        }
    }
    Ok(modules)
}

fn expected_workspace_modules(
    dependencies: &BTreeSet<String>,
    module_by_fnode: &HashMap<String, String>,
    language: &str,
) -> Result<BTreeSet<String>> {
    dependencies
        .iter()
        .map(|dependency| {
            module_by_fnode.get(dependency).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "formal dependency is missing or invalid for {language}: {dependency}"
                )
            })
        })
        .collect()
}

pub(crate) fn module_key(relative: &Path) -> Result<String> {
    let source = relative.with_extension("");
    let mut components = Vec::new();
    for component in source.components() {
        let Component::Normal(component) = component else {
            bail!("invalid formal module path: {}", relative.display());
        };
        components.push(
            component
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("formal module path is not valid UTF-8"))?,
        );
    }
    if components.is_empty() {
        bail!("empty formal module path");
    }
    Ok(components.join("/"))
}

#[cfg(test)]
pub(crate) fn environment_digest(root: &Path, language: &str) -> Result<Option<String>> {
    environment_digest_internal(root, language, None)
}

pub(crate) fn capture_environment(
    root: &Path,
    language: &str,
) -> Result<Option<FormalEnvironmentEvidence>> {
    let mut guards = Vec::new();
    let Some(digest) = environment_digest_internal(root, language, Some(&mut guards))? else {
        return Ok(None);
    };
    Ok(Some(FormalEnvironmentEvidence {
        root: root.to_path_buf(),
        digest,
        guards,
    }))
}

fn environment_digest_internal(
    root: &Path,
    language: &str,
    mut guards: Option<&mut Vec<InputGuard>>,
) -> Result<Option<String>> {
    let language_root = root.join(".mdc").join(language);
    let (required, optional): (&[&str], &[&str]) = match language {
        "lean" => (
            &["lakefile.toml", "lean-toolchain"],
            &["lake-manifest.json", "lakefile.lean"],
        ),
        "rocq" => (&[".mdc-module-inventory"], &[]),
        _ => bail!("unsupported formal language: {language}"),
    };
    if language == "rocq" {
        let marker = language_root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME);
        if guarded_or_stable_content(root, &marker, guards.as_deref_mut())?.is_some() {
            return Ok(None);
        }
    }
    let mut digest = Sha256::new();
    digest.update(b"mathdoc-formal-environment-v1\0");
    digest.update(language.as_bytes());
    for name in required {
        let path = language_root.join(name);
        let Some(content) = guarded_or_stable_content(root, &path, guards.as_deref_mut())? else {
            return Ok(None);
        };
        hash_value(&mut digest, name.as_bytes());
        hash_value(&mut digest, b"present");
        hash_value(&mut digest, &content);
    }
    for name in optional {
        let path = language_root.join(name);
        hash_value(&mut digest, name.as_bytes());
        match guarded_or_stable_content(root, &path, guards.as_deref_mut())? {
            Some(content) => {
                hash_value(&mut digest, b"present");
                hash_value(&mut digest, &content);
            }
            None => hash_value(&mut digest, b"missing"),
        }
    }
    Ok(Some(format!("{:x}", digest.finalize())))
}

fn guarded_or_stable_content(
    root: &Path,
    path: &Path,
    guards: Option<&mut Vec<InputGuard>>,
) -> Result<Option<Vec<u8>>> {
    match guards {
        Some(guards) => guarded_content(root, path, guards),
        None => stable_content(root, path),
    }
}

fn guarded_content(
    root: &Path,
    path: &Path,
    guards: &mut Vec<InputGuard>,
) -> Result<Option<Vec<u8>>> {
    let mut snapshots = FileSnapshotBatch::new(root)?;
    let snapshot = snapshots.capture_read(path)?;
    snapshots.finish()?;
    let content = snapshot
        .as_ref()
        .map(|snapshot| snapshot.content().to_vec());
    guards.push(InputGuard::Workspace {
        path: path.to_path_buf(),
        snapshot,
    });
    Ok(content)
}

#[cfg(test)]
pub(crate) fn compiler_identity(path: &Path) -> Result<(String, String)> {
    let evidence = capture_compiler_identity(path)?;
    Ok((evidence.path_string, evidence.digest))
}

pub(crate) fn capture_compiler_identity(path: &Path) -> Result<CompilerIdentityEvidence> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving formal compiler {}", path.display()))?;
    let snapshot = FileSnapshot::capture(&canonical)?;
    let content = snapshot
        .content()
        .ok_or_else(|| anyhow::anyhow!("formal compiler is missing: {}", canonical.display()))?;
    let path_string = canonical
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("formal compiler path is not valid UTF-8"))?
        .to_string();
    let digest = digest(content);
    if !snapshot.file_generation_unchanged(&canonical)? {
        bail!(
            "formal compiler changed while reading: {}",
            canonical.display()
        );
    }
    Ok(CompilerIdentityEvidence {
        path: canonical,
        path_string,
        digest,
        snapshot,
    })
}

fn receipt_inputs_current(
    receipt: &FormalCompilationReceipt,
    guards: &mut Vec<InputGuard>,
) -> Result<bool> {
    let mut digests = HashMap::new();
    external_inputs_current(
        &receipt.compiler_path,
        &receipt.compiler_sha256,
        &receipt.external_dependencies,
        &mut digests,
        guards,
    )
}

fn attested_inputs_current(
    attestation: &FormalAttestation,
    digests: &mut HashMap<String, String>,
    guards: &mut Vec<InputGuard>,
) -> Result<bool> {
    external_inputs_current(
        &attestation.compiler_path,
        &attestation.compiler_sha256,
        &attestation.external_dependencies,
        digests,
        guards,
    )
}

fn external_inputs_current(
    compiler_path: &str,
    compiler_sha256: &str,
    external_dependencies: &BTreeMap<String, String>,
    digests: &mut HashMap<String, String>,
    guards: &mut Vec<InputGuard>,
) -> Result<bool> {
    if external_input_digest(compiler_path, digests, guards)?.as_deref() != Some(compiler_sha256) {
        return Ok(false);
    }
    for (path, expected_digest) in external_dependencies {
        if external_input_digest(path, digests, guards)?.as_ref() != Some(expected_digest) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn external_input_digest(
    path: &str,
    digests: &mut HashMap<String, String>,
    guards: &mut Vec<InputGuard>,
) -> Result<Option<String>> {
    if let Some(digest) = digests.get(path) {
        return Ok(Some(digest.clone()));
    }
    let Some(content) = guarded_external_content(Path::new(path), guards)? else {
        return Ok(None);
    };
    let value = digest(&content);
    digests.insert(path.to_string(), value.clone());
    Ok(Some(value))
}

fn guarded_external_content(path: &Path, guards: &mut Vec<InputGuard>) -> Result<Option<Vec<u8>>> {
    if !path.is_absolute() {
        bail!(
            "formal compiler input path is not absolute: {}",
            path.display()
        );
    }
    let snapshot = FileSnapshot::capture(path)?;
    let content = snapshot.content().map(ToOwned::to_owned);
    if !snapshot.file_generation_unchanged(path)? {
        bail!(
            "formal compiler input changed while reading: {}",
            path.display()
        );
    }
    guards.push(InputGuard::External {
        path: path.to_path_buf(),
        snapshot,
    });
    Ok(content)
}

fn stable_content(root: &Path, path: &Path) -> Result<Option<Vec<u8>>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let resolved_path = path
        .strip_prefix(root)
        .map(|relative| canonical_root.join(relative))
        .unwrap_or_else(|_| path.to_path_buf());
    let path = resolved_path.as_path();
    if !path.starts_with(&canonical_root) {
        bail!(
            "formal verification path is outside the workspace: {}",
            path.display()
        );
    }
    let snapshot = match FileSnapshot::capture(path) {
        Ok(snapshot) => snapshot,
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            }) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    let content = snapshot.content().map(ToOwned::to_owned);
    if !snapshot.file_generation_unchanged(path)? {
        bail!(
            "formal verification input changed while reading: {}",
            path.display()
        );
    }
    Ok(content)
}

fn ensure_unchanged_beneath(snapshot: &FileSnapshot, root: &Path, path: &Path) -> Result<()> {
    if snapshot.unchanged_beneath(root, path)? {
        Ok(())
    } else {
        bail!(
            "formal verification input changed while reading: {}",
            path.display()
        )
    }
}

fn ensure_guards(root: &Path, guards: &[InputGuard]) -> Result<()> {
    let workspace = guards
        .iter()
        .filter_map(|guard| match guard {
            InputGuard::Workspace { path, snapshot } => Some((path, snapshot)),
            InputGuard::External { .. } => None,
        })
        .collect::<Vec<_>>();
    ensure_workspace_guards(root, &workspace)?;
    for guard in guards {
        if let InputGuard::External { path, snapshot } = guard {
            if !snapshot.file_generation_unchanged(path)? {
                bail!(
                    "formal compiler input changed while reading: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn ensure_workspace_guards(
    root: &Path,
    guards: &[(&PathBuf, &Option<ReadFileSnapshot>)],
) -> Result<()> {
    let _profile = crate::profile::scope("formal_status::ensure_workspace_guards");
    let worker_count = formal_worker_count(guards.len());
    if worker_count == 0 {
        return Ok(());
    }
    let chunk_size = guards.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        for chunk in guards.chunks(chunk_size) {
            workers.push(scope.spawn(move || -> Result<Vec<()>> {
                let mut snapshots = FileSnapshotBatch::new(root)?;
                for (path, expected) in chunk {
                    let current = snapshots.capture_read(path)?;
                    let unchanged = match expected {
                        Some(expected) => expected.matches(current.as_ref()),
                        None => current.is_none(),
                    };
                    if !unchanged {
                        bail!(
                            "formal verification input changed while reading: {}",
                            path.display()
                        );
                    }
                }
                snapshots.finish()?;
                Ok(Vec::new())
            }));
        }
        join_formal_workers(workers).map(|_| ())
    })
}

fn formal_worker_count(item_count: usize) -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(12)
        .min(item_count)
}

fn join_formal_workers<T>(
    workers: Vec<std::thread::ScopedJoinHandle<'_, Result<Vec<T>>>>,
) -> Result<Vec<T>> {
    let mut result = Vec::new();
    let mut first_error = None;
    for worker in workers {
        match worker.join() {
            Ok(Ok(items)) => result.extend(items),
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(_) => {
                if first_error.is_none() {
                    first_error = Some(anyhow::anyhow!("formal snapshot worker panicked"));
                }
            }
        }
    }
    first_error.map_or(Ok(result), Err)
}

fn source_path(root: &Path, relative: &Path, language: &str) -> Result<PathBuf> {
    match language {
        "lean" => Ok(lean_source_path(root, relative)),
        "rocq" => Ok(rocq_source_path(root, relative)),
        _ => bail!("unsupported formal language: {language}"),
    }
}

fn artifact_path(root: &Path, relative: &Path, language: &str) -> Result<PathBuf> {
    match language {
        "lean" => Ok(lean_artifact_path(root, relative)),
        "rocq" => Ok(rocq_artifact_path(root, relative)),
        _ => bail!("unsupported formal language: {language}"),
    }
}

fn digest(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

pub(crate) fn content_digest(content: &[u8]) -> String {
    digest(content)
}

pub(crate) fn file_digest(root: &Path, path: &Path) -> Result<String> {
    let content = stable_content(root, path)?
        .ok_or_else(|| anyhow::anyhow!("formal compiler output is missing: {}", path.display()))?;
    Ok(digest(&content))
}

fn hash_value(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

fn status_value(status: FormalCodeStatus) -> i64 {
    match status {
        FormalCodeStatus::NoCode => 0,
        FormalCodeStatus::Unverified => 1,
        FormalCodeStatus::Verified => 2,
    }
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
    use crate::indcache::IndCache;
    use crate::mdocnode::SrcBlock;
    use std::collections::HashMap;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn lean_node(
        root: &Path,
        relative: &str,
        title: &str,
        dependencies: &[String],
        content: &str,
    ) -> MdocNode {
        let mut node = MdocNode::new_at_path(&root.join(relative), title);
        node.depens = dependencies.to_vec();
        node.blocks.push(SrcBlock {
            srctype: "lean".to_string(),
            content: content.to_string(),
            metadata: HashMap::new(),
        });
        write(&node.path, &node.render().unwrap());
        write(&lean_source_path(root, Path::new(relative)), content);
        write(
            &lean_artifact_path(root, Path::new(relative)),
            &format!("artifact for {relative}"),
        );
        node
    }

    fn lean_environment(root: &Path) {
        write(
            &root.join(".mdc/lean/lakefile.toml"),
            "name = \"Lib\"\n[[lean_lib]]\nname = \"Lib\"\n",
        );
        write(
            &root.join(".mdc/lean/lean-toolchain"),
            "leanprover/lean4:stable\n",
        );
    }

    fn lean_receipt(cache: &mut IndCache, root: &Path, fnode: &str) -> FormalCompilationReceipt {
        let (_, _, path) = cache.resolve_ref(fnode, Some(root)).unwrap();
        let node = MdocNode::load(&path).unwrap();
        let relative = path.strip_prefix(cache.root()).unwrap();
        let mut direct_dependencies = BTreeMap::new();
        for dependency in &node.depens {
            let (_, _, dependency_path) = cache.resolve_ref(dependency, Some(root)).unwrap();
            let dependency_relative = dependency_path.strip_prefix(cache.root()).unwrap();
            direct_dependencies.insert(
                module_key(dependency_relative).unwrap(),
                file_digest(root, &lean_artifact_path(root, dependency_relative)).unwrap(),
            );
        }
        let compiler = std::env::current_exe().unwrap();
        let (compiler_path, compiler_sha256) = compiler_identity(&compiler).unwrap();
        FormalCompilationReceipt {
            language: "lean".to_string(),
            target_module: module_key(relative).unwrap(),
            source_sha256: file_digest(root, &lean_source_path(root, relative)).unwrap(),
            artifact_sha256: file_digest(root, &lean_artifact_path(root, relative)).unwrap(),
            environment_sha256: environment_digest(root, "lean").unwrap().unwrap(),
            compiler_path,
            compiler_sha256,
            direct_dependencies,
            external_dependencies: BTreeMap::new(),
        }
    }

    fn publish(cache: &mut IndCache, root: &Path, fnode: &str) -> Vec<(String, String)> {
        let receipt = lean_receipt(cache, root, fnode);
        publish_receipt(cache, root, fnode, receipt)
    }

    fn publish_receipt(
        cache: &mut IndCache,
        root: &Path,
        fnode: &str,
        receipt: FormalCompilationReceipt,
    ) -> Vec<(String, String)> {
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let lock = work_lock.acquire_mutation_lock().unwrap();
        let manifest_snapshot = formal_attestation::snapshot(root).unwrap();
        cache
            .publish_formal_attestations(
                &work_lock,
                &lock,
                &manifest_snapshot,
                fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap()
    }

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

    #[test]
    fn artifacts_require_a_matching_work_attestation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();

        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );

        let artifact = lean_artifact_path(root, Path::new("leaf.mdoc"));
        write(&artifact, "replaced artifact");
        cache.upsert_path(&node.path).unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn status_queries_discover_unattested_formal_block_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let attested = lean_node(root, "attested.mdoc", "Attested", &[], "def value := 1\n");
        let mut plain = MdocNode::new_at_path(&root.join("plain.mdoc"), "Plain");
        plain
            .upsert_source_block("text", "plain\n".to_string())
            .unwrap();
        write(&plain.path, &plain.render().unwrap());
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &attested.fnode).is_empty());
        drop(cache);

        let metadata = std::fs::metadata(&plain.path).unwrap();
        let modified = metadata.modified().unwrap();
        plain.remove_source_block("text");
        plain
            .upsert_source_block("lean", "plain\n".to_string())
            .unwrap();
        let rendered = plain.render().unwrap();
        assert_eq!(metadata.len(), rendered.len() as u64);
        write(&plain.path, &rendered);
        std::fs::File::options()
            .write(true)
            .open(&plain.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert_eq!(
            cache.formalization_status(&plain.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
        drop(cache);

        let metadata = std::fs::metadata(&plain.path).unwrap();
        let modified = metadata.modified().unwrap();
        plain.remove_source_block("lean");
        plain
            .upsert_source_block("text", "plain\n".to_string())
            .unwrap();
        let rendered = plain.render().unwrap();
        assert_eq!(metadata.len(), rendered.len() as u64);
        write(&plain.path, &rendered);
        std::fs::File::options()
            .write(true)
            .open(&plain.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert_eq!(
            cache.formalization_status(&plain.fnode).unwrap().lean,
            FormalCodeStatus::NoCode
        );
        assert_eq!(
            cache.formalization_status(&attested.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );
    }

    #[test]
    fn cache_open_does_not_scan_unattested_nodes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let attested = lean_node(root, "attested.mdoc", "Attested", &[], "def value := 1\n");
        let plain = MdocNode::new_at_path(&root.join("plain.mdoc"), "Plain");
        write(&plain.path, &plain.render().unwrap());
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &attested.fnode).is_empty());
        drop(cache);

        std::fs::set_permissions(&plain.path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let reopened = IndCache::open(root.to_path_buf()).unwrap();
        assert_eq!(
            reopened
                .indexed_formalization_status(&attested.fnode)
                .unwrap()
                .lean,
            FormalCodeStatus::Verified
        );
        drop(reopened);

        std::fs::set_permissions(&plain.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn malformed_attestation_manifest_is_fail_closed_and_does_not_block_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );

        write(
            &root.join(".mdc/formal-attestations.json"),
            "{ this is not valid json }\n",
        );
        let mut edited = MdocNode::load(&node.path).unwrap();
        edited.remove_source_block("lean");
        write(&edited.path, &edited.render().unwrap());
        drop(cache);

        let mut reopened = IndCache::open(root.to_path_buf()).unwrap();
        assert_eq!(
            reopened.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::NoCode
        );
        reopened.discover_workspace_changes().unwrap();
        assert_eq!(
            reopened.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::NoCode
        );
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_attestation_manifest_downgrades_cached_verification() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        let manifest = root.join(".mdc/formal-attestations.json");
        let unrelated = root.join("unrelated.json");
        write(&unrelated, "{}\n");
        std::fs::remove_file(&manifest).unwrap();
        symlink(&unrelated, &manifest).unwrap();

        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn compiler_and_external_dependency_changes_invalidate_attestations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let compiler = root.join("formal-compiler");
        let external = root.join("external.olean");
        write(&compiler, "compiler generation one");
        write(&external, "external generation one");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let mut receipt = lean_receipt(&mut cache, root, &node.fnode);
        (receipt.compiler_path, receipt.compiler_sha256) = compiler_identity(&compiler).unwrap();
        receipt.external_dependencies.insert(
            external
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            content_digest(b"external generation one"),
        );
        assert!(publish_receipt(&mut cache, root, &node.fnode, receipt).is_empty());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );

        write(&external, "external generation two");
        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );

        let mut receipt = lean_receipt(&mut cache, root, &node.fnode);
        (receipt.compiler_path, receipt.compiler_sha256) = compiler_identity(&compiler).unwrap();
        receipt.external_dependencies.insert(
            external
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            content_digest(b"external generation two"),
        );
        assert!(publish_receipt(&mut cache, root, &node.fnode, receipt).is_empty());
        write(&compiler, "compiler generation two");
        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn failed_publication_does_not_leave_a_latent_attestation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let artifact = lean_artifact_path(root, Path::new("leaf.mdoc"));
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let receipt = lean_receipt(&mut cache, root, &node.fnode);
        let changed_artifact = artifact.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteAfterPersistence,
            move || write(&changed_artifact, "raced artifact"),
        );

        let errors = publish_receipt(&mut cache, root, &node.fnode, receipt);
        assert_eq!(errors.len(), 1);
        write(&artifact, "artifact for leaf.mdoc");
        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn publication_rolls_back_after_mutation_lock_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let receipt = lean_receipt(&mut cache, root, &node.fnode);
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let mutation_lock = work_lock.acquire_mutation_lock().unwrap();
        let expected_manifest = formal_attestation::snapshot(root).unwrap();
        let lock_path = root.join(".mdc/mutation.lock");
        let displaced_lock = root.join("displaced-mutation.lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteAfterPersistence,
            move || {
                std::fs::rename(&lock_path, displaced_lock).unwrap();
                write(&lock_path, "replacement lock");
            },
        );

        let error = cache
            .publish_formal_attestations(
                &work_lock,
                &mutation_lock,
                &expected_manifest,
                &node.fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap_err();

        assert!(error
            .chain()
            .any(|cause| cause.is::<crate::workspace::WorkspaceGenerationError>()));
        assert!(!root.join(".mdc/formal-attestations.json").exists());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn publication_rolls_back_after_work_lock_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let receipt = lean_receipt(&mut cache, root, &node.fnode);
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let mutation_lock = work_lock.acquire_mutation_lock().unwrap();
        let expected_manifest = formal_attestation::snapshot(root).unwrap();
        let lock_path = root.join(".mdc/work.lock");
        let displaced_lock = root.join("displaced-work.lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteAfterPersistence,
            move || {
                std::fs::rename(&lock_path, displaced_lock).unwrap();
                write(&lock_path, "replacement lock");
            },
        );

        let error = cache
            .publish_formal_attestations(
                &work_lock,
                &mutation_lock,
                &expected_manifest,
                &node.fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap_err();

        assert!(error
            .chain()
            .any(|cause| cause.is::<crate::workspace::WorkspaceGenerationError>()));
        assert!(!root.join(".mdc/formal-attestations.json").exists());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn publication_rolls_back_evidence_changed_after_status_evaluation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let artifact = lean_artifact_path(root, Path::new("leaf.mdoc"));
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let receipt = lean_receipt(&mut cache, root, &node.fnode);
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let mutation_lock = work_lock.acquire_mutation_lock().unwrap();
        let expected_manifest = formal_attestation::snapshot(root).unwrap();
        let changed_artifact = artifact.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::FormalStatusAfterEvaluation,
            move || write(&changed_artifact, "raced artifact"),
        );

        let error = cache
            .publish_formal_attestations(
                &work_lock,
                &mutation_lock,
                &expected_manifest,
                &node.fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("formal verification input changed"));
        assert!(!root.join(".mdc/formal-attestations.json").exists());
        write(&artifact, "artifact for leaf.mdoc");
        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn post_commit_evidence_change_downgrades_verified_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let artifact = lean_artifact_path(root, Path::new("leaf.mdoc"));
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        let changed_artifact = artifact.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::FormalStatusAfterEvaluation,
            move || write(&changed_artifact, "raced artifact"),
        );

        let error = cache.refresh_formal_statuses().unwrap_err();

        assert!(error
            .to_string()
            .contains("formal verification input changed"));
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn revocation_reports_mutation_lock_replacement_without_restoring_attestation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(root).unwrap();
        let lock_path = root.join(".mdc/mutation.lock");
        let displaced_lock = root.join("displaced-mutation.lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteAfterPersistence,
            move || {
                std::fs::rename(&lock_path, displaced_lock).unwrap();
                write(&lock_path, "replacement lock");
            },
        );

        let error = cache
            .invalidate_formal_attestations(&mutation_lock, &node.fnode, &["lean".to_string()])
            .unwrap_err();

        assert!(error
            .chain()
            .any(|cause| cause.is::<crate::workspace::WorkspaceGenerationError>()));
        let loaded = formal_attestation::load(cache.root()).unwrap();
        assert!(loaded.manifest.get(&node.fnode, "lean").is_none());
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn publication_rejects_manifest_changes_during_compilation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let receipt = lean_receipt(&mut cache, root, &node.fnode);
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let lock = work_lock.acquire_mutation_lock().unwrap();
        let expected_manifest = formal_attestation::snapshot(root).unwrap();
        write(
            &root.join(".mdc/formal-attestations.json"),
            "{\"version\":1,\"nodes\":{}}\n",
        );

        let error = cache
            .publish_formal_attestations(
                &work_lock,
                &lock,
                &expected_manifest,
                &node.fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap_err();
        assert!(error.to_string().contains("changed during compilation"));
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn a_missing_indexed_mdoc_cannot_retain_verified_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let node = lean_node(root, "leaf.mdoc", "Leaf", &[], "def leaf : Nat := 1\n");
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &node.fnode).is_empty());
        std::fs::remove_file(&node.path).unwrap();

        cache.refresh_formal_statuses().unwrap();
        assert_eq!(
            cache.formalization_status(&node.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );
    }

    #[test]
    fn dependency_changes_invalidate_every_attested_referrer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let dependency = lean_node(
            root,
            "dep.mdoc",
            "Dependency",
            &[],
            "def value : Nat := 1\n",
        );
        let parent = lean_node(
            root,
            "parent.mdoc",
            "Parent",
            std::slice::from_ref(&dependency.fnode),
            "import Lib.dep\n#check value\n",
        );
        let grandparent = lean_node(
            root,
            "grandparent.mdoc",
            "Grandparent",
            std::slice::from_ref(&parent.fnode),
            "import Lib.parent\n#check value\n",
        );
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &dependency.fnode).is_empty());
        assert!(publish(&mut cache, root, &parent.fnode).is_empty());
        assert!(publish(&mut cache, root, &grandparent.fnode).is_empty());

        let mut edited = MdocNode::load(&dependency.path).unwrap();
        edited.blocks[0].content = "def value : Nat := 2\n".to_string();
        write(&edited.path, &edited.render().unwrap());
        write(
            &lean_source_path(root, Path::new("dep.mdoc")),
            "def value : Nat := 2\n",
        );
        cache.upsert_path(&edited.path).unwrap();

        for fnode in [&dependency.fnode, &parent.fnode, &grandparent.fnode] {
            assert_eq!(
                cache.formalization_status(fnode).unwrap().lean,
                FormalCodeStatus::Unverified
            );
        }

        write(
            &lean_artifact_path(root, Path::new("dep.mdoc")),
            "new dependency artifact",
        );
        assert!(publish(&mut cache, root, &dependency.fnode).is_empty());
        assert_eq!(
            cache.formalization_status(&dependency.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );
        assert_eq!(
            cache.formalization_status(&parent.fnode).unwrap().lean,
            FormalCodeStatus::Unverified
        );

        assert!(publish(&mut cache, root, &parent.fnode).is_empty());
        assert!(publish(&mut cache, root, &grandparent.fnode).is_empty());
        assert_eq!(
            cache.formalization_status(&grandparent.fnode).unwrap().lean,
            FormalCodeStatus::Verified
        );
    }

    #[test]
    fn strict_imports_reject_extra_and_unformalized_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        lean_environment(root);
        let dependency = lean_node(root, "dep.mdoc", "Dependency", &[], "def dep : Nat := 1\n");
        let extra = lean_node(root, "extra.mdoc", "Extra", &[], "def extra : Nat := 2\n");
        let parent = lean_node(
            root,
            "parent.mdoc",
            "Parent",
            std::slice::from_ref(&dependency.fnode),
            "import Lib.dep\nimport Lib.extra\n#check dep\n",
        );
        let plain = MdocNode::new_at_path(&root.join("plain.mdoc"), "Plain");
        write(&plain.path, &plain.render().unwrap());
        write(
            &lean_artifact_path(root, Path::new("plain.mdoc")),
            "untrusted plain artifact",
        );
        let missing_formal = lean_node(
            root,
            "missing-formal.mdoc",
            "Missing formal dependency",
            std::slice::from_ref(&plain.fnode),
            "import Lib.plain\n#check Nat\n",
        );
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        assert!(publish(&mut cache, root, &dependency.fnode).is_empty());
        assert!(publish(&mut cache, root, &extra.fnode).is_empty());

        let mut receipt = lean_receipt(&mut cache, root, &parent.fnode);
        receipt.direct_dependencies.insert(
            module_key(Path::new("extra.mdoc")).unwrap(),
            file_digest(root, &lean_artifact_path(root, Path::new("extra.mdoc"))).unwrap(),
        );
        let work_lock = crate::workspace::WorkspaceWorkLock::acquire(root).unwrap();
        let lock = work_lock.acquire_mutation_lock().unwrap();
        let manifest_snapshot = formal_attestation::snapshot(root).unwrap();
        let errors = cache
            .publish_formal_attestations(
                &work_lock,
                &lock,
                &manifest_snapshot,
                &parent.fnode,
                &[("lean".to_string(), true, Some(receipt))],
            )
            .unwrap();
        drop(lock);
        drop(work_lock);
        assert!(errors[0].1.contains("exactly match"));
        let errors = publish(&mut cache, root, &missing_formal.fnode);
        assert!(errors[0].1.contains("not verified"));
    }
}
