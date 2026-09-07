//! Native Lean LSP sessions plus Lake's existing incremental artifact store.
use crate::store::{digest, LeanProject, Node, Snapshot};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
    sync::RwLock,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

const TIMEOUT: Duration = Duration::from_secs(300);
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
pub async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
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
    Ok(serde_json::from_slice(&data)?)
}
pub async fn write_message(writer: &mut ChildStdin, value: &Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_MESSAGE {
        bail!("Lean request too large");
    }
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", data.len()).as_bytes())
        .await?;
    writer.write_all(&data).await?;
    writer.flush().await?;
    Ok(())
}

struct Document {
    version: u64,
    source: String,
    dependency_key: String,
}
struct Lsp {
    _process: Process,
    writer: ChildStdin,
    reader: BufReader<ChildStdout>,
    sequence: u64,
    documents: HashMap<String, Document>,
    diagnostics: HashMap<String, (u64, Vec<Value>)>,
}
impl Lsp {
    async fn start(root: &Path) -> Result<Self> {
        let mut process = spawn(root, &["serve"])?;
        let writer = process
            .child
            .stdin
            .take()
            .context("Lean stdin unavailable")?;
        let reader = BufReader::new(
            process
                .child
                .stdout
                .take()
                .context("Lean stdout unavailable")?,
        );
        let mut lsp = Self {
            _process: process,
            writer,
            reader,
            sequence: 0,
            documents: HashMap::new(),
            diagnostics: HashMap::new(),
        };
        lsp.request("initialize",json!({"processId":null,"rootUri":file_uri(root)?,"capabilities":{
            "textDocument":{"publishDiagnostics":{"versionSupport":true}},"experimental":{"silentDiagnosticSupport":true}},
            "initializationOptions":{"hasWidgets":true}})).await?;
        lsp.notify("initialized", json!({})).await?;
        Ok(lsp)
    }
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        write_message(
            &mut self.writer,
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
        )
        .await
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        write_message(
            &mut self.writer,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await?;
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let message = read_message(&mut self.reader).await?;
                if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                    if let Some(error) = message.get("error") {
                        bail!("Lean {method}: {error}");
                    }
                    return Ok(message["result"].clone());
                }
                if message["method"] == "textDocument/publishDiagnostics" {
                    let p = &message["params"];
                    if let (Some(uri), Some(version)) = (p["uri"].as_str(), p["version"].as_u64()) {
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
                    write_message(
                        &mut self.writer,
                        &json!({"jsonrpc":"2.0","id":message["id"],"result":result}),
                    )
                    .await?;
                }
            }
        })
        .await
        .context("Lean check timed out")?
    }
    async fn open(&mut self, uri: &str, source: &str, dependency_key: &str) -> Result<u64> {
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
    sources: HashMap<String, String>,
}
pub struct LeanService {
    root: PathBuf,
    // ponytail: one CLI compiler per branch; add workers if concurrent check throughput requires it.
    manager: Mutex<Manager>,
    results: RwLock<HashMap<String, CheckResult>>,
}
impl LeanService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            manager: Mutex::new(Manager {
                project_key: String::new(),
                root: root.clone(),
                lsp: None,
                sources: HashMap::new(),
            }),
            root,
            results: RwLock::new(HashMap::new()),
        }
    }
    pub fn cached(&self, key: &str, revision: &str, build: bool) -> Option<CheckResult> {
        self.results
            .read()
            .ok()?
            .get(key)
            .filter(|r| !build || r.built)
            .cloned()
            .map(|mut r| {
                r.revision = revision.into();
                r.cache_hit = true;
                r.elapsed_ms = 0;
                r
            })
    }
    pub async fn check(&self, input: Input, build: bool) -> Result<CheckResult> {
        let start = Instant::now();
        let (target, key) = input.chain.last().context("no Lean target")?;
        if let Some(result) = self.cached(key, &target.revision(), build) {
            return Ok(result);
        }
        let target_id = target.fnode.clone();
        let target_key = key.clone();
        let mut manager = self.manager.lock().await;
        if let Some(result) = self.cached(&target_key, &target.revision(), build) {
            return Ok(result);
        }
        self.prepare_input(&mut manager, &input).await?;
        let result = self.check_chain(&mut manager, &input, build).await;
        if result.is_err() {
            manager.lsp = None;
        }
        let mut result = result?;
        result.fnode = target_id;
        result.elapsed_ms = start.elapsed().as_millis();
        self.results
            .write()
            .map_err(|_| anyhow::anyhow!("Lean cache lock poisoned"))?
            .insert(target_key, result.clone());
        Ok(result)
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
                if manager.sources.get(&node.fnode).is_none_or(|s| s != source) {
                    write_source(&manager.root, node, source).await?;
                    manager.sources.insert(node.fnode.clone(), source.into());
                }
            }
        }
        Ok(())
    }
    async fn check_chain(
        &self,
        manager: &mut Manager,
        input: &Input,
        build: bool,
    ) -> Result<CheckResult> {
        let known: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, _)| (n.module.clone(), n.fnode.clone()))
            .collect();
        let keys: BTreeMap<_, _> = input
            .chain
            .iter()
            .map(|(n, k)| (n.fnode.clone(), k.clone()))
            .collect();
        let mut final_result = None;
        for (i, (node, key)) in input.chain.iter().enumerate() {
            let is_target = i + 1 == input.chain.len();
            if let Some(cached) = self.cached(key, &node.revision(), build && is_target) {
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
                    .insert(key.clone(), result.clone());
                final_result = Some(result);
                continue;
            };
            if manager.lsp.is_none() {
                manager.lsp = Some(Lsp::start(&manager.root).await?);
            }
            let uri = file_uri(&module_path(&manager.root, &node.module))?;
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
            let passed = !diagnostics.iter().any(|d| d["severity"] == 1);
            let mut errors = vec![];
            let mut imported = BTreeSet::new();
            for import in &imports {
                let module = import["module"]["name"]
                    .as_str()
                    .context("Lean import omitted module name")?;
                if module.starts_with("Lib.") {
                    if let Some(id) = known.get(module) {
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
                if !self
                    .results
                    .read()
                    .unwrap()
                    .get(&keys[dep])
                    .is_some_and(|r| r.certified)
                {
                    errors.push(format!("dependency {dep} is not verified for Lean"));
                }
            }
            let certified = passed && errors.is_empty();
            let mut result = CheckResult {
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
            if is_target && build && certified {
                build_module(&manager.root, &node.module).await?;
                let artifact = manager
                    .root
                    .join(".lake/build/lib/lean")
                    .join(node.module.replace('.', "/"))
                    .with_extension("olean");
                if !artifact.is_file() {
                    bail!("Lake succeeded without producing the target olean");
                }
                result.built = true;
            }
            self.results
                .write()
                .unwrap()
                .insert(key.clone(), result.clone());
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
        let uri = file_uri(&module_path(&manager.root, &node.module))?;
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
        }
        result
    }

    pub async fn editor_project(&self, input: &Input) -> Result<PathBuf> {
        // Warm the canonical Lake cache once; even an invalid draft still opens in the editor.
        let _ = self.check(input.clone(), false).await;
        let id = uuid::Uuid::new_v4().to_string();
        let root = self.root.join("drafts").join(id);
        prepare_project(&root, &input.project).await?;
        for (node, _) in &input.chain {
            if let Some(source) = node.source("lean") {
                write_source(&root, node, source).await?;
            }
        }
        // Reuse Lake artifacts with copy-on-write where the host filesystem supports it.
        let manager = self.manager.lock().await;
        if manager.project_key == input.project.key() && manager.root.join(".lake").exists() {
            let mut copy = Command::new("cp");
            #[cfg(target_os = "macos")]
            copy.arg("-cR");
            #[cfg(not(target_os = "macos"))]
            copy.args(["-R", "--reflink=auto"]);
            let status = copy
                .arg(manager.root.join(".lake"))
                .arg(&root)
                .status()
                .await?;
            if !status.success() {
                bail!("copying editor build environment failed");
            }
        }
        Ok(root)
    }
}
pub fn module_path(root: &Path, module: &str) -> PathBuf {
    root.join(module.replace('.', "/")).with_extension("lean")
}
pub fn file_uri(path: &Path) -> Result<String> {
    reqwest::Url::from_file_path(path)
        .map(|u| u.to_string())
        .map_err(|_| anyhow::anyhow!("invalid Lean file path"))
}
async fn write_source(root: &Path, node: &Node, source: &str) -> Result<()> {
    let path = module_path(root, &node.module);
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
    tokio::fs::write(root.join("lakefile.toml"), &project.lakefile).await?;
    if let Some(manifest) = &project.manifest {
        tokio::fs::write(root.join("lake-manifest.json"), manifest).await?;
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
    let status = tokio::time::timeout(TIMEOUT, process.child.wait())
        .await
        .context("Lake build timed out")??;
    drain.await??;
    if !status.success() {
        bail!("Lake build failed with {status}");
    }
    Ok(())
}
