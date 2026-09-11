//! Versioned documents in TerminusDB. The in-memory graph is a disposable projection.
use anyhow::{bail, Context, Result};
use reqwest::{Client, Method, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

pub const BLOCK_TYPES: [&str; 4] = ["text", "lean", "rocq", "latex"];

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Lean's quoted identifiers can contain dots; those dots belong to one filename.
pub fn module_parts(mut module: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    while !module.is_empty() {
        let (part, rest) = if let Some(quoted) = module.strip_prefix('«') {
            quoted
                .split_once('»')
                .context("unclosed Lean module identifier")?
        } else {
            let (part, rest) = module.split_once('.').unwrap_or((module, ""));
            if !plain_module_part(part) {
                bail!("invalid unquoted Lean module identifier");
            }
            parts.push(part);
            if rest.is_empty() {
                if module.ends_with('.') {
                    bail!("empty Lean module identifier");
                }
                break;
            }
            module = rest;
            continue;
        };
        if part.is_empty()
            || matches!(part, "." | "..")
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | '«' | '»'))
        {
            bail!("unsafe Lean module identifier");
        }
        parts.push(part);
        if rest.is_empty() {
            break;
        }
        module = rest
            .strip_prefix('.')
            .filter(|s| !s.is_empty())
            .context("invalid Lean module separator")?;
    }
    if parts.len() < 2 || parts[0] != "Lib" {
        bail!("managed Lean module must start with Lib.");
    }
    Ok(parts)
}

fn plain_module_part(part: &str) -> bool {
    part.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && part
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '\''))
}

pub fn module_file(module: &str, extension: &str) -> Result<PathBuf> {
    let mut parts = module_parts(module)?;
    let filename = format!("{}.{extension}", parts.pop().unwrap());
    Ok(parts.into_iter().collect::<PathBuf>().join(filename))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub srctype: String,
    pub content: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub fnode: String,
    pub title: String,
    /// Stable Lean module identity, independent of display name.
    pub module: String,
    #[serde(default)]
    pub depens: Vec<String>,
    #[serde(default)]
    pub blocks: Vec<Block>,
}

