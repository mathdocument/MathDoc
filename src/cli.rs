//! API client. Files are only read by explicit import and stdin source editing.
use crate::store::Database;
use anyhow::{bail, Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::io::Read;

#[derive(Parser)]
#[command(
    name = "mdc",
    about = "MathDoc local web service and API client",
    disable_help_subcommand = true
)]
struct Cli {
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
    /// Search branch nodes by title or UUID.
    Search {
        query: String,
        /// Maximum number of results (0–200).
        #[arg(short = 'n', long, default_value = "200", value_parser = clap::value_parser!(u16).range(0..=200))]
        max_results: u16,
    },
    /// Inspect or validate the dependency graph of the branch.
    Graph {
        #[command(subcommand)]
        command: Graph,
    },
    /// Export the entire branch graph and Lean project configuration as JSON.
    Export,
    /// Restore a complete graph JSON bundle into an empty branch.
    Import { input: std::path::PathBuf },
    /// Show the latest 50 commits in the branch's history.
    History,
    /// Create a branch or delete a stopped branch and its Lean caches.
    Branch {
        #[command(subcommand)]
        command: Branch,
    },
    /// Manage the branch's Lean toolchain and library configuration.
    Project {
        #[command(subcommand)]
        command: Project,
    },
    /// Create a node in the database.
    New {
        #[arg(short, long)]
        title: String,
        /// Atomically add the new node as a dependency of this parent.
        #[arg(long)]
        parent: Option<String>,
        /// Require this parent revision, returned by show.
        #[arg(long, requires = "parent")]
        revision: Option<String>,
    },
    /// Manage or traverse dependencies and referrers of a node.
    Dep {
        #[command(subcommand)]
        command: Dep,
    },
    /// Read a node by exact name or complete UUID.
    Show { source: String },
    /// Replace a source block from stdin, or delete it with --delete.
    Edit {
        source: String,
        #[arg(long="type",default_value="lean",value_parser=["text","lean","rocq","latex"])]
        language: String,
        /// Require the node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
        /// Delete this source block without reading stdin.
        #[arg(long)]
        delete: bool,
    },
    /// Rename a node using an optimistic revision guard.
    Rename {
        source: String,
        title: String,
        /// Require the node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
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
        /// Require the source node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Remove one or more dependencies from a node in one transaction.
    Rm {
        source: String,
        /// Remove these dependencies in one transaction.
        #[arg(short, long, required = true, num_args = 1..)]
        target: Vec<String>,
        /// Require the source node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
    },
    /// List dependencies reachable from a node at the requested depth.
    Show {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true, value_parser = clap::value_parser!(i32).range(-1..))]
        depth: i32,
    },
    /// List nodes that depend on the target at the requested depth.
    Refs {
        target: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true, value_parser = clap::value_parser!(i32).range(-1..))]
        depth: i32,
    },
    /// List dependency leaves reachable from a node.
    Leaf { source: String },
    /// Search nodes not already linked as direct dependencies of the source.
    Candidates {
        source: String,
        #[arg(default_value = "")]
        query: String,
        /// Maximum number of results (0–200).
        #[arg(short = 'n', long, default_value = "200", value_parser = clap::value_parser!(u16).range(0..=200))]
        max_results: u16,
    },
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
        /// Require the node revision returned by show.
        #[arg(long)]
        revision: Option<String>,
    },
}
#[derive(Subcommand)]
enum Project {
    /// Read the branch's Lean toolchain, Lake configuration and dependency lockfile.
    Show,
    /// Replace the branch's Lean toolchain, Lake configuration and lockfile from stdin JSON.
    Set {
        /// Require the branch revision returned by project show.
        #[arg(long)]
        revision: Option<String>,
    },
}
#[derive(Subcommand)]
enum Branch {
    /// Fork a new branch from the selected branch head.
    New { name: String },
    /// Delete the selected branch and its Lean caches; stop its service first.
    Del,
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
    let mut command = Cli::command().mut_subcommands(|subcommand| {
        let subcommand = subcommand.arg(
            clap::Arg::new("meas")
                .short('m')
                .long("meas")
                .action(clap::ArgAction::SetTrue)
                .global(true)
                .help("Print command timing measurements to stderr"),
        );
        if subcommand.is_hide_set() || command_group(subcommand.get_name()) == 0 {
            return subcommand;
        }
        subcommand.arg(
            clap::Arg::new("proj")
                .short('p')
                .long("proj")
                // Inherit only within this client command, never at the CLI root.
                .global(true)
                .value_name("DATABASE/BRANCH")
                .value_parser(parse_project)
                .help("Select a project branch (required)"),
        )
    });
    command.build();
    let header = command.get_styles().get_header();
    // Render together to align columns, then separate management, graph and node operations.
    let mut commands = command
        .clone()
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
    group_options(command.help_template(template))
}

