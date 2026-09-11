//! Native Lean LSP sessions plus Lake's existing incremental artifact store.
pub(crate) mod editor;
use crate::store::{digest, LeanProject, Node, Snapshot};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::RwLock,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, Mutex},
};

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
const MAX_MESSAGE: usize = 32 * 1024 * 1024;

/// Kill the entire Lake/Lean process group on timeout, disconnect or shutdown.
pub struct Process {
    pub child: Child,
    pid: i32,
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.pid > 0 {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
    }
}
pub fn command(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("lake");
    c.args(args)
        .current_dir(root)
        // Lake keys artifacts by compiler inputs, including transitive imports.
        // Editor roots link this directory to the branch's canonical project.
        .env("LAKE_ARTIFACT_CACHE", "true")
        .env("LAKE_RESTORE_ARTIFACTS", "true")
        .env("LAKE_CACHE_DIR", root.join(".lake/cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    c.process_group(0);
    // Compiler metaprograms run as the local author, without database credentials.
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("MDC_")
            || name.contains("TOKEN")
            || name.contains("PASSWORD")
            || name.contains("SECRET")
            || name.ends_with("API_KEY")
        {
            c.env_remove(key);
        }
    }
    c
}
pub fn spawn(root: &Path, args: &[&str]) -> Result<Process> {
    let child = command(root, args)
        .spawn()
        .context("start Lake; install the pinned Lean toolchain first")?;
    let pid = child.id().context("compiler process has no PID")? as i32;
    Ok(Process { child, pid })
}
/// One transport for CLI and browser clients. Dropping it asks Lean's watchdog
/// to terminate its workers, which use separate process groups of their own.
pub struct Server {
    input: mpsc::Sender<String>,
    output: mpsc::Receiver<Result<String>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub fn start(root: &Path) -> Result<Self> {
        let mut process = spawn(root, &["serve"])?;
        let mut writer = process
            .child
            .stdin
            .take()
            .context("Lean stdin unavailable")?;
        let mut reader = BufReader::new(
            process
                .child
                .stdout
                .take()
                .context("Lean stdout unavailable")?,
        );
        let (input, mut incoming) = mpsc::channel::<String>(32);
        let (outgoing, output) = mpsc::channel(32);
        let errors = outgoing.clone();
        // Never cancel a partially read frame. After the client disconnects,
        // continue draining stdout until Lean finishes its shutdown handshake.
        tokio::spawn(async move {
            loop {
                let message = read_frame(&mut reader).await;
                let failed = message.is_err();
                let _ = outgoing.send(message).await;
                if failed {
                    break;
                }
            }
        });
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = errors.closed() => {},
                _ = async {
                    while let Some(text) = incoming.recv().await {
                        if let Err(error) = write_frame(&mut writer, &text).await {
                            let _ = errors.send(Err(error)).await;
                            break;
                        }
                    }
                } => {},
            }
            // Lean processes these in order. Keep both pipes alive until it has
            // killed and reaped the file workers; dropping stdout first causes EPIPE.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                write_frame(
                    &mut writer,
                    r#"{"jsonrpc":"2.0","id":"mdc-shutdown","method":"shutdown","params":null}"#,
                )
                .await?;
                write_frame(&mut writer, r#"{"jsonrpc":"2.0","method":"exit"}"#).await?;
                process.child.wait().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            // Process::drop is the fallback for a stuck or crashed server.
        });
        Ok(Self {
            input,
            output,
            task,
        })
    }
    pub async fn send(&self, text: String) -> Result<()> {
        self.input
            .send(text)
            .await
            .context("Lean server closed its input")
    }
    pub async fn receive(&mut self) -> Result<String> {
        self.output
            .recv()
            .await
            .context("Lean server closed its output")?
    }
    pub async fn shutdown(self) {
        drop(self.input);
        drop(self.output);
        let _ = self.task.await;
    }
}

