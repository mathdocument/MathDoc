//! API client. Files are only read by explicit import and stdin source editing.
use crate::store::Database;
use anyhow::{bail, Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::io::Read;

#[derive(Parser)]
#[command(name = "mdc", about = "MathDoc local web service and API client")]
struct Cli {
    /// Print command timing measurements to stderr.
    #[arg(long, global = true)]
    prof: bool,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// List all project branches and their local service ports, including stopped projects.
    Status,
    /// Create a new project database on the configured TerminusDB instance.
    Init { database: String },
    /// Start a project branch in the background and print its browser URL.
    Start {
        #[arg(value_name = "DATABASE/BRANCH", value_parser = parse_project)]
        project: String,
        /// Use this port; omitted means an available port chosen by the OS.
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
        port: Option<u16>,
    },
    /// Stop a project service and wait for Lean workers to shut down.
    Stop {
        #[arg(value_name = "DATABASE/BRANCH", value_parser = parse_project)]
        project: String,
    },
    /// Run the internal background service for one branch.
    #[command(name = "__run", hide = true)]
    Run {
        #[arg(value_parser = parse_project)]
        project: String,
        #[arg(long)]
        port: u16,
    },
    /// Search branch nodes by title or UUID.
    Search {
        query: String,
        #[arg(short = 'n', long, default_value = "200")]
        max_results: usize,
    },
    /// Inspect or validate the dependency graph of the branch.
    Graph {
        #[command(subcommand)]
        command: Graph,
    },
    /// Export the database, or one node, as a portable JSON bundle.
    Export { source: Option<String> },
    /// Import a JSON bundle without overwriting existing node UUIDs.
    Import { input: std::path::PathBuf },
    /// Manage the branch's Lean toolchain and library configuration.
    Project {
        #[command(subcommand)]
        command: Project,
    },
    /// Show the latest 50 commits in the branch's history.
    History,
    /// Create a branch from the selected branch head.
    Branch {
        #[command(subcommand)]
        command: Branch,
    },
    /// Create a node in the database.
    New {
        #[arg(short, long)]
        title: String,
    },
    /// Read a node by exact name or complete UUID.
    Show { source: String },
    /// Replace one source block of a node with text from stdin.
    Edit {
        source: String,
        #[arg(long="type",default_value="lean",value_parser=["text","lean","rocq","latex"])]
        language: String,
        /// Require the node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Rename a node using an optimistic revision guard.
    Rename { source: String, title: String },
    /// Manage or traverse dependencies and referrers of a node.
    Dep {
        #[command(subcommand)]
        command: Dep,
    },
    /// Compute graph metrics for a node.
    Metric {
        #[command(subcommand)]
        command: Metric,
    },
    /// Check Lean code or inspect proof goals for a node.
    Lean {
        #[command(subcommand)]
        command: Lean,
    },
}
#[derive(Subcommand)]
enum Graph {
    /// Validate the branch graph and report missing links or cycles.
    Check,
    /// List unreferenced graph roots and their component sizes.
    Roots,
    /// Return all node summaries and dependency edges in the branch.
    Full,
}
#[derive(Subcommand)]
enum Metric {
    /// Compute a node's IOR metric from its dependency and referrer counts.
    Ior { source: String },
}
#[derive(Subcommand)]
enum Dep {
    /// Add a dependency from one node to another.
    Add {
        source: String,
        #[arg(short, long)]
        target: String,
    },
    /// Remove a dependency from a node.
    Rm {
        source: String,
        #[arg(short, long)]
        target: String,
    },
    /// List dependencies reachable from a node at the requested depth.
    Show {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
    /// List nodes that depend on the target at the requested depth.
    Refs {
        target: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
    /// List dependency leaves reachable from a node.
    Leaf { source: String },
}
#[derive(Subcommand)]
enum Lean {
    /// Validate a node with Lean and optionally build its olean artifact.
    Check {
        source: String,
        /// Generate the target node's olean artifact.
        #[arg(long)]
        build: bool,
        /// Require the node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Show Lean proof goals at a position in a node.
    Goals {
        source: String,
        /// Zero-based line number.
        #[arg(long)]
        line: u32,
        /// Zero-based character offset within the line.
        #[arg(long, default_value = "0")]
        column: u32,
    },
}
#[derive(Subcommand)]
enum Project {
    /// Read the branch's Lean toolchain, Lake configuration and dependency lockfile.
    Show,
    /// Replace the branch's Lean toolchain, Lake configuration and lockfile from stdin JSON.
    Set,
}
#[derive(Subcommand)]
enum Branch {
    /// Fork a new branch from the selected branch head.
    Create { name: String },
}

struct Api {
    client: Client,
    url: String,
    token: String,
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
            .header("x-mdc-service", &self.token)
            .query(query);
        if let Some(body) = body {
            req = req.json(&body);
        }
        if let Some(rev) = revision {
            req = req.header("if-match", format!("\"{rev}\""));
        }
        let response = req.send().await.with_context(|| {
            format!(
                "connect to the selected project at {}; check mdc status",
                self.url
            )
        })?;
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

fn command_group(name: &str) -> u8 {
    match name {
        "status" | "init" | "start" | "stop" => 0,
        "new" | "show" | "edit" | "rename" | "dep" | "metric" | "lean" => 2,
        _ => 1,
    }
}

fn grouped_command() -> clap::Command {
    let command = Cli::command().mut_subcommands(|subcommand| {
        if subcommand.is_hide_set() || command_group(subcommand.get_name()) == 0 {
            return subcommand;
        }
        subcommand.arg(
            clap::Arg::new("proj")
                .long("proj")
                // Inherit only within this client command, never at the CLI root.
                .global(true)
                .value_name("DATABASE/BRANCH")
                .value_parser(parse_project)
                .help("Select a running project branch (required)"),
        )
    });
    let header = command.get_styles().get_header();
    // Render together to align columns, then separate management, graph and node operations.
    let mut commands = command
        .clone()
        .disable_help_subcommand(true)
        .help_template("{subcommands}")
        .render_help()
        .to_string();
    let mut previous = 0;
    for subcommand in command.get_subcommands().filter(|c| !c.is_hide_set()) {
        let group = command_group(subcommand.get_name());
        if group != previous {
            let entry = format!("\n  {} ", subcommand.get_name());
            commands = commands.replacen(&entry, &format!("\n{entry}"), 1);
        }
        previous = group;
    }
    let template = format!("{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{header}Commands:{header:#}\n{commands}\n{header}Options:{header:#}\n{{options}}");
    command.help_template(template)
}

pub fn run() -> i32 {
    let matches = grouped_command().get_matches();
    let project = matches
        .subcommand()
        .and_then(|(_, args)| args.try_get_one::<String>("proj").ok().flatten())
        .cloned();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    crate::profile::set_enabled(cli.prof);
    let result = tokio::runtime::Runtime::new()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(dispatch(cli, project)));
    crate::profile::print_report();
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", crate::core::escape_terminal(&e.to_string()));
            1
        }
    }
}
async fn dispatch(cli: Cli, project: Option<String>) -> Result<i32> {
    let _profile = crate::profile::scope("service.request");
    let local = match &cli.command {
        Commands::Status => {
            let projects = crate::store::Terminus::from_env()?.projects().await?;
            let mut status = serde_json::Map::new();
            for db in projects {
                status.insert(
                    format!("{}/{}", db.database, db.branch),
                    json!({"port":crate::service::running_service(&db.cache_path()?)?.map(|s| s.port)}),
                );
            }
            Some(Value::Object(status))
        }
        Commands::Init { database } => {
            Database::from_env(database.clone(), "main".into())?
                .initialize()
                .await?;
            Some(json!({"initialized":true}))
        }
        Commands::Start { project, port } => {
            Some(crate::service::start(project, port.unwrap_or(0)).await?)
        }
        Commands::Stop { project } => Some(crate::service::stop(project).await?),
        Commands::Run { project, port } => {
            let result = async {
                let (database, branch) = crate::config::project_parts(project)?;
                crate::service::serve(Database::from_env(database.into(), branch.into())?, *port)
                    .await
            }
            .await;
            if let Err(error) = result {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stdout(),
                    "{}",
                    json!({"error":format!("{error:#}")})
                );
                eprintln!("error: {error:#}");
                return Ok(1);
            }
            return Ok(0);
        }
        _ => None,
    };
    if let Some(value) = local {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
    let project = project
        .context("select a project with --proj DATABASE/BRANCH; use mdc status to list projects")?;
    let root = crate::config::Settings::load()?.project_cache(&project)?;
    let service = crate::service::running_service(&root)?
        .with_context(|| format!("{project} is not running; run mdc start {project}"))?;
    let api = Api {
        client: Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(1800))
            .build()?,
        url: service.url(),
        token: service.token,
    };
    let value = match cli.command {
        Commands::Status
        | Commands::Init { .. }
        | Commands::Start { .. }
        | Commands::Stop { .. }
        | Commands::Run { .. } => unreachable!(),
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

fn parse_project(value: &str) -> Result<String, String> {
    crate::config::project_parts(value).map_err(|e| e.to_string())?;
    Ok(value.into())
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
    fn help_groups_commands_without_footers_or_changing_subcommands() {
        let mut command = grouped_command();
        let short = command.render_help().to_string();
        assert_eq!(
            short.split_once("Options:").unwrap().0,
            command
                .render_long_help()
                .to_string()
                .split_once("Options:")
                .unwrap()
                .0
        );
        assert_eq!(short.matches("Commands:").count(), 1);
        let (_, groups) = short.split_once("Commands:\n").unwrap();
        let groups = groups.split_once("Options:\n").unwrap().0.trim();
        let names: Vec<Vec<_>> = groups
            .split("\n\n")
            .map(|group| {
                group
                    .lines()
                    .filter_map(|line| line.split_whitespace().next())
                    .collect()
            })
            .collect();
        assert_eq!(
            names,
            [
                vec!["status", "init", "start", "stop"],
                vec!["search", "graph", "export", "import", "project", "history", "branch"],
                vec!["new", "show", "edit", "rename", "dep", "metric", "lean"],
            ]
        );
        assert!(!short.contains("__run"));
        assert!(!short.contains("Examples:"));
        assert!(!short.contains("--proj"));
        assert!(short.contains("Print command timing measurements to stderr"));
        for args in [
            vec!["mdc", "help"],
            vec!["mdc", "help", "status"],
            vec!["mdc", "status", "--help"],
        ] {
            let error = grouped_command().try_get_matches_from(args).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(!error
                .to_string()
                .contains("Reads the configured TerminusDB"));
        }
    }

    #[test]
    fn every_command_and_subcommand_has_a_description_in_help() {
        let mut pending = vec![(vec!["mdc".to_string()], Cli::command())];
        while let Some((path, command)) = pending.pop() {
            let about = command
                .get_about()
                .expect("command description")
                .to_string();
            assert!(!about.trim().is_empty(), "{path:?}");
            assert!(command.get_after_help().is_none(), "{path:?}");
            let mut args = path.clone();
            args.push("--help".into());
            let error = grouped_command().try_get_matches_from(args).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(error.to_string().starts_with(&about), "{path:?}: {error}");
            for child in command.get_subcommands() {
                let mut child_path = path.clone();
                child_path.push(child.get_name().into());
                pending.push((child_path, child.clone()));
            }
        }
    }

    #[test]
    fn project_selection_is_scoped_to_client_commands() {
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "init"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "start", "mdocs/agent"])
            .is_ok());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "stop", "mdocs/agent"])
            .is_ok());
        for project in [
            "mdocs",
            "/main",
            "mdocs/",
            "mdocs/../main",
            "../main",
            "mdocs/main/extra",
        ] {
            assert!(grouped_command()
                .try_get_matches_from(["mdc", "start", project])
                .is_err());
        }
        for port in ["0", "65536", "-1", "abc"] {
            assert!(grouped_command()
                .try_get_matches_from(["mdc", "start", "mdocs/main", "--port", port])
                .is_err());
        }
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "serve", "mdocs"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "--database", "other", "graph", "check"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "--url", "http://localhost:7600", "graph", "check"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "graph", "check", "--proj", "mdocs/main"])
            .is_ok());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "graph", "--proj", "mdocs/main", "check"])
            .is_ok());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "--proj", "mdocs/main", "graph", "check"])
            .is_err());
        for args in [
            vec!["mdc", "status"],
            vec!["mdc", "init", "db"],
            vec!["mdc", "start", "db/main"],
            vec!["mdc", "stop", "db/main"],
        ] {
            let mut help = args.clone();
            help.push("--help");
            let error = grouped_command().try_get_matches_from(help).unwrap_err();
            assert!(!error.to_string().contains("--proj"));
            let mut invalid = args;
            invalid.extend(["--proj", "mdocs/main"]);
            assert!(grouped_command().try_get_matches_from(invalid).is_err());
        }
        for path in [
            vec!["new"],
            vec!["graph", "check"],
            vec!["project", "show"],
            vec!["history"],
            vec!["branch", "create"],
        ] {
            let mut args = vec!["mdc"];
            args.extend(path);
            args.push("--help");
            let error = grouped_command().try_get_matches_from(args).unwrap_err();
            assert!(error.to_string().contains("--proj"));
        }
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "work", "node"])
            .is_err());
    }
}
