//! API client. Files are only read by explicit import and stdin source editing.
use crate::store::Database;
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
    #[arg(long, global = true)]
    prof: bool,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// List all project branches and their local service ports, including stopped projects.
    #[command(
        after_help = "Reads the configured TerminusDB and local cache directory directly; no running mdc service is required. Project is DATABASE/BRANCH. Port is blank for stopped services. Uses terminus_url / MDC_TERMINUS_URL; --url is not applicable."
    )]
    Status,
    /// Create a new project database on the configured TerminusDB instance.
    Init {
        database: String,
    },
    /// Serve one project database branch from any directory.
    Serve {
        database: String,
        #[arg(long, default_value = "main")]
        branch: String,
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
    Lean {
        #[command(subcommand)]
        command: Lean,
    },
    /// Export the database, or one node, as a portable JSON bundle.
    Export {
        source: Option<String>,
    },
    /// Import a JSON bundle. Existing node UUIDs are never overwritten.
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
    if matches!(cli.command, Commands::Status) {
        if cli.url.is_some() {
            bail!(
                "status reads TerminusDB directly; use MDC_TERMINUS_URL or terminus_url, not --url"
            );
        }
        let projects = crate::store::Terminus::from_env()?.projects().await?;
        let rows = projects
            .iter()
            .map(|db| {
                Ok((
                    format!("{}/{}", db.database, db.branch),
                    crate::service::running_port(&db.cache_path()?)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        use std::io::IsTerminal;
        print!("{}", status_table(&rows, std::io::stdout().is_terminal()));
        return Ok(0);
    }
    let url = cli
        .url
        .or_else(|| std::env::var("MDC_URL").ok())
        .or(crate::config::Settings::load()?.url)
        .unwrap_or("http://127.0.0.1:7599".into());
    let api = Api {
        client: Client::builder()
            .timeout(std::time::Duration::from_secs(1800))
            .build()?,
        url: url.trim_end_matches('/').into(),
    };
    let value = match cli.command {
        Commands::Status => unreachable!(),
        Commands::Init { database } => {
            Database::from_env(database, "main".into())?
                .initialize()
                .await?;
            json!({"initialized":true})
        }
        Commands::Serve {
            database,
            branch,
            bind,
        } => {
            crate::service::serve(Database::from_env(database, branch)?, &bind).await?;
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
            let node = serde_json::from_value::<crate::store::Node>(json!({
                "fnode":n["fnode"], "title":n["title"], "module":n["module"],
                "depens":n["depens"], "blocks":n["blocks"]
            }))?;
            json!({"nodes":[node], "project":null})
        }
        Commands::Import { input } => {
            let body: Value = serde_json::from_slice(
                &std::fs::read(input).context("import expects a JSON bundle file")?,
            )?;
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

fn status_table(rows: &[(String, Option<u16>)], bold: bool) -> String {
    use std::fmt::Write;
    let width = rows
        .iter()
        .map(|(project, _)| project.len())
        .max()
        .unwrap_or(0)
        .max(7);
    let (start, end) = if bold {
        ("\x1b[1m", "\x1b[0m")
    } else {
        ("", "")
    };
    let mut table = format!("{start}{:<width$}  Port{end}\n", "Project");
    for (project, port) in rows {
        writeln!(
            table,
            "{project:<width$}  {}",
            port.map(|p| p.to_string()).unwrap_or_default()
        )
        .unwrap();
    }
    table
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_headers_are_bold_only_in_a_terminal_and_stopped_ports_are_blank() {
        let rows = vec![("etp/main".into(), Some(7600)), ("mdocs/main".into(), None)];
        assert_eq!(
            status_table(&rows, false),
            "Project     Port\netp/main    7600\nmdocs/main  \n"
        );
        assert_eq!(
            status_table(&rows, true),
            "\x1b[1mProject     Port\x1b[0m\netp/main    7600\nmdocs/main  \n"
        );
        assert_eq!(status_table(&[], false), "Project  Port\n");
    }
    #[test]
    fn project_selection_belongs_to_service_commands() {
        assert!(Cli::try_parse_from(["mdc", "init"]).is_err());
        assert!(Cli::try_parse_from(["mdc", "serve", "mdocs", "--branch", "agent"]).is_ok());
        assert!(Cli::try_parse_from(["mdc", "--database", "other", "graph", "check"]).is_err());
        assert!(
            Cli::try_parse_from(["mdc", "--url", "http://localhost:7600", "graph", "check"])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["mdc", "work", "node"]).is_err());
    }
}
