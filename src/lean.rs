//! Native Lean LSP sessions plus Lake's existing incremental artifact store.
pub mod cache;
mod certificates;
pub(crate) mod editor;
mod transport;
use crate::store::{digest, LeanProject, Node, Snapshot};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
pub use transport::{command, spawn, Process, Server};

fn timeout() -> Result<Duration> {
    let seconds = std::env::var("MDC_LEAN_TIMEOUT_SECONDS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()
        .context("MDC_LEAN_TIMEOUT_SECONDS must be a positive integer")?
        .or(crate::config::Settings::load()?.lean_timeout_seconds)
        .unwrap_or(300);
    if seconds == 0 {
        bail!("MDC_LEAN_TIMEOUT_SECONDS must be positive");
    }
    Ok(Duration::from_secs(seconds))
}
struct Document {
    version: u64,
    source: String,
    dependency_key: String,
}
struct Lsp {
    server: Server,
    sequence: u64,
    documents: HashMap<String, Document>,
    open_order: VecDeque<String>,
    diagnostics: HashMap<String, (u64, Vec<Value>)>,
}
impl Lsp {
    async fn start(root: &Path) -> Result<Self> {
        let mut lsp = Self {
            server: Server::start(root)?,
            sequence: 0,
            documents: HashMap::new(),
            open_order: VecDeque::new(),
            diagnostics: HashMap::new(),
        };
        lsp.request("initialize",json!({"processId":null,"rootUri":file_uri(root)?,"capabilities":{
            "textDocument":{"publishDiagnostics":{"versionSupport":true}},"experimental":{"silentDiagnosticSupport":true}},
            "initializationOptions":{"hasWidgets":true}})).await?;
        lsp.notify("initialized", json!({})).await?;
        Ok(lsp)
    }
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.server
            .send(json!({"jsonrpc":"2.0","method":method,"params":params}).to_string())
            .await
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        self.server
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string())
            .await?;
        tokio::time::timeout(timeout()?, async {
            loop {
                let message: Value = serde_json::from_str(&self.server.receive().await?)?;
                if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                    if let Some(error) = message.get("error") {
                        bail!("Lean {method}: {error}");
                    }
                    return Ok(message["result"].clone());
                }
                if message["method"] == "textDocument/publishDiagnostics" {
                    let p = &message["params"];
                    if let (Some(uri), Some(version)) = (p["uri"].as_str(), p["version"].as_u64()) {
                        if !self.documents.contains_key(uri) {
                            continue;
                        }
                        let entry = self
                            .diagnostics
                            .entry(uri.into())
                            .or_insert((version, vec![]));
                        if version < entry.0 {
                            continue;
                        }
                        if version != entry.0 || p["isIncremental"] != true {
                            *entry = (version, vec![]);
                        }
                        entry
                            .1
                            .extend(p["diagnostics"].as_array().cloned().unwrap_or_default());
                    }
                } else if message.get("method").is_some() && message.get("id").is_some() {
                    let result = if message["method"] == "workspace/configuration" {
                        json!(message["params"]["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|_| Value::Null)
                            .collect::<Vec<_>>())
                    } else {
                        Value::Null
                    };
                    self.server
                        .send(
                            json!({"jsonrpc":"2.0","id":message["id"],"result":result}).to_string(),
                        )
                        .await?;
                }
            }
        })
        .await
        .context("Lean check timed out")?
    }
    async fn open(&mut self, uri: &str, source: &str, dependency_key: &str) -> Result<u64> {
        self.open_order.retain(|open| open != uri);
        self.open_order.push_back(uri.into());
        // ponytail: one hot CLI file avoids retaining several whole Mathlib environments.
        // Lake artifacts and certificates retain reusable work for evicted files.
        while self.open_order.len() > 1 {
            let old = self.open_order.pop_front().unwrap();
            self.notify("textDocument/didClose", json!({"textDocument":{"uri":old}}))
                .await?;
            self.documents.remove(&old);
            self.diagnostics.remove(&old);
        }
        if self
            .documents
            .get(uri)
            .is_some_and(|d| d.dependency_key != dependency_key)
        {
            self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
                .await?;
            self.documents.remove(uri);
            self.diagnostics.remove(uri);
        }
        let version = if let Some(doc) = self.documents.get(uri) {
            if doc.source == source {
                return Ok(doc.version);
            }
            let version = doc.version + 1;
            self.notify("textDocument/didChange",json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":source}]})).await?;
            version
        } else {
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument":{"uri":uri,"languageId":"lean4","version":1,"text":source}}),
            )
            .await?;
            1
        };
        self.documents.insert(
            uri.into(),
            Document {
                version,
                source: source.into(),
                dependency_key: dependency_key.into(),
            },
        );
        Ok(version)
    }
    async fn check(
        &mut self,
        uri: &str,
        source: &str,
        dependency_key: &str,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        let version = self.open(uri, source, dependency_key).await?;
        self.request(
            "textDocument/waitForDiagnostics",
            json!({"uri":uri,"version":version}),
        )
        .await?;
        let diagnostics = self
            .diagnostics
            .get(uri)
            .filter(|d| d.0 == version)
            .map(|d| d.1.clone())
            .unwrap_or_default();
        // The module hierarchy comes from Lean's elaborated reference information.
        let module = self
            .request(
                "$/lean/prepareModuleHierarchy",
                json!({"textDocument":{"uri":uri}}),
            )
            .await?;
        let imports = if module.is_null() {
            vec![]
        } else {
            self.request("$/lean/moduleHierarchy/imports", json!({"module":module}))
                .await?
                .as_array()
                .context("Lean returned invalid module imports")?
                .clone()
        };
        Ok((diagnostics, imports))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub fnode: String,
    pub revision: String,
    pub input_key: String,
    pub passed: bool,
    pub certified: bool,
    /// Native sorry evidence for this module; absent in older certificates.
    #[serde(default)]
    pub has_sorry: Option<bool>,
    pub built: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<cache::Artifacts>,
    pub cache_hit: bool,
    pub diagnostics: Vec<Value>,
    pub imports: Vec<Value>,
    pub dependency_errors: Vec<String>,
    pub elapsed_ms: u128,
}