fn group_options(command: clap::Command) -> clap::Command {
    let mut command = command.mut_args(|arg| {
        let order = match arg.get_id().as_str() {
            "help" => 0,
            "meas" => 1,
            _ => arg.get_display_order().saturating_add(2),
        };
        arg.display_order(order)
    });
    // Keep Clap's alignment and styling; add one separator before command-specific options.
    let mut help = command.render_help().ansi().to_string();
    let literal = command.get_styles().get_literal();
    let separator = command
        .get_arguments()
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "meas"))
        .filter_map(|arg| {
            let prefix = match arg.get_short() {
                Some(short) => format!("\n  {literal}-{short}"),
                None => format!("\n      {literal}--{}", arg.get_long()?),
            };
            help.find(&prefix)
        })
        .min();
    if let Some(separator) = separator {
        help.insert(separator, '\n');
    }
    command.override_help(help).mut_subcommands(group_options)
}

pub fn run() -> i32 {
    let matches = grouped_command().get_matches();
    let project = matches
        .subcommand()
        .and_then(|(_, args)| args.try_get_one::<String>("proj").ok().flatten())
        .cloned();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    crate::profile::set_enabled(
        matches
            .subcommand()
            .is_some_and(|(_, args)| args.get_flag("meas")),
    );
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
            let Some(info) = crate::service::start(project, port.unwrap_or(0)).await? else {
                return Ok(0);
            };
            Some(info)
        }
        Commands::Stop { project } => Some(crate::service::stop(project).await?),
        _ => None,
    };
    if let Some(value) = local {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
    let project = project
        .context("select a project with --proj DATABASE/BRANCH; use mdc status to list projects")?;
    if matches!(
        cli.command,
        Commands::Branch {
            command: Branch::Del
        }
    ) {
        let value = crate::service::delete_branch(&project).await?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
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
        | Commands::Branch {
            command: Branch::Del,
        } => unreachable!(),
        Commands::New {
            title,
            parent,
            revision,
        } => {
            let parent = match parent {
                Some(reference) => Some(api.node(&reference).await?),
                None => None,
            };
            let mut body = json!({"title":title});
            if let Some(parent) = &parent {
                body["parent_fnode"] = parent["fnode"].clone();
            }
            let response = api
                .request(
                    Method::POST,
                    "/node/new",
                    &[],
                    Some(body),
                    revision
                        .as_deref()
                        .or_else(|| parent.as_ref().and_then(|p| p["revision"].as_str())),
                )
                .await?;
            if let Some(parent) = parent {
                // The browser endpoint returns the updated parent; CLI new returns the new node.
                let id = response["depens"]
                    .as_array()
                    .context("invalid linked node response")?
                    .iter()
                    .find(|id| !parent["depens"].as_array().unwrap().contains(id))
                    .and_then(Value::as_str)
                    .context("new dependency missing from response")?;
                api.node(id).await?
            } else {
                response
            }
        }
        Commands::Show { source } => api.node(&source).await?,
        Commands::Edit {
            source,
            language,
            revision,
            delete,
        } => {
            let body = if delete {
                None
            } else {
                Some(json!({"content":stdin()?}))
            };
            let node = api.node(&source).await?;
            api.request(
                if delete { Method::DELETE } else { Method::PUT },
                &format!("/node/{}/block/{language}", node["fnode"].as_str().unwrap()),
                &[],
                body,
                Some(
                    revision
                        .as_deref()
                        .unwrap_or(node["revision"].as_str().unwrap()),
                ),
            )
            .await?
        }
        Commands::Rename {
            source,
            title,
            revision,
        } => {
            let n = api.node(&source).await?;
            api.request(
                Method::PUT,
                &format!("/node/{}/title", n["fnode"].as_str().unwrap()),
                &[],
                Some(json!({"title":title})),
                revision.as_deref().or(n["revision"].as_str()),
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
            Dep::Add {
                source,
                target,
                revision,
            } => mutate_dep(&api, &source, &[target], true, revision).await?,
            Dep::Rm {
                source,
                target,
                revision,
            } => mutate_dep(&api, &source, &target, false, revision).await?,
            Dep::Show { source, depth } => traverse(&api, &source, "show", depth).await?,
            Dep::Refs { target, depth } => traverse(&api, &target, "refs", depth).await?,
            Dep::Leaf { source } => traverse(&api, &source, "leaf", -1).await?,
            Dep::Candidates {
                source,
                query,
                max_results,
            } => {
                let n = api.node(&source).await?;
                api.request(
                    Method::GET,
                    &format!("/node/{}/dep/candidates", n["fnode"].as_str().unwrap()),
                    &[("q", query), ("n", max_results.to_string())],
                    None,
                    None,
                )
                .await?
            }
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
                revision,
            } => {
                let n = api.node(&source).await?;
                api.request(
                    Method::POST,
                    &format!("/node/{}/lean/goals", n["fnode"].as_str().unwrap()),
                    &[],
                    Some(json!({"line":line,"character":column})),
                    revision.as_deref().or(n["revision"].as_str()),
                )
                .await?
            }
        },
        Commands::Export => api.get("/export").await?,
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
            command: Project::Set { revision },
        } => {
            let project: Value = serde_json::from_str(&stdin()?)?;
            let p = api.get("/project/lean").await?;
            api.request(
                Method::PUT,
                "/project/lean",
                &[],
                Some(project),
                revision.as_deref().or(p["revision"].as_str()),
            )
            .await?
        }
        Commands::History => api.get("/history").await?,
        Commands::Branch {
            command: Branch::New { name },
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
async fn mutate_dep(
    api: &Api,
    source: &str,
    targets: &[String],
    add: bool,
    revision: Option<String>,
) -> Result<Value> {
    let n = api.node(source).await?;
    let mut ids = Vec::with_capacity(targets.len());
    for target in targets {
        ids.push(api.node(target).await?["fnode"].clone());
    }
    let body = if add {
        json!({"dep_fnode":ids[0]})
    } else {
        json!({"dep_fnodes":ids})
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
        revision.as_deref().or(n["revision"].as_str()),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn errors_only_advertise_public_commands() {
        for args in [
            vec!["mdc"],
            vec!["mdc", "-m"],
            vec!["mdc", "--meas"],
            vec!["mdc", "__rn"],
            vec!["mdc", "strt"],
        ] {
            let mut command = grouped_command();
            let error = command.try_get_matches_from_mut(args.clone()).unwrap_err();
            assert_eq!(error.exit_code(), 2);
            let text = error.to_string();
            assert!(!text.contains("__run"), "{args:?}: {text}");
            assert!(text.contains("--help"), "{args:?}: {text}");
            if args.last() == Some(&"strt") {
                assert!(text.contains("tip:") && text.contains("'start'"), "{text}");
            }
        }
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "__run", "db/main", "--port", "0"])
            .is_err());
    }

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
                vec!["search", "graph", "export", "import", "history", "branch", "project"],
                vec!["new", "dep", "show", "edit", "rename", "metric", "lean"],
            ]
        );
        assert!(!short.contains("__run"));
        assert!(!short.contains("Examples:"));
        assert!(!short.contains("--proj"));
        assert!(!short.contains("--meas"));
        for args in [
            vec!["mdc", "--help"],
            vec!["mdc", "status", "-h"],
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
            let help = error.to_string();
            let options: Vec<_> = help
                .split_once("Options:\n")
                .unwrap()
                .1
                .trim_end()
                .lines()
                .collect();
            assert!(
                options[0].trim_start().starts_with("-h, --help"),
                "{path:?}"
            );
            if path.len() == 1 {
                assert_eq!(options.len(), 1);
            } else {
                assert!(
                    options[1].trim_start().starts_with("-m, --meas"),
                    "{path:?}"
                );
            }
            if options.len() > 2 {
                assert_eq!(options[2], "", "{path:?}");
                assert!(options[3].trim_start().starts_with('-'), "{path:?}");
            }
            let mut short_args = path.clone();
            short_args.push("-h".into());
            assert_eq!(
                grouped_command()
                    .try_get_matches_from(short_args)
                    .unwrap_err()
                    .to_string(),
                help
            );
            assert!(!error
                .to_string()
                .lines()
                .any(|line| line.trim_start().starts_with("help ")));
            if command.has_subcommands() {
                let mut args = path.clone();
                args.push("help".into());
                let error = grouped_command().try_get_matches_from(args).unwrap_err();
                assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
            }
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
            vec!["branch", "new"],
            vec!["branch", "del"],
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
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "branch", "create", "agent", "--proj", "db/main"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "branch", "del", "--proj", "db/agent"])
            .is_ok());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "branch", "del", "agent", "--proj", "db/main"])
            .is_err());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "export", "--proj", "db/main"])
            .is_ok());
        assert!(grouped_command()
            .try_get_matches_from(["mdc", "export", "node", "--proj", "db/main"])
            .is_err());
        for args in [
            vec!["mdc", "graph", "check", "-m", "-p", "db/main"],
            vec!["mdc", "graph", "-p", "db/main", "check", "--meas"],
        ] {
            let matches = grouped_command().try_get_matches_from(args).unwrap();
            assert_eq!(
                matches
                    .subcommand()
                    .unwrap()
                    .1
                    .get_one::<String>("proj")
                    .unwrap(),
                "db/main"
            );
            assert!(matches.subcommand().unwrap().1.get_flag("meas"));
        }
        for args in [
            vec!["mdc", "status", "--prof"],
            vec!["mdc", "-m", "status"],
            vec!["mdc", "--meas", "status"],
            vec!["mdc", "status", "-p", "db/main"],
            vec!["mdc", "-p", "db/main", "graph", "check"],
        ] {
            assert!(grouped_command().try_get_matches_from(args).is_err());
        }
    }
}