impl Node {
    pub fn new(title: String) -> Result<Self> {
        let id = uuid::Uuid::new_v4();
        let node = Self {
            fnode: id.to_string(),
            title,
            module: format!("Lib.N_{}", id.simple()),
            depens: vec![],
            blocks: vec![],
        };
        node.validate()?;
        Ok(node)
    }
    pub fn revision(&self) -> String {
        let mut canonical = self.clone();
        canonical.depens.sort();
        digest(&serde_json::to_vec(&canonical).expect("serializable node"))
    }
    pub fn source(&self, language: &str) -> Option<&str> {
        self.blocks
            .iter()
            .find(|b| b.srctype == language)
            .map(|b| b.content.as_str())
    }
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.fnode).is_err()
            || uuid::Uuid::parse_str(&self.fnode)?.to_string() != self.fnode
        {
            bail!("node identity must be a canonical lowercase hyphenated UUID");
        }
        if self.title.trim().is_empty()
            || self.title != self.title.trim()
            || self.title.chars().any(char::is_control)
        {
            bail!("name must be nonempty, trimmed and contain no control characters");
        }
        module_parts(&self.module)?;
        let mut types = BTreeSet::new();
        for block in &self.blocks {
            if !BLOCK_TYPES.contains(&block.srctype.as_str()) || !types.insert(&block.srctype) {
                bail!("blocks must have distinct types: text, lean, rocq, latex");
            }
        }
        let mut deps = BTreeSet::new();
        for dep in &self.depens {
            if dep == &self.fnode
                || uuid::Uuid::parse_str(dep).is_err()
                || uuid::Uuid::parse_str(dep)?.to_string() != *dep
                || !deps.insert(dep)
            {
                bail!("dependencies must be distinct UUIDs other than the node itself");
            }
        }
        Ok(())
    }
    fn document(&self) -> Value {
        json!({"@id":format!("Node/{}", self.fnode), "@type":"Node", "fnode":self.fnode,
            "title":self.title, "module":self.module,
            "depens":self.depens.iter().map(|id| format!("Node/{id}")).collect::<Vec<_>>(),
            "blocks":serde_json::to_string(&self.blocks).expect("serializable blocks")})
    }
    fn from_document(value: Value) -> Result<Self> {
        let string = |field: &str| {
            value[field]
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("invalid Node.{field}"))
        };
        let node = Self {
            fnode: string("fnode")?,
            title: string("title")?,
            module: string("module")?,
            depens: value["depens"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| {
                    v.as_str()
                        .and_then(|s| s.rsplit('/').next())
                        .map(str::to_owned)
                        .context("invalid dependency link")
                })
                .collect::<Result<_>>()?,
            blocks: serde_json::from_str(&string("blocks")?)?,
        };
        node.validate()?;
        Ok(node)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LeanProject {
    pub toolchain: String,
    pub lakefile: String,
    #[serde(default)]
    pub manifest: Option<String>,
}
impl Default for LeanProject {
    fn default() -> Self {
        Self {
            toolchain: "leanprover/lean4:v4.33.1".into(),
            lakefile: "name = \"MathDoc\"\nversion = \"0.1.0\"\n\n[[lean_lib]]\nname = \"Lib\"\n"
                .into(),
            manifest: None,
        }
    }
}
impl LeanProject {
    pub fn key(&self) -> String {
        digest(&serde_json::to_vec(self).expect("serializable project"))
    }
    pub fn validate(&self) -> Result<()> {
        if !self.toolchain.starts_with("leanprover/lean4:v")
            || self.toolchain.chars().any(char::is_whitespace)
        {
            bail!("pin a Lean toolchain release, such as leanprover/lean4:v4.33.1");
        }
        let config: toml::Value = toml::from_str(&self.lakefile)?;
        if !config
            .get("lean_lib")
            .and_then(toml::Value::as_array)
            .is_some_and(|libs| {
                libs.iter()
                    .any(|l| l.get("name").and_then(toml::Value::as_str) == Some("Lib"))
            })
        {
            bail!("Lake project must declare the Lib lean_lib");
        }
        for key in ["srcDir", "buildDir", "leanLibDir"] {
            if config.get(key).is_some() {
                bail!("custom {key} is not supported in managed projects");
            }
        }
        let required = config.get("require").and_then(toml::Value::as_array);
        if required.is_some_and(|r| !r.is_empty()) && self.manifest.is_none() {
            bail!("external libraries require a committed lake-manifest.json with pinned Git revisions");
        }
        if required
            .into_iter()
            .flatten()
            .any(|r| r.get("path").is_some())
        {
            bail!("publish local libraries to Git and pin them; mutable path dependencies are unsupported");
        }
        if let Some(manifest) = &self.manifest {
            let manifest: Value = serde_json::from_str(manifest)?;
            let packages = manifest["packages"]
                .as_array()
                .context("Lake manifest must contain packages")?;
            if manifest["packagesDir"]
                .as_str()
                .is_some_and(|p| p != ".lake/packages")
            {
                bail!("Lake packagesDir must be .lake/packages");
            }
            for package in packages {
                let rev = package["rev"].as_str().unwrap_or("");
                if package["type"] != "git"
                    || rev.len() != 40
                    || !rev.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    bail!("every library must be locked to a full Git commit in the Lake manifest");
                }
            }
            for required in required.into_iter().flatten() {
                let name = required
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .context("Lake dependency needs a name")?;
                if !packages.iter().any(|p| p["name"] == name) {
                    bail!("library {name} is missing from the Lake manifest");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Terminus {
    client: Client,
    url: String,
    user: String,
    password: String,
    cache_root: PathBuf,
}
impl Terminus {
    pub fn from_env() -> Result<Self> {
        let settings = crate::config::Settings::load()?;
        let cache_root = settings.cache_root()?;
        let url = std::env::var("MDC_TERMINUS_URL")
            .ok()
            .or(settings.terminus_url)
            .unwrap_or("http://127.0.0.1:6363".into());
        let parsed = reqwest::Url::parse(&url)?;
        if !["http", "https"].contains(&parsed.scheme()) {
            bail!("invalid database URL");
        }
        Ok(Self {
            client: Client::builder()
                .pool_idle_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
            url: url.trim_end_matches('/').into(),
            user: std::env::var("MDC_TERMINUS_USER")
                .ok()
                .or(settings.terminus_user)
                .unwrap_or("admin".into()),
            password: std::env::var("MDC_TERMINUS_PASSWORD")
                .ok()
                .or(settings.terminus_password)
                .context("set terminus_password in the user config or MDC_TERMINUS_PASSWORD")?,
            cache_root,
        })
    }
    pub async fn projects(&self) -> Result<Vec<Database>> {
        #[derive(Deserialize)]
        struct Info {
            path: String,
            label: Option<String>,
            branches: Vec<String>,
        }
        let inventory: Vec<Info> = self
            .request(
                Method::GET,
                "db",
                &[("verbose", "true"), ("branches", "true")],
                None,
                None,
            )
            .await?
            .json()
            .await?;
        let mut projects = vec![];
        for info in inventory {
            if info.label.as_deref() != Some("MathDoc") {
                continue;
            }
            let Some(name) = info.path.strip_prefix("admin/") else {
                continue;
            };
            for branch in info.branches {
                projects.push(Database::new(self.clone(), name.into(), branch)?);
            }
        }
        projects.sort_by(|a, b| (&a.database, &a.branch).cmp(&(&b.database, &b.branch)));
        Ok(projects)
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
        version: Option<&str>,
    ) -> Result<Response> {
        let mut request = self
            .client
            .request(method, format!("{}/api/{path}", self.url))
            .basic_auth(&self.user, Some(&self.password))
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(version) = version {
            request = request.header("TerminusDB-Data-Version", version);
        }
        let response = request.send().await.context("connecting to TerminusDB")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if body.contains("DataVersion") || body.contains("data_version") {
                bail!("database revision conflict; reload and retry");
            }
            bail!(
                "TerminusDB {status}: {}",
                body.chars().take(1500).collect::<String>()
            );
        }
        Ok(response)
    }
}

#[derive(Clone)]
pub struct Database {
    server: Terminus,
    pub database: String,
    pub branch: String,
}
impl Database {
    pub fn from_env(database: String, branch: String) -> Result<Self> {
        Self::new(Terminus::from_env()?, database, branch)
    }
    fn new(server: Terminus, database: String, branch: String) -> Result<Self> {
        for part in [&database, &branch] {
            if part.is_empty()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                bail!("database and branch names allow letters, digits, hyphens and underscores");
            }
        }
        Ok(Self {
            server,
            database,
            branch,
        })
    }
    fn path(&self) -> String {
        format!("admin/{}/local/branch/{}", self.database, self.branch)
    }
    fn response_version(response: &Response) -> Result<String> {
        response
            .headers()
            .get("TerminusDB-Data-Version")
            .context("TerminusDB omitted data version")?
            .to_str()
            .map(str::to_owned)
            .map_err(Into::into)
    }
    pub async fn initialize(&self) -> Result<()> {
        self.server
            .request(
                Method::POST,
                &format!("db/admin/{}", self.database),
                &[],
                Some(json!({"label":"MathDoc", "comment":"Versioned MathDoc nodes"})),
                None,
            )
            .await?;
        let schema = json!([
            {"@type":"Class", "@id":"Node", "@key":{"@type":"Lexical","@fields":["fnode"]},
             "fnode":"xsd:string", "title":"xsd:string", "module":"xsd:string", "blocks":"xsd:string",
             "depens":{"@type":"Set","@class":"Node"}},
            {"@type":"Class", "@id":"Project", "@key":{"@type":"Lexical","@fields":["name"]},
             "name":"xsd:string", "config":"xsd:string"}
        ]);
        self.server
            .request(
                Method::POST,
                &format!("document/{}", self.path()),
                &[
                    ("graph_type", "schema"),
                    ("author", "mdc"),
                    ("message", "Initialize MathDoc schema"),
                ],
                Some(schema),
                None,
            )
            .await?;
        self.server.request(Method::POST, &format!("document/{}",self.path()), &[("author","mdc"),("message","Initialize Lean project")],
            Some(json!({"@type":"Project","name":"lean","config":serde_json::to_string(&LeanProject::default())?})),None).await?;
        Ok(())
    }
    pub async fn version(&self) -> Result<String> {
        Self::response_version(
            &self
                .server
                .request(
                    Method::GET,
                    &format!("document/{}", self.path()),
                    &[("count", "0"), ("as_list", "true")],
                    None,
                    None,
                )
                .await?,
        )
    }
    pub async fn load(&self) -> Result<Snapshot> {
        let response = self
            .server
            .request(
                Method::GET,
                &format!("document/{}", self.path()),
                &[("as_list", "true"), ("unfold", "false")],
                None,
                None,
            )
            .await?;
        let version = Self::response_version(&response)?;
        let docs: Vec<Value> = response.json().await?;
        let mut nodes = BTreeMap::new();
        let mut project = None;
        for doc in docs {
            match doc["@type"].as_str() {
                Some("Node") => {
                    let node = Node::from_document(doc)?;
                    nodes.insert(node.fnode.clone(), node);
                }
                Some("Project") if doc["name"] == "lean" => {
                    project = Some(serde_json::from_str(
                        doc["config"].as_str().context("invalid project config")?,
                    )?);
                }
                _ => {}
            }
        }
        let mut snapshot = Snapshot {
            version,
            nodes,
            project: project.context("database has no Lean project")?,
            depths: HashMap::new(),
            lean_keys: HashMap::new(),
            referrers: HashMap::new(),
        };
        snapshot.project.validate()?;
        let nodes: Vec<_> = snapshot.nodes.values().cloned().collect();
        let empty = Snapshot {
            version: String::new(),
            nodes: BTreeMap::new(),
            project: snapshot.project.clone(),
            depths: HashMap::new(),
            lean_keys: HashMap::new(),
            referrers: HashMap::new(),
        };
        empty.validate_changes(&nodes)?;
        snapshot.recompute();
        Ok(snapshot)
    }
    pub async fn put(&self, nodes: &[Node], version: &str, message: &str) -> Result<String> {
        self.put_bundle(nodes, None, version, message).await
    }
    pub async fn put_bundle(
        &self,
        nodes: &[Node],
        project: Option<&LeanProject>,
        version: &str,
        message: &str,
    ) -> Result<String> {
        let mut documents: Vec<Value> = nodes.iter().map(Node::document).collect();
        if let Some(project) = project {
            documents.push(json!({"@id":"Project/lean","@type":"Project","name":"lean","config":serde_json::to_string(project)?}));
        }
        let response = self
            .server
            .request(
                Method::PUT,
                &format!("document/{}", self.path()),
                &[("create", "true"), ("author", "mdc"), ("message", message)],
                Some(Value::Array(documents)),
                Some(version),
            )
            .await?;
        Self::response_version(&response)
    }
    pub async fn put_project(&self, project: &LeanProject, version: &str) -> Result<String> {
        project.validate()?;
        let response = self.server.request(Method::PUT,&format!("document/{}",self.path()),&[("author","mdc"),("message","Update Lean environment")],
            Some(json!({"@id":"Project/lean","@type":"Project","name":"lean","config":serde_json::to_string(project)?})),Some(version)).await?;
        Self::response_version(&response)
    }
    pub async fn history(&self) -> Result<Value> {
        Ok(self
            .server
            .request(
                Method::GET,
                &format!("log/{}", self.path()),
                &[("count", "50")],
                None,
                None,
            )
            .await?
            .json()
            .await?)
    }
    pub async fn create_branch(&self, name: &str) -> Result<Value> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid branch name");
        }
        Ok(self
            .server
            .request(
                Method::POST,
                &format!("branch/admin/{}/local/branch/{name}", self.database),
                &[],
                Some(json!({"origin":self.path()})),
                None,
            )
            .await?
            .json()
            .await?)
    }
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.server.cache_root = root;
        self
    }
    pub fn cache_path(&self) -> Result<PathBuf> {
        Ok(self
            .server
            .cache_root
            .join(&digest(self.server.url.as_bytes())[..12])
            .join(&self.database)
            .join(&self.branch))
    }
}