fn sorry_warning(message: &str) -> bool {
    // ponytail: use native warnings; a full axiom audit is needed to cover disabled warn.sorry.
    message.contains("declaration uses") && message.contains("sorry")
}

fn diagnostics_have_sorry(diagnostics: &[Value]) -> bool {
    diagnostics
        .iter()
        .any(|d| d["severity"] == 2 && d["message"].as_str().is_some_and(sorry_warning))
}

async fn artifact_has_sorry(base: &Path, module: &str) -> Option<bool> {
    let path = base.join(crate::store::module_file(module, "trace").ok()?);
    let trace: Value = serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()?;
    if trace["synthetic"] != false {
        return None;
    }
    Some(trace["log"].as_array()?.iter().any(|entry| {
        entry["level"] == "warning" && entry["message"].as_str().is_some_and(sorry_warning)
    }))
}

fn artifact_parts(root: &Path, module: &str) -> Option<Vec<PathBuf>> {
    let cache = root.join(".lake/cache");
    if let Some(artifacts) =
        cache::Artifacts::from_trace(root, module).filter(|a| a.complete(&cache))
    {
        return Some(artifacts.parts(&cache));
    }
    let file = root
        .join(".lake/build/lib/lean")
        .join(crate::store::module_file(module, "olean").ok()?);
    if !file.is_file() {
        return None;
    }
    let server = file.with_extension("olean.server");
    if server.is_file() {
        let private = file.with_extension("olean.private");
        return private.is_file().then_some(vec![file, server, private]);
    }
    Some(vec![file])
}