/// Keep editor RPC payloads opaque: Lean's tagged expressions can exceed JSON
/// tree deserializers' recursion limits even for ordinary mathematical terms.
pub async fn read_frame(reader: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> Result<String> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let n = (&mut *reader)
            .take((8193 - header_bytes) as u64)
            .read_line(&mut line)
            .await?;
        if n == 0 {
            bail!("Lean server exited");
        }
        header_bytes += n;
        if header_bytes > 8192 {
            bail!("Lean protocol header too large");
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
    }
    let length = length.context("Lean response omitted Content-Length")?;
    if length > MAX_MESSAGE {
        bail!("Lean protocol message too large");
    }
    let mut data = vec![0; length];
    reader.read_exact(&mut data).await?;
    Ok(String::from_utf8(data)?)
}
pub async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    data: &str,
) -> Result<()> {
    if data.len() > MAX_MESSAGE {
        bail!("Lean request too large");
    }
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", data.len()).as_bytes())
        .await?;
    writer.write_all(data.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
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
    pub built: bool,
    pub cache_hit: bool,
    pub diagnostics: Vec<Value>,
    pub imports: Vec<Value>,
    pub dependency_errors: Vec<String>,
    pub elapsed_ms: u128,
}
#[derive(Clone)]
pub struct Input {
    pub project: LeanProject,
    pub chain: Vec<(Node, String)>,
}
impl Input {
    pub fn environment_key(&self) -> Result<String> {
        let node = &self.chain.last().context("no Lean target")?.0;
        Ok(digest(&serde_json::to_vec(&(
            self.project.key(),
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
    sources: HashMap<String, (String, String)>,
}
fn dependency_errors(
    node: &Node,
    imports: &[Value],
    known: &BTreeMap<PathBuf, String>,
    keys: &BTreeMap<String, String>,
    results: &HashMap<String, CheckResult>,
) -> Result<Vec<String>> {
    let mut errors = vec![];
    let mut imported = BTreeSet::new();
    for import in imports {
        let module = import["module"]["name"]
            .as_str()
            .context("Lean import omitted module name")?;
        if module.starts_with("Lib.") {
            if let Some(id) = known.get(&crate::store::module_file(module, "lean")?) {
                imported.insert(id.clone());
            } else {
                errors.push(format!("managed import {module} is not declared in dep"));
            }
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
    // ponytail: one CLI compiler per branch; add workers if concurrent check throughput requires it.
    manager: Mutex<Manager>,
    results: RwLock<HashMap<String, CheckResult>>,
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
        let known: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, _)| {
                Ok((
                    crate::store::module_file(&n.module, "lean")?,
                    n.fnode.clone(),
                ))
            })
            .collect::<Result<_>>()?;
        let keys: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, k)| (n.fnode.clone(), k.clone()))
            .collect();
        let nodes: HashMap<_, _> = input.chain.iter().map(|(n, _)| (&n.fnode, n)).collect();
        let mut observed =
            HashMap::from([(target.fnode.clone(), (diagnostics.clone(), imports.clone()))]);
        let mut pending = if diagnostics.iter().any(|d| d["severity"] == 1) {
            vec![]
        } else {
            imports
        };
        while let Some(import) = pending.pop() {
            let module = import["module"]["name"]
                .as_str()
                .context("Lean import omitted module name")?;
            if !module.starts_with("Lib.") {
                continue;
            }
            let Some(id) = known.get(&crate::store::module_file(module, "lean")?) else {
                continue;
            };
            if observed.contains_key(id) {
                continue;
            }
            let node = nodes[id];
            let Some(source) = node.source("lean") else {
                continue;
            };
            if tokio::fs::read_to_string(module_path(root, &node.module)?).await? != source {
                bail!("editor dependency source changed during checking");
            }
            let base = root.join(".lake/build/lib/lean");
            if !base
                .join(crate::store::module_file(&node.module, "olean")?)
                .is_file()
            {
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
            observed.insert(id.clone(), (vec![], imports));
        }
        let mut final_result = None;
        for (node, key) in &input.chain {
            if let Some(result) = self
                .cached_or_load(node, key, &input.project, false)
                .await
                .filter(|r| r.certified)
            {
                final_result = Some(result);
                continue;
            }
            let Some((diagnostics, imports)) = observed.remove(&node.fnode) else {
                continue;
            };
            let passed = !diagnostics.iter().any(|d| d["severity"] == 1);
            let errors =
                dependency_errors(node, &imports, &known, &keys, &self.results.read().unwrap())?;
            let result = CheckResult {
                fnode: node.fnode.clone(),
                revision: node.revision(),
                input_key: key.clone(),
                passed,
                certified: passed && errors.is_empty(),
                built: false,
                cache_hit: false,
                diagnostics,
                imports,
                dependency_errors: errors,
                elapsed_ms: started.elapsed().as_millis(),
            };
            self.persist_result(&result).await?;
            self.results
                .write()
                .unwrap()
                .insert(node.fnode.clone(), result.clone());
            final_result = Some(result);
        }
        final_result
            .filter(|r| r.fnode == target.fnode)
            .context("editor check produced no target result")
    }
    pub fn new(root: PathBuf) -> Result<Self> {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{OpenOptionsExt, PermissionsExt},
        };
        std::fs::create_dir_all(&root)?;
        let lease = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("service.lock"))?;
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("this database branch cache is already owned by another service");
        }
        // A new owner is not serving until its HTTP listener has been bound.
        lease.set_len(0)?;
        lease.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            manager: Mutex::new(Manager {
                project_key: String::new(),
                root: root.clone(),
                lsp: None,
                sources: HashMap::new(),
            }),
            root,
            results: RwLock::new(HashMap::new()),
            _lease: lease,
        })
    }
    pub fn cached(&self, id: &str, key: &str, revision: &str, build: bool) -> Option<CheckResult> {
        self.results
            .read()
            .ok()?
            .get(id)
            .filter(|r| r.input_key == key && (!build || r.built))
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
        project: &LeanProject,
        build: bool,
    ) -> Option<CheckResult> {
        let artifact = self
            .root
            .join("projects")
            .join(project.key())
            .join(".lake/build/lib/lean")
            .join(crate::store::module_file(&node.module, "olean").ok()?);
        if let Some(mut result) = self.cached(&node.fnode, key, &node.revision(), false) {
            if result.built && !artifact.is_file() {
                result.built = false;
                self.results
                    .write()
                    .ok()?
                    .insert(node.fnode.clone(), result.clone());
            }
            return (!build || result.built).then_some(result);
        }
        let bytes = tokio::fs::read(
            self.root
                .join("checks-v1")
                .join(format!("{}.json", node.fnode)),
        )
        .await
        .ok()?;
        let mut result: CheckResult = serde_json::from_slice(&bytes).ok()?;
        if result.fnode != node.fnode
            || result.input_key != key
            || !result.certified
            || !result.passed
        {
            return None;
        }
        result.built &= artifact.is_file();
        self.results
            .write()
            .ok()?
            .insert(node.fnode.clone(), result);
        self.cached(&node.fnode, key, &node.revision(), build)
    }
    async fn persist_result(&self, result: &CheckResult) -> Result<()> {
        if !result.certified {
            return Ok(());
        }
        let root = self.root.join("checks-v1");
        tokio::fs::create_dir_all(&root).await?;
        let temporary = root.join(format!("{}.{}.tmp", result.fnode, uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, serde_json::to_vec(result)?).await?;
        tokio::fs::rename(temporary, root.join(format!("{}.json", result.fnode))).await?;
        Ok(())
    }
    pub async fn check(&self, input: Input, build: bool) -> Result<CheckResult> {
        let start = Instant::now();
        let (target, key) = input.chain.last().context("no Lean target")?;
        if let Some(result) = self
            .cached_or_load(target, key, &input.project, build)
            .await
        {
            return Ok(result);
        }
        let target_id = target.fnode.clone();
        let target_key = key.clone();
        let mut manager = self.manager.lock().await;
        if let Some(result) = self.cached(&target.fnode, &target_key, &target.revision(), build) {
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
            let artifact = manager
                .root
                .join(".lake/build/lib/lean")
                .join(crate::store::module_file(&target.module, "olean")?);
            if !artifact.is_file() {
                bail!("Lake succeeded without producing the target olean");
            }
            result.built = true;
            result.cache_hit = false;
            self.persist_result(&result).await?;
        }
        result.fnode = target_id;
        result.elapsed_ms = start.elapsed().as_millis();
        self.results
            .write()
            .map_err(|_| anyhow::anyhow!("Lean cache lock poisoned"))?
            .insert(result.fnode.clone(), result.clone());
        Ok(result)
    }
    pub async fn shutdown(&self) {
        if let Some(lsp) = self.manager.lock().await.lsp.take() {
            lsp.server.shutdown().await;
        }
    }
    async fn prepare_input(&self, manager: &mut Manager, input: &Input) -> Result<()> {
        let project_key = input.project.key();
        if manager.project_key != project_key {
            manager.lsp = None;
            manager.sources.clear();
            manager.root = self.root.join("projects").join(&project_key);
            prepare_project(&manager.root, &input.project).await?;
            manager.project_key = project_key;
        }
        for (node, _) in &input.chain {
            if let Some(source) = node.source("lean") {
                if manager
                    .sources
                    .get(&node.fnode)
                    .is_none_or(|(module, s)| module != &node.module || s != source)
                {
                    write_source(&manager.root, node, source).await?;
                    manager
                        .sources
                        .insert(node.fnode.clone(), (node.module.clone(), source.into()));
                }
            }
        }
        Ok(())
    }
    async fn check_chain(&self, manager: &mut Manager, input: &Input) -> Result<CheckResult> {
        let (target, key) = input.chain.last().context("no Lean target")?;
        if let Some(result) = self
            .cached_or_load(target, key, &input.project, false)
            .await
        {
            return Ok(result);
        }
        let known: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, _)| {
                Ok((
                    crate::store::module_file(&n.module, "lean")?,
                    n.fnode.clone(),
                ))
            })
            .collect::<Result<_>>()?;
        let keys: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, k)| (n.fnode.clone(), k.clone()))
            .collect();
        let mut final_result = None;
        for (node, key) in &input.chain {
            if let Some(cached) = self.cached_or_load(node, key, &input.project, false).await {
                final_result = Some(cached);
                continue;
            }
            let Some(source) = node.source("lean") else {
                let result = CheckResult {
                    fnode: node.fnode.clone(),
                    revision: node.revision(),
                    input_key: key.clone(),
                    passed: false,
                    certified: false,
                    built: false,
                    cache_hit: false,
                    diagnostics: vec![],
                    imports: vec![],
                    dependency_errors: vec![format!("dependency {} has no Lean block", node.title)],
                    elapsed_ms: 0,
                };
                self.results
                    .write()
                    .unwrap()
                    .insert(node.fnode.clone(), result.clone());
                final_result = Some(result);
                continue;
            };
            if manager.lsp.is_none() {
                manager.lsp = Some(Lsp::start(&manager.root).await?);
            }
            let uri = file_uri(&module_path(&manager.root, &node.module)?)?;
            let dependency_key = digest(&serde_json::to_vec(
                &node
                    .depens
                    .iter()
                    .map(|id| (id, &keys[id]))
                    .collect::<BTreeMap<_, _>>(),
            )?);
            let (diagnostics, imports) = manager
                .lsp
                .as_mut()
                .unwrap()
                .check(&uri, source, &dependency_key)
                .await?;
            verify_manifest(&manager.root, &input.project).await?;
            let passed = !diagnostics.iter().any(|d| d["severity"] == 1);
            let errors =
                dependency_errors(node, &imports, &known, &keys, &self.results.read().unwrap())?;
            let certified = passed && errors.is_empty();
            let result = CheckResult {
                fnode: node.fnode.clone(),
                revision: node.revision(),
                input_key: key.clone(),
                passed,
                certified,
                built: false,
                cache_hit: false,
                diagnostics,
                imports,
                dependency_errors: errors,
                elapsed_ms: 0,
            };
            self.persist_result(&result).await?;
            self.results
                .write()
                .unwrap()
                .insert(node.fnode.clone(), result.clone());
            final_result = Some(result);
        }
        final_result.context("Lean check produced no result")
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
        let keys: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, k)| (n.fnode.clone(), k.clone()))
            .collect();
        let dependency_key = digest(&serde_json::to_vec(
            &node
                .depens
                .iter()
                .map(|id| (id, &keys[id]))
                .collect::<BTreeMap<_, _>>(),
        )?);
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
        for (node, _) in &input.chain {
            if let Some(source) = node.source("lean") {
                write_source(root, node, source).await?;
            }
        }
        let canonical = self.root.join("projects").join(input.project.key());
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
        let artifacts = canonical.join(".lake/cache");
        tokio::fs::create_dir_all(&artifacts).await?;
        tokio::fs::symlink(
            tokio::fs::canonicalize(artifacts).await?,
            root.join(".lake/cache"),
        )
        .await?;
        // Never wait for a running CLI check. Lake can rebuild the isolated managed
        // modules from this snapshot while still using the shared external libraries.
        if let Ok(_manager) = self.manager.try_lock() {
            if canonical.join(".lake/build").exists() {
                let mut copy = Command::new("cp");
                #[cfg(target_os = "macos")]
                copy.arg("-cR");
                #[cfg(not(target_os = "macos"))]
                copy.args(["-R", "--reflink=auto"]);
                let status = copy
                    .arg(canonical.join(".lake/build"))
                    .arg(root.join(".lake"))
                    .kill_on_drop(true)
                    .status()
                    .await?;
                if !status.success() {
                    bail!("copying managed module artifacts failed");
                }
            }
        }
        Ok(directory)
    }
}
pub fn module_path(root: &Path, module: &str) -> Result<PathBuf> {
    Ok(root.join(crate::store::module_file(module, "lean")?))
}
pub async fn refresh_editor_sources(root: &Path, input: &Input) -> Result<()> {
    for (node, _) in &input.chain {
        if let Some(source) = node.source("lean") {
            let path = module_path(root, &node.module)?;
            match tokio::fs::read_to_string(path).await {
                Ok(existing) if existing == source => continue,
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into())
                }
                _ => write_source(root, node, source).await?,
            }
        }
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
    tokio::fs::create_dir_all(root.join("Lib")).await?;
    tokio::fs::write(
        root.join("lean-toolchain"),
        format!("{}\n", project.toolchain),
    )
    .await?;
    let mut config: toml::Table = toml::from_str(&project.lakefile)?;
    // Older pinned Lake releases do not read LAKE_RESTORE_ARTIFACTS. Keep
    // standard artifact paths for metadata consumers without changing user data.
    config.insert("restoreAllArtifacts".into(), toml::Value::Boolean(true));
    tokio::fs::write(root.join("lakefile.toml"), toml::to_string(&config)?).await?;
    if let Some(manifest) = &project.manifest {
        tokio::fs::write(root.join("lake-manifest.json"), manifest).await?;
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
            content: "import Lean\ntheorem depTruth : True := by trivial\n".into(),
            ..Default::default()
        });
        let mut target = Node::new("Editor target".into()).unwrap();
        target.depens.push(dependency.fnode.clone());
        target.blocks.push(crate::store::Block {
            srctype: "lean".into(),
            content: format!(
                "import Std\nimport {}\ntheorem targetTruth : True := depTruth\n",
                dependency.module
            ),
            ..Default::default()
        });
        let input = Input {
            project: LeanProject::default(),
            chain: vec![
                (dependency.clone(), "dep-key".into()),
                (target.clone(), "target-key".into()),
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
        wrong.chain.last_mut().unwrap().0.depens.clear();
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
            project: LeanProject::default(),
            chain: vec![(node.clone(), "input".into())],
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
        assert!(second
            .path()
            .join(".lake/build/lib/lean")
            .join(crate::store::module_file(&node.module, "olean").unwrap())
            .is_file());
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
    async fn editor_frames_preserve_deep_json_and_bound_protocol_input() {
        let text = format!("{}0{}", "[".repeat(2048), "]".repeat(2048));
        let mut wire = Vec::new();
        write_frame(&mut wire, &text).await.unwrap();
        let mut reader = BufReader::new(wire.as_slice());
        assert_eq!(read_frame(&mut reader).await.unwrap(), text);
        for invalid in [
            "x".repeat(8193),
            format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1),
            "Content-Length: 10\r\n\r\n{}".into(),
        ] {
            assert!(read_frame(&mut BufReader::new(invalid.as_bytes()))
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn editor_shares_libraries_without_waiting_for_the_compiler() {
        let cache = tempfile::tempdir().unwrap();
        let service = LeanService::new(cache.path().to_path_buf()).unwrap();
        let input = Input {
            project: LeanProject::default(),
            chain: vec![],
        };
        let canonical = cache.path().join("projects").join(input.project.key());
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
