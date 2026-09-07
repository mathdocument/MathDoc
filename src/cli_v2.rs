//! API client. Files are only read by explicit import and stdin source editing.
use crate::store::{Block, Database, LeanProject, Node};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::io::Read;

#[derive(Parser)]
#[command(name = "mdc", about = "MathDoc local web service and API client")]
struct Cli {
    #[arg(long, global = true)]
    url: Option<String>,
    #[arg(long, global = true, default_value = "mathdoc")]
    database: String,
    #[arg(long, global = true, default_value = "main")]
    branch: String,
    #[arg(long, global = true)]
    prof: bool,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Initialize a TerminusDB database. Requires MDC_TERMINUS_PASSWORD.
    Init,
    /// Start the local browser service. Requires MDC_TERMINUS_PASSWORD.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7599")]
        bind: String,
    },
    /// Create a node in the database.
    New {
        #[arg(short, long)]
        title: String,
    },
    /// Read a node by exact name or complete UUID.
    Show {
        source: String,
    },
    /// Replace one source block with stdin; never opens or synchronizes a workspace file.
    Edit {
        source: String,
        #[arg(long="type",default_value="lean",value_parser=["text","lean","rocq","latex"])]
        language: String,
        #[arg(long)]
        revision: Option<String>,
    },
    /// Rename a node using an optimistic revision guard.
    Rename {
        source: String,
        title: String,
    },
    Search {
        query: String,
        #[arg(short = 'n', long, default_value = "200")]
        max_results: usize,
    },
    Graph {
        #[command(subcommand)]
        command: Graph,
    },
    Dep {
        #[command(subcommand)]
        command: Dep,
    },
    Metric {
        #[command(subcommand)]
        command: Metric,
    },
    /// Check the saved Lean block through the persistent Lean server.
    Work {
        source: String,
        #[arg(long)]
        build: bool,
    },
    Lean {
        #[command(subcommand)]
        command: Lean,
    },
    /// Export the database as portable JSON, or a selected node as mdoc.
    Export {
        source: Option<String>,
    },
    /// Explicit one-time import of a JSON export or a legacy .mdoc directory.
    Import {
        input: std::path::PathBuf,
    },
    Project {
        #[command(subcommand)]
        command: Project,
    },
    History,
    Branch {
        #[command(subcommand)]
        command: Branch,
    },
}
#[derive(Subcommand)]
enum Graph {
    Check,
    Roots,
    Full,
}
#[derive(Subcommand)]
enum Metric {
    Ior { source: String },
}
#[derive(Subcommand)]
enum Dep {
    Add {
        source: String,
        #[arg(short, long)]
        target: String,
    },
    Rm {
        source: String,
        #[arg(short, long)]
        target: String,
    },
    Show {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
    Refs {
        target: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
    Leaf {
        source: String,
    },
}
#[derive(Subcommand)]
enum Lean {
    Check {
        source: String,
        #[arg(long)]
        build: bool,
        #[arg(long)]
        revision: Option<String>,
    },
    Goals {
        source: String,
        #[arg(long)]
        line: u32,
        #[arg(long, default_value = "0")]
        column: u32,
    },
}
#[derive(Subcommand)]
enum Project {
    Show,
    Set,
}
#[derive(Subcommand)]
enum Branch {
    Create { name: String },
}

struct Api {
    client: Client,
    url: String,
}
impl Api {
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
        revision: Option<&str>,
    ) -> Result<Value> {
        let mut req = self
            .client
            .request(method, format!("{}/api{path}", self.url))
            .query(query);
        if let Some(body) = body {
            req = req.json(&body);
        }
        if let Some(rev) = revision {
            req = req.header("if-match", format!("\"{rev}\""));
        }
        let response = req
            .send()
            .await
            .context("connect to mdc serve (default http://127.0.0.1:7599)")?;
        let status = response.status();
        let text = response.text().await?;
        let value: Value = serde_json::from_str(&text).unwrap_or(json!({"error":text}));
        if !status.is_success() {
            bail!(
                "HTTP {status}: {}",
                value["error"].as_str().unwrap_or("request failed")
            );
        }
        Ok(value)
    }
    async fn get(&self, path: &str) -> Result<Value> {
        self.request(Method::GET, path, &[], None, None).await
    }
    async fn node(&self, reference: &str) -> Result<Value> {
        let resolved = self
            .request(
                Method::GET,
                "/resolve",
                &[("ref", reference.into())],
                None,
                None,
            )
            .await?;
        let id = resolved["fnode"]
            .as_str()
            .context("invalid resolve response")?;
        Ok(self.get(&format!("/node/{id}/view")).await?["node"].clone())
    }
    async fn check(&self, source: &str, build: bool, revision: Option<String>) -> Result<Value> {
        let n = self.node(source).await?;
        self.request(
            Method::POST,
            &format!("/node/{}/lean/check", n["fnode"].as_str().unwrap()),
            &[],
            Some(json!({"build":build})),
            Some(
                revision
                    .as_deref()
                    .unwrap_or(n["revision"].as_str().unwrap()),
            ),
        )
        .await
    }
}
fn stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}
pub fn run() -> i32 {
    let cli = Cli::parse();
    crate::profile::set_enabled(cli.prof);
    let result = tokio::runtime::Runtime::new()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(dispatch(cli)));
    crate::profile::print_report();
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", crate::core::escape_terminal(&e.to_string()));
            1
        }
    }
}
async fn dispatch(cli: Cli) -> Result<i32> {
    let _profile = crate::profile::scope("service.request");
    let url = cli
        .url
        .or_else(|| std::env::var("MDC_URL").ok())
        .unwrap_or("http://127.0.0.1:7599".into());
    let api = Api {
        client: Client::builder()
            .timeout(std::time::Duration::from_secs(1800))
            .build()?,
        url: url.trim_end_matches('/').into(),
    };
    let value = match cli.command {
        Commands::Init => {
            Database::from_env(cli.database, cli.branch)?
                .initialize()
                .await?;
            json!({"initialized":true})
        }
        Commands::Serve { bind } => {
            crate::service::serve(Database::from_env(cli.database, cli.branch)?, &bind).await?;
            return Ok(0);
        }
        Commands::New { title } => {
            api.request(
                Method::POST,
                "/node/new",
                &[],
                Some(json!({"title":title})),
                None,
            )
            .await?
        }
        Commands::Show { source } => api.node(&source).await?,
        Commands::Edit {
            source,
            language,
            revision,
        } => {
            let content = stdin()?;
            let node = api.node(&source).await?;
            api.request(
                Method::PUT,
                &format!("/node/{}/block/{language}", node["fnode"].as_str().unwrap()),
                &[],
                Some(json!({"content":content})),
                Some(
                    revision
                        .as_deref()
                        .unwrap_or(node["revision"].as_str().unwrap()),
                ),
            )
            .await?
        }
        Commands::Rename { source, title } => {
            let n = api.node(&source).await?;
            api.request(
                Method::PUT,
                &format!("/node/{}/title", n["fnode"].as_str().unwrap()),
                &[],
                Some(json!({"title":title})),
                n["revision"].as_str(),
            )
            .await?
        }
        Commands::Search { query, max_results } => {
            api.request(
                Method::GET,
                "/search",
                &[("q", query), ("n", max_results.to_string())],
                None,
                None,
            )
            .await?
        }
        Commands::Graph { command } => {
            api.get(match command {
                Graph::Check => "/graph/check",
                Graph::Roots => "/graph/roots",
                Graph::Full => "/graph/full",
            })
            .await?
        }
        Commands::Dep { command } => match command {
            Dep::Add { source, target } => mutate_dep(&api, &source, &target, true).await?,
            Dep::Rm { source, target } => mutate_dep(&api, &source, &target, false).await?,
            Dep::Show { source, depth } => traverse(&api, &source, "show", depth).await?,
            Dep::Refs { target, depth } => traverse(&api, &target, "refs", depth).await?,
            Dep::Leaf { source } => traverse(&api, &source, "leaf", -1).await?,
        },
        Commands::Metric {
            command: Metric::Ior { source },
        } => {
            let n = api.node(&source).await?;
            api.get(&format!(
                "/node/{}/metric/ior",
                n["fnode"].as_str().unwrap()
            ))
            .await?
        }
        Commands::Work { source, build } => api.check(&source, build, None).await?,
        Commands::Lean { command } => match command {
            Lean::Check {
                source,
                build,
                revision,
            } => api.check(&source, build, revision).await?,
            Lean::Goals {
                source,
                line,
                column,
            } => {
                let n = api.node(&source).await?;
                api.request(
                    Method::POST,
                    &format!("/node/{}/lean/goals", n["fnode"].as_str().unwrap()),
                    &[],
                    Some(json!({"line":line,"character":column})),
                    n["revision"].as_str(),
                )
                .await?
            }
        },
        Commands::Export { source: None } => api.get("/export").await?,
        Commands::Export {
            source: Some(source),
        } => {
            let n = api.node(&source).await?;
            let node = crate::mdocnode::MdocNode {
                path: std::path::PathBuf::new(),
                fnode: n["fnode"].as_str().unwrap().into(),
                title: n["title"].as_str().unwrap().into(),
                depens: serde_json::from_value(n["depens"].clone())?,
                blocks: serde_json::from_value(n["blocks"].clone())?,
            };
            print!("{}", node.render()?);
            return Ok(0);
        }
        Commands::Import { input } => {
            let body = if input.is_dir() {
                legacy_import(&input)?
            } else {
                serde_json::from_slice(&std::fs::read(input)?)?
            };
            api.request(Method::POST, "/import", &[], Some(body), None)
                .await?
        }
        Commands::Project {
            command: Project::Show,
        } => api.get("/project/lean").await?,
        Commands::Project {
            command: Project::Set,
        } => {
            let project: Value = serde_json::from_str(&stdin()?)?;
            let p = api.get("/project/lean").await?;
            api.request(
                Method::PUT,
                "/project/lean",
                &[],
                Some(project),
                p["revision"].as_str(),
            )
            .await?
        }
        Commands::History => api.get("/history").await?,
        Commands::Branch {
            command: Branch::Create { name },
        } => {
            api.request(
                Method::POST,
                "/branches",
                &[],
                Some(json!({"name":name})),
                None,
            )
            .await?
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(
        if value.get("passed") == Some(&Value::Bool(false))
            || value.get("certified") == Some(&Value::Bool(false))
        {
            1
        } else {
            0
        },
    )
}
async fn traverse(api: &Api, source: &str, mode: &str, depth: i32) -> Result<Value> {
    let n = api.node(source).await?;
    api.request(
        Method::GET,
        &format!("/node/{}/dep", n["fnode"].as_str().unwrap()),
        &[("mode", mode.into()), ("depth", depth.to_string())],
        None,
        None,
    )
    .await
}
async fn mutate_dep(api: &Api, source: &str, target: &str, add: bool) -> Result<Value> {
    let n = api.node(source).await?;
    let t = api.node(target).await?;
    let body = if add {
        json!({"dep_fnode":t["fnode"]})
    } else {
        json!({"dep_fnodes":[t["fnode"]]})
    };
    api.request(
        Method::POST,
        &format!(
            "/node/{}/dep/{}",
            n["fnode"].as_str().unwrap(),
            if add { "add" } else { "rm" }
        ),
        &[],
        Some(body),
        n["revision"].as_str(),
    )
    .await
}
fn legacy_import(root: &std::path::Path) -> Result<Value> {
    let mut nodes = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.') || e.depth() == 0)
    {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("mdoc")
        {
            continue;
        }
        let legacy = crate::mdocnode::MdocNode::load(entry.path())?;
        let relative = entry.path().strip_prefix(root)?.with_extension("");
        let module = format!(
            "Lib.{}",
            relative
                .components()
                .map(|p| p.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join(".")
        );
        let node = Node {
            fnode: legacy.fnode,
            title: legacy.title,
            module,
            depens: legacy.depens,
            blocks: legacy
                .blocks
                .into_iter()
                .map(|b| Block {
                    srctype: b.srctype,
                    content: b.content,
                    metadata: b.metadata.into_iter().collect(),
                })
                .collect(),
        };
        node.validate()
            .with_context(|| format!("import {}", entry.path().display()))?;
        nodes.push(node);
    }
    let lean = root.join(".mdc/lean");
    let project = if lean.join("lean-toolchain").exists() {
        Some(LeanProject {
            toolchain: std::fs::read_to_string(lean.join("lean-toolchain"))?
                .trim()
                .into(),
            lakefile: std::fs::read_to_string(lean.join("lakefile.toml"))?,
            manifest: std::fs::read_to_string(lean.join("lake-manifest.json")).ok(),
        })
    } else {
        None
    };
    Ok(json!({"nodes":nodes,"project":project}))
}