pub struct Snapshot {
    pub version: String,
    pub nodes: BTreeMap<String, Node>,
    pub project: LeanProject,
    pub depths: HashMap<String, u32>,
    pub lean_keys: HashMap<String, String>,
    pub referrers: HashMap<String, Vec<String>>,
}
impl Snapshot {
    pub fn graph(&self) -> HashMap<String, Vec<String>> {
        self.nodes
            .iter()
            .map(|(id, n)| (id.clone(), n.depens.clone()))
            .collect()
    }
    pub fn recompute(&mut self) {
        self.depths = crate::core::all_topo_depths(&self.graph());
        self.referrers = self.nodes.keys().map(|id| (id.clone(), vec![])).collect();
        for node in self.nodes.values() {
            for dep in &node.depens {
                self.referrers
                    .entry(dep.clone())
                    .or_default()
                    .push(node.fnode.clone());
            }
        }
        self.refresh_lean_keys(self.nodes.keys().cloned().collect());
    }
    fn refresh_lean_keys(&mut self, seeds: BTreeSet<String>) {
        let mut affected = seeds.clone();
        let mut queue: Vec<_> = seeds.into_iter().collect();
        while let Some(id) = queue.pop() {
            for parent in self.referrers.get(&id).into_iter().flatten() {
                if affected.insert(parent.clone()) {
                    queue.push(parent.clone());
                }
            }
        }
        let mut ordered: Vec<_> = affected.into_iter().collect();
        ordered.sort_by_key(|id| self.depths.get(id).copied().unwrap_or(0));
        let environment = self.project.key();
        for id in ordered {
            let node = &self.nodes[&id];
            let deps: BTreeMap<_, _> = node
                .depens
                .iter()
                .map(|dep| (dep, self.lean_keys.get(dep)))
                .collect();
            let key = digest(
                &serde_json::to_vec(&(&environment, &node.module, node.source("lean"), deps))
                    .expect("serializable compilation inputs"),
            );
            self.lean_keys.insert(id, key);
        }
    }
    pub fn resolve(&self, reference: &str) -> Result<&Node> {
        if let Ok(id) = uuid::Uuid::parse_str(reference) {
            return self.nodes.get(&id.to_string()).context("node not found");
        }
        let mut matches = self.nodes.values().filter(|n| n.title == reference);
        let node = matches
            .next()
            .context("node not found; use an exact name or complete UUID")?;
        if matches.next().is_some() {
            bail!("name is ambiguous; use the complete UUID");
        }
        Ok(node)
    }
    pub fn validate_changes(&self, changes: &[Node]) -> Result<()> {
        let mut ids = BTreeSet::new();
        for node in changes {
            node.validate()?;
            if !ids.insert(&node.fnode) {
                bail!("duplicate UUID in transaction");
            }
        }
        if changes.iter().all(|n| {
            self.nodes
                .get(&n.fnode)
                .is_some_and(|old| old.depens == n.depens && old.module == n.module)
        }) {
            return Ok(());
        }
        let mut graph = self.graph();
        let mut modules: BTreeMap<_, _> = self
            .nodes
            .values()
            .map(|n| (n.module.clone(), n.fnode.clone()))
            .collect();
        for node in changes {
            node.validate()?;
            if let Some(owner) = modules.insert(node.module.clone(), node.fnode.clone()) {
                if owner != node.fnode {
                    bail!("Lean module name is already assigned to another node");
                }
            }
            graph.insert(node.fnode.clone(), node.depens.clone());
        }
        for node in changes {
            for dep in &node.depens {
                if !graph.contains_key(dep) {
                    bail!("dependency node does not exist: {dep}");
                }
            }
        }
        if crate::core::strongly_connected_components(&graph)
            .iter()
            .any(|c| c.len() > 1)
        {
            bail!("dependency cycle rejected");
        }
        Ok(())
    }
    pub fn apply(&mut self, changes: Vec<Node>, version: String) {
        let lean_changed = changes
            .iter()
            .filter(|n| {
                self.nodes.get(&n.fnode).is_none_or(|old| {
                    old.source("lean") != n.source("lean") || old.module != n.module
                })
            })
            .map(|n| n.fnode.clone())
            .collect();
        let graph_changed = changes.iter().any(|n| {
            self.nodes
                .get(&n.fnode)
                .is_none_or(|old| old.depens != n.depens)
        });
        for node in changes {
            self.nodes.insert(node.fnode.clone(), node);
        }
        self.version = version;
        if graph_changed {
            self.recompute();
        } else {
            self.refresh_lean_keys(lean_changed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn module_names_preserve_filename_boundaries() {
        let name = "Lib.EGA.«1-1.7.1»";
        assert_eq!(name, "Lib.EGA.«1-1.7.1»");
        assert_eq!(
            module_file(&name, "lean").unwrap(),
            PathBuf::from("Lib/EGA/1-1.7.1.lean")
        );
        assert_eq!(
            module_file(&name, "olean").unwrap(),
            PathBuf::from("Lib/EGA/1-1.7.1.olean")
        );
        for invalid in [
            "Lib.«..»",
            "Lib.«a/b»",
            "Lib.«a\\b»",
            "Lib.«unclosed",
            "Lib.«a»b",
            "Lib.a.",
            "Lib..a",
            "Lib.«a».",
            "Other.a",
        ] {
            assert!(module_file(invalid, "lean").is_err(), "{invalid}");
        }
    }
    #[test]
    fn project_requires_immutable_libraries() {
        let mut p = LeanProject::default();
        assert!(p.validate().is_ok());
        p.lakefile
            .push_str("\n[[require]]\nname=\"example\"\ngit=\"https://example.org/lib.git\"\n");
        assert!(p.validate().is_err());
        p.manifest =
            Some(json!({"packages":[{"name":"example","type":"path","dir":"../lib"}]}).to_string());
        assert!(p.validate().is_err());
        p.manifest=Some(json!({"packages":[{"name":"example","type":"git","rev":"0123456789012345678901234567890123456789"}]}).to_string());
        assert!(p.validate().is_ok());
    }
    #[test]
    fn references_and_cycles_are_explicit() {
        let a = Node::new("A".into()).unwrap();
        let mut b = Node::new("B".into()).unwrap();
        b.depens.push(a.fnode.clone());
        let s = Snapshot {
            version: String::new(),
            nodes: [a.clone(), b.clone()]
                .into_iter()
                .map(|n| (n.fnode.clone(), n))
                .collect(),
            project: LeanProject::default(),
            depths: HashMap::new(),
            lean_keys: HashMap::new(),
            referrers: HashMap::new(),
        };
        assert_eq!(s.resolve("A").unwrap().fnode, a.fnode);
        assert!(s.resolve("A.mdoc").is_err());
        assert!(s.resolve(&a.fnode[..8]).is_err());
        assert_eq!(s.resolve(&a.fnode.to_uppercase()).unwrap().fnode, a.fnode);
        let mut alias = a.clone();
        alias.fnode = uuid::Uuid::parse_str(&alias.fnode)
            .unwrap()
            .simple()
            .to_string();
        assert!(alias.validate().is_err());
        let mut changed = a.clone();
        changed.depens.push(b.fnode);
        assert!(s.validate_changes(&[changed]).is_err());
        assert_eq!(Node::from_document(a.document()).unwrap(), a);
    }
}