/// Lake's content cache restores synthetic traces without warning logs. Inspect
/// their existing proof bodies once in a batch, using the project's own Lean.
async fn artifact_sorry_fallback(root: &Path, paths: &[Vec<PathBuf>]) -> Result<Vec<Option<bool>>> {
    if paths.is_empty() {
        return Ok(vec![]);
    }
    let script = tempfile::Builder::new().suffix(".lean").tempfile_in(root)?;
    tokio::fs::write(script.path(), include_str!("lean/artifact_status.lean")).await?;
    let mut process = spawn(
        root,
        &[
            "env",
            "lean",
            "--run",
            script
                .path()
                .to_str()
                .context("invalid artifact inspector path")?,
        ],
    )?;
    let mut stdin = process
        .child
        .stdin
        .take()
        .context("Lean stdin unavailable")?;
    let mut stdout = process
        .child
        .stdout
        .take()
        .context("Lean stdout unavailable")?;
    tokio::time::timeout(timeout()?, async {
        let mut input = serde_json::to_vec(paths)?;
        input.push(b'\n');
        stdin.write_all(&input).await?;
        drop(stdin);
        let mut output = vec![];
        stdout.read_to_end(&mut output).await?;
        if !process.child.wait().await?.success() {
            bail!(
                "Lean artifact inspection failed: {}",
                String::from_utf8_lossy(&output)
            );
        }
        let result: Vec<Option<bool>> = serde_json::from_slice(&output)?;
        if result.len() != paths.len() {
            bail!("Lean artifact inspection returned an invalid result count");
        }
        Ok(result)
    })
    .await
    .context("Lean artifact inspection timed out")?
}
#[derive(Clone)]
pub struct Input {
    pub project: Arc<LeanProject>,
    pub project_key: String,
    pub chain: Vec<(Arc<Node>, String)>,
    pub modules: Arc<BTreeMap<PathBuf, String>>,
}
impl Input {
    pub fn environment_key(&self) -> Result<String> {
        let node = &self.chain.last().context("no Lean target")?.0;
        Ok(digest(&serde_json::to_vec(&(
            &self.project_key,
            &node.module,
            self.chain[..self.chain.len() - 1]
                .iter()
                .map(|(_, key)| key)
                .collect::<Vec<_>>(),
        ))?))
    }
    pub fn capture(snapshot: &Snapshot, id: &str) -> Result<Self> {
        let node = snapshot.resolve(id)?;
        let mut seen = BTreeSet::new();
        let mut queue = vec![node.fnode.clone()];
        while let Some(id) = queue.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            queue.extend(snapshot.nodes[&id].depens.iter().cloned());
        }
        let mut ids: Vec<_> = seen.into_iter().collect();
        ids.sort_by_key(|id| snapshot.depths[id]);
        Ok(Self {
            project: snapshot.project.clone(),
            project_key: snapshot.project_key.clone(),
            modules: snapshot.modules.clone(),
            chain: ids
                .into_iter()
                .map(|id| (snapshot.nodes[&id].clone(), snapshot.lean_keys[&id].clone()))
                .collect(),
        })
    }
}
struct Manager {
    project_key: String,
    root: PathBuf,
    lsp: Option<Lsp>,
    sources: SourceState,
}
fn dependency_errors(
    node: &Node,
    imports: &[Value],
    known: &BTreeMap<PathBuf, String>,
    keys: &HashMap<String, String>,
    results: &HashMap<String, CheckResult>,
    root: &Path,
    project: &LeanProject,
) -> Result<Vec<String>> {
    let mut errors = vec![];
    let mut imported = BTreeSet::new();
    for import in imports {
        let module = import["module"]["name"]
            .as_str()
            .context("Lean import omitted module name")?;
        let source = crate::store::module_file(module, "lean")?;
        if let Some(id) = known.get(&source) {
            imported.insert(id.clone());
        } else if !project
            .files
            .contains_key(&source.to_string_lossy().to_string())
            && (root.join(&source).is_file()
                || root
                    .join(".lake/build/lib/lean")
                    .join(source.with_extension("olean"))
                    .is_file())
        {
            // A deleted node's old files are not an external library, even if
            // a warm worker or a concurrent build can still import them.
            errors.push(format!(
                "local Lean import {module} is no longer a graph node or project file"
            ));
        }
    }
    if imported != node.depens.iter().cloned().collect() {
        errors.push("Lean workspace imports must exactly match direct dep entries".into());
    }
    for dep in &node.depens {
        if !results
            .get(dep)
            .is_some_and(|r| keys.get(dep) == Some(&r.input_key) && r.certified)
        {
            errors.push(format!("dependency {dep} is not verified for Lean"));
        }
    }
    Ok(errors)
}
pub struct LeanService {
    root: PathBuf,
    pool: cache::Pool,
    // ponytail: one CLI compiler per branch; add workers if concurrent check throughput requires it.
    manager: Mutex<Manager>,
    results: RwLock<HashMap<String, CheckResult>>,
    certificates: Mutex<HashMap<String, Option<Arc<certificates::Certificates>>>>,
    synced: Mutex<(String, u64)>,
    _lease: std::fs::File,
}
impl LeanService {
    /// Certify only the saved snapshot whose diagnostics the native editor has
    /// completed. Imported .ilean metadata is emitted by the same Lean compiler
    /// that produced the dependencies the worker successfully loaded.
    pub async fn record_editor_check(
        &self,
        root: &Path,
        input: &Input,
        diagnostics: Vec<Value>,
        imports: Vec<Value>,
    ) -> Result<CheckResult> {
        let started = Instant::now();
        verify_manifest(root, &input.project).await?;
        let target = &input.chain.last().context("no Lean target")?.0;
        let known = &input.modules;
        let keys: HashMap<_, _> = input
            .chain
            .iter()
            .map(|(n, k)| (n.fnode.clone(), k.clone()))
            .collect();
        let nodes: HashMap<_, _> = input.chain.iter().map(|(n, _)| (&n.fnode, n)).collect();
        let mut observed = HashMap::from([(
            target.fnode.clone(),
            (
                Some(diagnostics_have_sorry(&diagnostics)),
                diagnostics.clone(),
                imports.clone(),
            ),
        )]);
        let mut pending = if diagnostics.iter().any(|d| d["severity"] == 1) {
            vec![]
        } else {
            imports
        };
        let mut visited = BTreeSet::from([target.fnode.clone()]);
        while let Some(import) = pending.pop() {
            let module = import["module"]["name"]
                .as_str()
                .context("Lean import omitted module name")?;
            let Some(id) = known.get(&crate::store::module_file(module, "lean")?) else {
                continue;
            };
            if !visited.insert(id.clone()) {
                continue;
            }
            let Some(node) = nodes.get(id) else {
                continue;
            };
            if let Some(cached) = self.results.read().unwrap().get(id).filter(|r| {
                r.certified && r.has_sorry.is_some() && keys.get(id) == Some(&r.input_key)
            }) {
                // A known direct dependency can still have older, incomplete
                // transitive certificates. Follow its native imports as well.
                pending.extend(cached.imports.clone());
                continue;
            }
            let Some(source) = node.source("lean") else {
                continue;
            };
            if tokio::fs::read_to_string(module_path(root, &node.module)?).await? != source {
                bail!("editor dependency source changed during checking");
            }
            let base = root.join(".lake/build/lib/lean");
            if artifact_parts(root, &node.module).is_none() {
                continue; // No compiler evidence: leave the dependency unverified.
            }
            #[derive(Deserialize)]
            struct Ilean {
                module: String,
                #[serde(rename = "directImports")]
                direct_imports: Vec<Vec<Value>>,
            }
            let data =
                tokio::fs::read(base.join(crate::store::module_file(&node.module, "ilean")?))
                    .await?;
            let metadata: Ilean = serde_json::from_slice(&data)?;
            if crate::store::module_file(&metadata.module, "lean")?
                != crate::store::module_file(&node.module, "lean")?
            {
                bail!("Lean artifact module mismatch");
            }
            let imports = metadata.direct_imports.iter().map(|i| Ok(json!({"module":{"name":i.first().and_then(Value::as_str).context("invalid Lean artifact import")?}}))).collect::<Result<Vec<_>>>()?;
            pending.extend(imports.clone());
            observed.insert(
                id.clone(),
                (
                    artifact_has_sorry(&base, &node.module).await,
                    vec![],
                    imports,
                ),
            );
        }
        let missing: Vec<_> = observed
            .iter()
            .filter(|(_, (has_sorry, _, _))| has_sorry.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        let paths = missing
            .iter()
            .map(|id| {
                artifact_parts(root, &nodes[id].module).context("Lean import artifacts disappeared")
            })
            .collect::<Result<Vec<_>>>()?;
        for (id, has_sorry) in missing
            .iter()
            .zip(artifact_sorry_fallback(root, &paths).await?)
        {
            observed.get_mut(id).unwrap().0 = has_sorry;
        }
        let mut final_result = None;
        for (node, key) in &input.chain {
            let Some((has_sorry, diagnostics, imports)) = observed.remove(&node.fnode) else {
                continue;
            };
            let passed = !diagnostics.iter().any(|d| d["severity"] == 1);
            let errors = dependency_errors(
                node,
                &imports,
                known,
                &keys,
                &self.results.read().unwrap(),
                root,
                &input.project,
            )?;
            let artifacts = (node.fnode != target.fnode)
                .then(|| cache::Artifacts::from_trace(root, &node.module))
                .flatten()
                .filter(|a| a.complete(&root.join(".lake/cache")));
            let result = CheckResult {
                fnode: node.fnode.clone(),
                revision: node.revision(),
                input_key: key.clone(),
                passed,
                certified: passed && errors.is_empty(),
                has_sorry,
                built: artifacts.is_some(),
                artifacts,
                cache_hit: false,
                diagnostics,
                imports,
                dependency_errors: errors,
                elapsed_ms: started.elapsed().as_millis(),
            };
            self.persist_result(input, &result, root).await?;
            self.results
                .write()
                .unwrap()
                .insert(node.fnode.clone(), result.clone());
            final_result = Some(result);
        }
        self.synced.lock().await.1 = 0;
        final_result
            .filter(|r| r.fnode == target.fnode)
            .context("editor check produced no target result")
    }
    pub fn new(root: PathBuf) -> Result<Self> {
        let pool = root.join(".shared");
        Self::with_shared_cache(root, pool)
    }
    pub fn with_shared_cache(root: PathBuf, shared: PathBuf) -> Result<Self> {
        let pool = cache::Pool::open(shared)?;
        let lease = crate::service::acquire_lease(&root)?;
        Ok(Self {
            manager: Mutex::new(Manager {
                project_key: String::new(),
                root: root.clone(),
                lsp: None,
                sources: HashMap::new(),
            }),
            root,
            pool,
            results: RwLock::new(HashMap::new()),
            certificates: Mutex::new(HashMap::new()),
            synced: Mutex::new((String::new(), 0)),
            _lease: lease,
        })
    }
    pub fn formal_status(&self, node: &Node, key: &str) -> &'static str {
        if node
            .source("lean")
            .is_none_or(|source| source.trim().is_empty())
        {
            return "no_code";
        }
        if self
            .results
            .read()
            .unwrap()
            .get(&node.fnode)
            .is_some_and(|r| r.input_key == key && r.certified && r.has_sorry == Some(false))
        {
            "verified"
        } else {
            "unverified"
        }
    }
    pub fn cached(&self, id: &str, key: &str, revision: &str, build: bool) -> Option<CheckResult> {
        self.results
            .read()
            .ok()?
            .get(id)
            .filter(|r| r.input_key == key && r.has_sorry.is_some() && (!build || r.built))
            .cloned()
            .map(|mut r| {
                r.revision = revision.into();
                r.cache_hit = true;
                r.elapsed_ms = 0;
                r
            })
    }
    pub async fn cached_or_load(
        &self,
        node: &Node,
        key: &str,
        project_key: &str,
        build: bool,
    ) -> Option<CheckResult> {
        let cache = self.pool.project(project_key).join("lake");
        if let Some(mut result) = self.cached(&node.fnode, key, &node.revision(), false) {
            if result.built
                && !result
                    .artifacts
                    .as_ref()
                    .is_some_and(|a| a.complete(&cache))
            {
                result.built = false;
                self.results
                    .write()
                    .ok()?
                    .insert(node.fnode.clone(), result.clone());
            }
            return (!build || result.built).then_some(result);
        }
        None
    }
    async fn certificates(
        &self,
        project: &LeanProject,
        key: &str,
        retry: bool,
    ) -> Option<Arc<certificates::Certificates>> {
        let mut stores = self.certificates.lock().await;
        if let Some(store) = stores.get(key) {
            if store.is_some() || !retry {
                return store.clone();
            }
        }
        let store = tokio::time::timeout(
            Duration::from_secs(5),
            certificates::Certificates::open(self.pool.project(key), &project.toolchain),
        )
        .await
        .ok()
        .and_then(Result::ok);
        stores.insert(key.into(), store.clone());
        store
    }
    async fn persist_result(&self, input: &Input, result: &CheckResult, root: &Path) -> Result<()> {
        if let Some(store) = self
            .certificates(&input.project, &input.project_key, true)
            .await
        {
            store.publish(result, root).await?;
        }
        Ok(())
    }
    fn restore_results<'a>(
        &self,
        store: &certificates::Certificates,
        project: &LeanProject,
        project_key: &str,
        nodes: impl IntoIterator<Item = (&'a Node, &'a String)>,
        modules: &BTreeMap<PathBuf, String>,
        keys: &HashMap<String, String>,
    ) {
        let shared = store.results.read().unwrap();
        let mut results = self.results.write().unwrap();
        let root = self.root.join("projects").join(project_key);
        for (node, key) in nodes {
            if let Some(mut result) = shared.get(key).cloned() {
                result.fnode = node.fnode.clone();
                result.revision = node.revision();
                result.dependency_errors = dependency_errors(
                    node,
                    &result.imports,
                    modules,
                    keys,
                    &results,
                    &root,
                    project,
                )
                .unwrap_or_else(|e| vec![e.to_string()]);
                result.certified = result.passed && result.dependency_errors.is_empty();
                results.insert(node.fnode.clone(), result);
            }
        }
    }
    /// Refresh all matching statuses once per graph/evidence version, not per node view.
    pub async fn sync_snapshot(&self, snapshot: &Snapshot) {
        let Some(store) = self
            .certificates(&snapshot.project, &snapshot.project_key, false)
            .await
        else {
            return;
        };
        let generation = store.generation();
        let mut synced = self.synced.lock().await;
        if synced.0 == snapshot.version && synced.1 == generation {
            return;
        }
        let mut nodes: Vec<_> = snapshot.nodes.values().collect();
        nodes.sort_by_key(|n| snapshot.depths.get(&n.fnode).copied().unwrap_or(0));
        self.restore_results(
            &store,
            &snapshot.project,
            &snapshot.project_key,
            nodes
                .into_iter()
                .map(|n| (n.as_ref(), &snapshot.lean_keys[&n.fnode])),
            &snapshot.modules,
            &snapshot.lean_keys,
        );
        *synced = (snapshot.version.clone(), generation);
    }
    async fn cached_input(&self, input: &Input, build: bool) -> Option<CheckResult> {
        if let Some(store) = self
            .certificates(&input.project, &input.project_key, false)
            .await
        {
            let keys = input
                .chain
                .iter()
                .map(|(n, k)| (n.fnode.clone(), k.clone()))
                .collect();
            self.restore_results(
                &store,
                &input.project,
                &input.project_key,
                input.chain.iter().map(|(n, k)| (n.as_ref(), k)),
                &input.modules,
                &keys,
            );
        }
        let (target, key) = input.chain.last()?;
        let result = self
            .cached_or_load(target, key, &input.project_key, build)
            .await?;
        if !result.certified {
            return None;
        }
        let results = self.results.read().ok()?;
        input
            .chain
            .iter()
            .all(|(node, key)| {
                results
                    .get(&node.fnode)
                    .is_some_and(|r| r.certified && r.has_sorry.is_some() && &r.input_key == key)
            })
            .then_some(result)
    }
    pub async fn check(&self, input: Input, build: bool) -> Result<CheckResult> {
        let start = Instant::now();
        let (target, _) = input.chain.last().context("no Lean target")?;
        if let Some(result) = self.cached_input(&input, build).await {
            return Ok(result);
        }
        // Exact input single-flight across branches; unrelated targets still run concurrently.
        let store = self
            .certificates(&input.project, &input.project_key, true)
            .await;
        let _flight = if let Some(store) = &store {
            let key = digest(input.chain.last().unwrap().1.as_bytes());
            let lock = cache::lock(store.root.join(format!("{key}.check.lock"))).await?;
            store
                .refresh(input.chain.iter().map(|(_, k)| k.clone()).collect())
                .await?;
            Some(lock)
        } else {
            None
        };
        let target_id = target.fnode.clone();
        let mut manager = self.manager.lock().await;
        if let Some(result) = self.cached_input(&input, build).await {
            return Ok(result);
        }
        self.prepare_input(&mut manager, &input).await?;
        let result = self.check_chain(&mut manager, &input).await;
        if result.is_err() {
            manager.lsp = None;
            manager.project_key.clear();
        }
        let mut result = result?;
        if build && result.certified {
            build_module(&manager.root, &target.module).await?;
            let artifacts = cache::Artifacts::from_trace(&manager.root, &target.module)
                .filter(|a| a.complete(&manager.root.join(".lake/cache")))
                .context("Lake succeeded without complete native module artifacts")?;
            result.artifacts = Some(artifacts);
            result.built = true;
            result.cache_hit = false;
            self.persist_result(&input, &result, &manager.root).await?;
        }
        result.fnode = target_id;
        result.elapsed_ms = start.elapsed().as_millis();
        self.results
            .write()
            .map_err(|_| anyhow::anyhow!("Lean cache lock poisoned"))?
            .insert(result.fnode.clone(), result.clone());
        self.synced.lock().await.1 = 0;
        Ok(result)
    }
    pub async fn shutdown(&self) {
        if let Some(lsp) = self.manager.lock().await.lsp.take() {
            lsp.server.shutdown().await;
        }
    }
    async fn prepare_input(&self, manager: &mut Manager, input: &Input) -> Result<()> {
        let project_key = input.project_key.clone();
        if manager.project_key != project_key {
            manager.lsp = None;
            manager.sources.clear();
            manager.root = self.root.join("projects").join(&project_key);
            prepare_project(&manager.root, &input.project).await?;
            self.pool.attach(&manager.root, &project_key).await?;
            manager.project_key = project_key;
        }
        refresh_editor_sources(&manager.root, input, &mut manager.sources).await
    }
    async fn check_chain(&self, manager: &mut Manager, input: &Input) -> Result<CheckResult> {
        let (target, _) = input.chain.last().context("no Lean target")?;
        if let Some(result) = self.cached_input(input, false).await {
            return Ok(result);
        }
        let source = target.source("lean").context("node has no Lean block")?;
        if manager.lsp.is_none() {
            manager.lsp = Some(Lsp::start(&manager.root).await?);
        }
        let uri = file_uri(&module_path(&manager.root, &target.module)?)?;
        // Lake prepares the import closure once. Reuse its native artifact
        // metadata, as browser certification does, instead of opening every
        // dependency in a second interactive worker.
        let (diagnostics, imports) = manager
            .lsp
            .as_mut()
            .unwrap()
            .check(&uri, source, &input.environment_key()?)
            .await?;
        self.record_editor_check(&manager.root, input, diagnostics, imports)
            .await
    }
    pub async fn goals(&self, input: Input, line: u32, character: u32) -> Result<Value> {
        let node = input.chain.last().context("no Lean target")?.0.clone();
        self.check(input.clone(), false).await?;
        let mut manager = self.manager.lock().await;
        self.prepare_input(&mut manager, &input).await?;
        if manager.lsp.is_none() {
            manager.lsp = Some(Lsp::start(&manager.root).await?);
        }
        let uri = file_uri(&module_path(&manager.root, &node.module)?)?;
        let dependency_key = input.environment_key()?;
        let lsp = manager.lsp.as_mut().unwrap();
        let result = async {
            // A cached result can outlive its worker; reopen the exact input before querying goals.
            lsp.check(
                &uri,
                node.source("lean").context("node has no Lean block")?,
                &dependency_key,
            )
            .await?;
            lsp.request(
                "$/lean/plainGoal",
                json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
            )
            .await
        }
        .await;
        if result.is_err() {
            manager.lsp = None;
            manager.project_key.clear();
        }
        result
    }

    pub async fn editor_project(&self, input: &Input) -> Result<tempfile::TempDir> {
        let drafts = self.root.join("drafts");
        tokio::fs::create_dir_all(&drafts).await?;
        let directory = tempfile::tempdir_in(drafts)?;
        let root = directory.path();
        prepare_project(root, &input.project).await?;
        let canonical = self.root.join("projects").join(&input.project_key);
        // Package revisions and toolchain are pinned by the project key. Share that
        // cache; cloning Mathlib's entire file tree costs seconds even with APFS COW.
        let packages = canonical.join(".lake/packages");
        tokio::fs::create_dir_all(&packages).await?;
        tokio::fs::create_dir_all(root.join(".lake")).await?;
        tokio::fs::symlink(
            tokio::fs::canonicalize(packages).await?,
            root.join(".lake/packages"),
        )
        .await?;
        self.pool.attach(&canonical, &input.project_key).await?;
        self.pool.attach(root, &input.project_key).await?;
        refresh_editor_sources(root, input, &mut SourceState::new()).await?;
        Ok(directory)
    }
}
pub fn module_path(root: &Path, module: &str) -> Result<PathBuf> {
    Ok(root.join(crate::store::module_file(module, "lean")?))
}
pub type SourceState = HashMap<String, Arc<Node>>;
pub async fn refresh_editor_sources(
    root: &Path,
    input: &Input,
    sources: &mut SourceState,
) -> Result<()> {
    for (node, _) in &input.chain {
        if sources.get(&node.fnode).is_some_and(|old| {
            Arc::ptr_eq(old, node)
                || old.module == node.module && old.source("lean") == node.source("lean")
        }) {
            continue;
        }
        if let Some(source) = node.source("lean") {
            write_source(root, node, source).await?;
        } else {
            for file in [
                module_path(root, &node.module)?,
                root.join(".lake/build/lib/lean")
                    .join(crate::store::module_file(&node.module, "olean")?),
            ] {
                match tokio::fs::remove_file(file).await {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        sources.insert(node.fnode.clone(), node.clone());
    }
    Ok(())
}
pub fn file_uri(path: &Path) -> Result<String> {
    reqwest::Url::from_file_path(path)
        .map(|u| u.to_string())
        .map_err(|_| anyhow::anyhow!("invalid Lean file path"))
}
async fn write_source(root: &Path, node: &Node, source: &str) -> Result<()> {
    let path = module_path(root, &node.module)?;
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(path, source).await?;
    Ok(())
}
pub async fn prepare_project(root: &Path, project: &LeanProject) -> Result<()> {
    project.validate()?;
    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(
        root.join("lean-toolchain"),
        format!("{}\n", project.toolchain),
    )
    .await?;
    tokio::fs::write(root.join(project.lakefile_name()), &project.lakefile).await?;
    if let Some(manifest) = &project.manifest {
        tokio::fs::write(root.join("lake-manifest.json"), manifest).await?;
    }
    for (name, content) in &project.files {
        let path = root.join(name);
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(path, content).await?;
    }
    Ok(())
}
async fn verify_manifest(root: &Path, project: &LeanProject) -> Result<()> {
    let Some(expected) = &project.manifest else {
        return Ok(());
    };
    let resolution = |text: &str| -> Result<BTreeMap<String, Value>> {
        let value: Value = serde_json::from_str(text)?;
        value["packages"]
            .as_array()
            .context("invalid Lake manifest")?
            .iter()
            .map(|p| {
                Ok((
                    p["name"].as_str().context("package name missing")?.into(),
                    json!([p["type"], p["url"], p["rev"], p["subDir"]]),
                ))
            })
            .collect()
    };
    let actual = tokio::fs::read_to_string(root.join("lake-manifest.json")).await?;
    if resolution(&actual)? != resolution(expected)? {
        bail!("Lake changed the locked library revisions; update the versioned project manifest before checking");
    }
    Ok(())
}

async fn build_module(root: &Path, module: &str) -> Result<()> {
    let target = format!("+{module}");
    let mut process = spawn(root, &["build", &target])?;
    drop(process.child.stdin.take());
    let mut stdout = process
        .child
        .stdout
        .take()
        .context("Lake stdout unavailable")?;
    let drain =
        tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await });
    let status = tokio::time::timeout(timeout()?, process.child.wait())
        .await
        .context("Lake build timed out")??;
    drain.await??;
    if !status.success() {
        bail!("Lake build failed with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires native Lean and Lake"]
    async fn editor_evidence_certifies_imports_without_a_second_checker() {
        let cache = tempfile::tempdir().unwrap();
        let service = LeanService::new(cache.path().to_path_buf()).unwrap();
        let mut dependency = Node::new("Editor dependency".into()).unwrap();
        dependency.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: "import Lean\ntheorem depTruth : True := by sorry\n".into(),
            ..Default::default()
        });
        let mut target = Node::new("Editor target".into()).unwrap();
        target.depens.push(dependency.fnode.clone());
        target.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: format!(
                "import Std\nimport {}\n-- sorry in prose is not a proof gap\ndef message := \"sorry\"\ntheorem targetTruth : True := depTruth\n",
                dependency.module
            ),
            ..Default::default()
        });
        let input = Input {
            project: LeanProject::default().into(),
            project_key: LeanProject::default().key(),
            modules: [&dependency, &target]
                .into_iter()
                .map(|n| {
                    (
                        crate::store::module_file(&n.module, "lean").unwrap(),
                        n.fnode.clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into(),
            chain: vec![
                (Arc::new(dependency.clone()), "dep-key".into()),
                (Arc::new(target.clone()), "target-key".into()),
            ],
        };
        let draft = service.editor_project(&input).await.unwrap();
        let mut editor = Lsp::start(draft.path()).await.unwrap();
        let uri = file_uri(&module_path(draft.path(), &target.module).unwrap()).unwrap();
        let (diagnostics, imports) = editor
            .check(&uri, target.source("lean").unwrap(), "deps")
            .await
            .unwrap();
        let result = service
            .record_editor_check(draft.path(), &input, diagnostics, imports)
            .await
            .unwrap();
        assert!(result.certified, "{result:?}");
        assert_eq!(result.has_sorry, Some(false));
        assert_eq!(service.formal_status(&target, "target-key"), "verified");
        assert_eq!(service.formal_status(&target, "stale-key"), "unverified");
        assert_eq!(service.formal_status(&dependency, "dep-key"), "unverified");
        assert_eq!(
            service
                .cached(&dependency.fnode, "dep-key", &dependency.revision(), false)
                .unwrap()
                .has_sorry,
            Some(true)
        );
        let empty = Node::new("No Lean".into()).unwrap();
        assert_eq!(service.formal_status(&empty, "unused"), "no_code");
        assert!(
            service
                .cached(&dependency.fnode, "dep-key", &dependency.revision(), false)
                .unwrap()
                .certified
        );
        assert!(service.check(input.clone(), false).await.unwrap().cache_hit);
        assert!(
            service.manager.lock().await.lsp.is_none(),
            "the CLI must reuse editor evidence without creating a checker"
        );
        assert!(service.check(input.clone(), true).await.unwrap().built);
        assert!(
            service.manager.lock().await.lsp.is_none(),
            "building artifacts must not repeat an already certified LSP check"
        );
        let mut wrong = input.clone();
        Arc::make_mut(&mut wrong.chain.last_mut().unwrap().0)
            .depens
            .clear();
        wrong.chain.last_mut().unwrap().1 = "wrong-deps".into();
        let (diagnostics, imports) = editor
            .check(&uri, target.source("lean").unwrap(), "deps")
            .await
            .unwrap();
        let invalid = service
            .record_editor_check(draft.path(), &wrong, diagnostics, imports)
            .await
            .unwrap();
        assert!(
            invalid.passed && !invalid.certified,
            "native success cannot waive the graph/import check"
        );
        editor.server.shutdown().await;
        service.shutdown().await;
        drop(service);
        let reopened = LeanService::new(cache.path().to_path_buf()).unwrap();
        assert!(reopened.cached_input(&input, false).await.is_some());
        assert_eq!(reopened.formal_status(&dependency, "dep-key"), "unverified");
        // The latest target certificate was deliberately invalidated above.
        // Its earlier successful persisted certificate is still keyed correctly.
        assert_eq!(reopened.formal_status(&target, "target-key"), "verified");
    }

    #[tokio::test]
    #[ignore = "requires native Lean and Lake"]
    async fn restored_artifacts_refresh_the_entire_dependency_status() {
        let cache = tempfile::tempdir().unwrap();
        let service = LeanService::new(cache.path().to_path_buf()).unwrap();
        let marker = cache.path().join("compiled");
        let mut leaf = Node::new("Complete leaf".into()).unwrap();
        let mut gap = Node::new("Pending leaf".into()).unwrap();
        let mut middle = Node::new("Complete dependent proof".into()).unwrap();
        let mut target = Node::new("Root".into()).unwrap();
        middle.depens = vec![leaf.fnode.clone(), gap.fnode.clone()];
        target.depens = vec![middle.fnode.clone()];
        for (node, source) in [
            (&mut leaf, format!("import Lean\nrun_cmd Lean.Elab.Command.liftIO <| IO.FS.writeFile {} \"compiled\"\ntheorem leafTruth : True := by trivial\n", serde_json::to_string(&marker.to_string_lossy()).unwrap())),
            (&mut gap, "module\npublic import Lean\nset_option warn.sorry false\npublic theorem gapTruth : True := by sorry\n".into()),
        ] {
            node.blocks.push(crate::store::Block {srctype: "lean".into(), content: source, ..Default::default()});
        }
        middle.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: format!(
                "import {}\nimport {}\ntheorem middleTruth : True := gapTruth\n",
                leaf.module, gap.module
            ),
            ..Default::default()
        });
        target.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: format!(
                "import {}\ntheorem rootTruth : True := middleTruth\n",
                middle.module
            ),
            ..Default::default()
        });
        let input = Input {
            project: LeanProject::default().into(),
            project_key: LeanProject::default().key(),
            modules: [&leaf, &gap, &middle, &target]
                .into_iter()
                .map(|node| {
                    (
                        crate::store::module_file(&node.module, "lean").unwrap(),
                        node.fnode.clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into(),
            chain: [&leaf, &gap, &middle, &target]
                .into_iter()
                .map(|node| (Arc::new(node.clone()), node.fnode.clone()))
                .collect(),
        };
        let draft = service.editor_project(&input).await.unwrap();
        let mut editor = Lsp::start(draft.path()).await.unwrap();
        let uri = file_uri(&module_path(draft.path(), &target.module).unwrap()).unwrap();
        let (diagnostics, imports) = editor
            .check(&uri, target.source("lean").unwrap(), "deps")
            .await
            .unwrap();
        assert!(
            service
                .record_editor_check(draft.path(), &input, diagnostics, imports)
                .await
                .unwrap()
                .certified
        );
        assert!(marker.exists());
        // Model older certificates: the parent and direct import are complete,
        // but transitive imports have lost their warning evidence.
        for node in [&leaf, &gap] {
            let mut result = service.results.read().unwrap()[&node.fnode].clone();
            result.has_sorry = None;
            let shared = service
                .certificates(&input.project, &input.project_key, false)
                .await
                .unwrap();
            shared.results.write().unwrap().remove(&result.input_key);
            std::fs::remove_file(
                shared
                    .root
                    .join(format!("{}.json", digest(result.input_key.as_bytes()))),
            )
            .unwrap();
            service
                .results
                .write()
                .unwrap()
                .insert(node.fnode.clone(), result);
        }
        assert!(service.cached_input(&input, false).await.is_none());
        editor.server.shutdown().await;
        drop(draft);
        std::fs::remove_file(&marker).unwrap();
        // Only open the root; Lake restores all imports from the local cache.
        let result = service.check(input.clone(), false).await.unwrap();
        assert!(result.certified, "{result:?}");
        assert!(!result.cache_hit);
        assert!(
            !marker.exists(),
            "status recovery must not recompile imports"
        );
        for node in [&leaf, &middle, &target] {
            assert_eq!(
                service.formal_status(node, &node.fnode),
                "verified",
                "{}",
                node.title
            );
        }
        assert_eq!(
            service
                .cached(&gap.fnode, &gap.fnode, &gap.revision(), false)
                .unwrap()
                .has_sorry,
            Some(true)
        );
        assert_eq!(service.formal_status(&gap, &gap.fnode), "unverified");
        assert!(service.check(input.clone(), false).await.unwrap().cache_hit);
        let root = service.manager.lock().await.root.clone();
        assert_eq!(
            artifact_has_sorry(&root.join(".lake/build/lib/lean"), &leaf.module).await,
            None
        );
        service.shutdown().await;
        drop(service);
        let reopened = LeanService::new(cache.path().to_path_buf()).unwrap();
        assert!(reopened.cached_input(&input, false).await.is_some());
        assert_eq!(reopened.formal_status(&leaf, &leaf.fnode), "verified");
        assert_eq!(reopened.formal_status(&gap, &gap.fnode), "unverified");
    }

    #[tokio::test]
    #[ignore = "requires native Lean and Lake"]
    async fn lake_reuses_editor_artifacts_after_the_draft_is_removed() {
        let cache = tempfile::tempdir().unwrap();
        let service = LeanService::new(cache.path().to_path_buf()).unwrap();
        let marker = cache.path().join("compiled");
        let mut node = Node::new("Cached editor artifact".into()).unwrap();
        node.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: format!("import Lean\nrun_cmd Lean.Elab.Command.liftIO <| IO.FS.writeFile {} \"compiled\"\ntheorem cachedEditor : True := by trivial\n", serde_json::to_string(&marker.to_string_lossy()).unwrap()),
            ..Default::default()
        });
        let input = Input {
            project: LeanProject::default().into(),
            project_key: LeanProject::default().key(),
            modules: [(
                crate::store::module_file(&node.module, "lean").unwrap(),
                node.fnode.clone(),
            )]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
            .into(),
            chain: vec![(Arc::new(node.clone()), "input".into())],
        };
        let first = service.editor_project(&input).await.unwrap();
        build_module(first.path(), &node.module).await.unwrap();
        assert!(marker.is_file());
        drop(first);
        std::fs::remove_file(&marker).unwrap();
        let second = service.editor_project(&input).await.unwrap();
        assert!(!second.path().join(".lake/build").exists());
        build_module(second.path(), &node.module).await.unwrap();
        assert!(
            !marker.exists(),
            "restoring Lake artifacts must not execute the compiler again"
        );
        assert!(artifact_parts(second.path(), &node.module).is_some());
        assert!(!second
            .path()
            .join(".lake/build/lib/lean")
            .join(crate::store::module_file(&node.module, "olean").unwrap())
            .exists());
        node.blocks[0].content.push_str("\n-- new input\n");
        write_source(second.path(), &node, node.source("lean").unwrap())
            .await
            .unwrap();
        build_module(second.path(), &node.module).await.unwrap();
        assert!(
            marker.exists(),
            "changed source must miss the old artifact cache"
        );
    }

    #[tokio::test]
    async fn editor_shares_libraries_without_waiting_for_the_compiler() {
        let cache = tempfile::tempdir().unwrap();
        let service = LeanService::new(cache.path().to_path_buf()).unwrap();
        let deleted = Node::new("Deleted Lean block".into()).unwrap();
        let deleted_artifact = PathBuf::from(".lake/build/lib/lean")
            .join(crate::store::module_file(&deleted.module, "olean").unwrap());
        let input = Input {
            project: LeanProject::default().into(),
            project_key: LeanProject::default().key(),
            modules: Default::default(),
            chain: vec![(Arc::new(deleted), "deleted".into())],
        };
        let canonical = cache.path().join("projects").join(&input.project_key);
        tokio::fs::create_dir_all(canonical.join(".lake/packages/example"))
            .await
            .unwrap();
        tokio::fs::write(
            canonical.join(".lake/packages/example/library.olean"),
            "shared",
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(canonical.join(".lake/build"))
            .await
            .unwrap();
        tokio::fs::write(canonical.join(".lake/build/local.olean"), "original")
            .await
            .unwrap();
        std::fs::create_dir_all(canonical.join(&deleted_artifact).parent().unwrap()).unwrap();
        std::fs::write(canonical.join(&deleted_artifact), "stale artifact").unwrap();
        let busy = service.manager.lock().await;
        let draft = tokio::time::timeout(Duration::from_secs(2), service.editor_project(&input))
            .await
            .unwrap()
            .unwrap();
        assert!(draft.path().join(".lake/packages").is_symlink());
        assert!(!draft.path().join(".lake/build").exists());
        drop(draft);
        drop(busy);
        let draft = service.editor_project(&input).await.unwrap();
        assert!(!draft.path().join(&deleted_artifact).exists());
        tokio::fs::create_dir_all(draft.path().join(".lake/build"))
            .await
            .unwrap();
        tokio::fs::write(draft.path().join(".lake/build/local.olean"), "draft")
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(canonical.join(".lake/build/local.olean"))
                .await
                .unwrap(),
            "original"
        );
        drop(draft);
        assert!(canonical
            .join(".lake/packages/example/library.olean")
            .is_file());
    }

    #[tokio::test]
    async fn materialization_only_writes_changed_saved_sources() {
        let root = tempfile::tempdir().unwrap();
        let mut node = Node::new("A".into()).unwrap();
        node.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: "-- saved\n".into(),
            ..Default::default()
        });
        let mut input = Input {
            project: LeanProject::default().into(),
            project_key: LeanProject::default().key(),
            chain: vec![(Arc::new(node), "a".into())],
            modules: Default::default(),
        };
        let mut sources = SourceState::new();
        refresh_editor_sources(root.path(), &input, &mut sources)
            .await
            .unwrap();
        let file = module_path(root.path(), &input.chain[0].0.module).unwrap();
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        refresh_editor_sources(root.path(), &input, &mut sources)
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            modified
        );
        let original = input.clone();
        Arc::make_mut(&mut input.chain[0].0).blocks[0].content = "-- changed\n".into();
        refresh_editor_sources(root.path(), &input, &mut sources)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "-- changed\n");
        assert_eq!(original.chain[0].0.source("lean"), Some("-- saved\n"));
        assert!(Arc::ptr_eq(
            &sources[&input.chain[0].0.fnode],
            &input.chain[0].0
        ));
        let artifact = root
            .path()
            .join(".lake/build/lib/lean")
            .join(crate::store::module_file(&input.chain[0].0.module, "olean").unwrap());
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        std::fs::write(&artifact, "old compiled module").unwrap();
        Arc::make_mut(&mut input.chain[0].0).blocks.clear();
        refresh_editor_sources(root.path(), &input, &mut sources)
            .await
            .unwrap();
        assert!(!file.exists());
        assert!(!artifact.exists());
        // A fresh session may also encounter artifacts from before the deletion.
        refresh_editor_sources(root.path(), &input, &mut SourceState::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_changed_library_resolution_cannot_be_cached() {
        let root = tempfile::tempdir().unwrap();
        let manifest = |rev: &str| {
            json!({"packages":[{"name":"Example","type":"git","url":"https://example.org/lib.git","rev":rev}]}).to_string()
        };
        let project = LeanProject {
            manifest: Some(manifest("before")),
            ..Default::default()
        };
        tokio::fs::write(root.path().join("lake-manifest.json"), manifest("before"))
            .await
            .unwrap();
        assert!(verify_manifest(root.path(), &project).await.is_ok());
        tokio::fs::write(root.path().join("lake-manifest.json"), manifest("after"))
            .await
            .unwrap();
        assert!(verify_manifest(root.path(), &project).await.is_err());
    }
}
