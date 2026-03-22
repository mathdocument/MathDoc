mod cmd_core;
mod cmd_deps;
mod cmd_eval;
mod cmd_graph;
mod cmd_tui;
mod cmd_work;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::{DependencyItem, GraphIssue};
use crate::indcache::IndCache;
use crate::workspace::find_mdcroot;

// ── ANSI color helpers ────────────────────────────────────────────────────────

const RST: &str = "\x1b[0m";
const BLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GRN: &str = "\x1b[32m";
const YLW: &str = "\x1b[33m";
const CYN: &str = "\x1b[36m";

fn eprintln_err(msg: &str) {
    eprintln!("{RED}error:{RST} {msg}");
}

fn short_fnode(fnode: &str) -> &str {
    let s = fnode.trim_matches(|c| c == '<' || c == '>');
    &s[..s.len().min(8)]
}

const TITLE_WIDTH: usize = 40;
const PATH_WIDTH: usize = 40;

fn truncate_title(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    if chars.len() <= TITLE_WIDTH {
        let pad = TITLE_WIDTH - chars.len();
        format!("{}{}", title, " ".repeat(pad))
    } else {
        let truncated: String = chars[..TITLE_WIDTH - 1].iter().collect();
        format!("{truncated}…")
    }
}

fn truncate_path(path: &str) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= PATH_WIDTH {
        return path.to_string();
    }
    // Keep ".mdoc" suffix + "…"; visible prefix fills the rest
    const SUFFIX: &str = ".mdoc";
    let prefix_len = PATH_WIDTH - SUFFIX.len() - 1; // -1 for "…"
    let prefix: String = chars[..prefix_len].iter().collect();
    format!("{prefix}….mdoc")
}

fn fmt_item(fnode: &str, title: &str, rel_path: &str, broken: bool) -> String {
    let sf = short_fnode(fnode);
    let marker = if broken {
        format!("{RED}✗{RST} ")
    } else {
        String::new()
    };
    let title_col = truncate_title(title);
    let path_col = truncate_path(rel_path);
    format!("{DIM}{sf}{RST}  {marker}{BLD}{title_col}{RST}  {DIM}{path_col}{RST}")
}

// ── Clap command structure ────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "mdc", about = "MathDoc CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new MathDoc folder in the current directory.
    Init,

    /// Create a new mdoc file.
    New {
        #[arg(short, long, default_value = "Untitled")]
        title: String,
        /// Relative output file path (without .mdoc suffix). Defaults to {fnode}.mdoc at root.
        #[arg(short, long, default_value = ".")]
        file: String,
    },

    /// Open a mdoc file in $EDITOR and reindex it on exit.
    Edit { source: String },

    /// Force refresh all index entries.
    Sync,

    /// Search mdocs by title or fnode.
    Search {
        query: String,
        #[arg(short = 'n', long, default_value = "200")]
        max_results: usize,
    },

    /// Inspect the global dependency graph.
    Graph {
        #[command(subcommand)]
        command: GraphCommands,
    },

    /// Compile and run all blocks in a mdoc.
    Eval {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },

    /// Generate merged work files for editing in native tools.
    Work {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },

    /// Extract edits from work files and write back to mdoc files.
    Back,

    /// Manage mdoc dependencies.
    Dep {
        #[command(subcommand)]
        command: DepCommands,
    },
}

#[derive(Subcommand)]
enum GraphCommands {
    /// Scan the whole repo and report graph issues.
    Check,
    /// List all global root nodes with no incoming dependencies.
    Roots,
    /// Open interactive TUI graph browser.
    Tui {
        /// Start at this node (fnode prefix, path, or title). Defaults to deepest node.
        source: Option<String>,
    },
}

#[derive(Subcommand)]
enum DepCommands {
    /// Search and add dependencies to a mdoc.
    Add {
        source: String,
        query: String,
        #[arg(short = 'n', long, default_value = "200")]
        max_results: usize,
    },
    /// Show dependencies of a mdoc.
    Show {
        source: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
    /// Show all leaf dependencies (no further deps).
    Leaf { source: String },
    /// Interactively remove dependencies from a mdoc.
    Rm { source: String },
    /// Show reverse dependencies (who depends on this).
    Refs {
        target: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run() -> i32 {
    let cli = Cli::parse();
    match dispatch(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln_err(&e.to_string());
            1
        }
    }
}

fn dispatch(cmd: Commands) -> Result<i32> {
    match cmd {
        Commands::Init => cmd_core::cmd_init(),
        Commands::New { title, file } => cmd_core::cmd_new(title, file),
        Commands::Edit { source } => cmd_core::cmd_edit(source),
        Commands::Sync => cmd_core::cmd_sync(),
        Commands::Search { query, max_results } => cmd_core::cmd_search(query, max_results),
        Commands::Graph { command } => match command {
            GraphCommands::Check => cmd_graph::cmd_graph_check(),
            GraphCommands::Roots => cmd_graph::cmd_graph_roots(),
            GraphCommands::Tui { source } => cmd_tui::cmd_graph_tui(source),
        },
        Commands::Eval { source, depth } => cmd_eval::cmd_eval(source, depth),
        Commands::Work { source, depth } => cmd_work::cmd_work(source, depth),
        Commands::Back => cmd_work::cmd_back(),
        Commands::Dep { command } => match command {
            DepCommands::Add {
                source,
                query,
                max_results,
            } => cmd_deps::cmd_dep_add(source, query, max_results),
            DepCommands::Show { source, depth } => cmd_deps::cmd_dep_show(source, depth),
            DepCommands::Leaf { source } => cmd_deps::cmd_dep_leaf(source),
            DepCommands::Rm { source } => cmd_deps::cmd_dep_rm(source),
            DepCommands::Refs { target, depth } => cmd_deps::cmd_dep_refs(target, depth),
        },
    }
}

// ── Workspace helpers ─────────────────────────────────────────────────────────

fn require_mdcroot() -> Result<PathBuf> {
    find_mdcroot(&std::env::current_dir()?)
        .ok_or_else(|| anyhow::anyhow!("not inside an mdoc directory, run `mdc init` first"))
}

fn open_cache(mdcroot: PathBuf) -> Result<IndCache> {
    let mut cache = IndCache::open(mdcroot)?;
    cache.bootstrap_if_needed()?;
    Ok(cache)
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ── Shared output helpers ─────────────────────────────────────────────────────

/// Print a single cycle in vertical format with a left-side bracket.
/// `label_map`: fnode → (title, rel_path), used for display titles.
/// Cycles are stored as [A, B, …, A]; the repeated tail is stripped.
fn print_cycle(cycle: &[String], label_map: &HashMap<String, (String, String)>) {
    let nodes = if cycle.len() > 1 && cycle.first() == cycle.last() {
        &cycle[..cycle.len() - 1]
    } else {
        cycle
    };
    if nodes.is_empty() {
        return;
    }
    let fmt_node = |f: &str| -> String {
        match label_map.get(f) {
            Some((t, p)) => fmt_item(f, t, p, false),
            None => format!("{DIM}{}{RST}", short_fnode(f)),
        }
    };
    if nodes.len() == 1 {
        println!("    {YLW}↺{RST}  {}", fmt_node(&nodes[0]));
    } else {
        for (i, fnode) in nodes.iter().enumerate() {
            let item = fmt_node(fnode);
            if i == 0 {
                println!("    {DIM}┌➤{RST}  {item}");
            } else if i == nodes.len() - 1 {
                println!("    {DIM}└─{RST}  {item}");
            } else {
                println!("    {DIM}│{RST}   {item}");
            }
        }
    }
}

pub(super) fn print_cycles_if_any(cycles: &[Vec<String>], cache: &IndCache) {
    if cycles.is_empty() {
        return;
    }
    println!("   {RED}cycles ({}):{RST}", cycles.len());
    for cycle in cycles {
        let fnode_refs: Vec<&str> = cycle.iter().map(|s| s.as_str()).collect();
        let label_map = cache.lookup_by_fnode(&fnode_refs).unwrap_or_default();
        print_cycle(cycle, &label_map);
    }
}

fn print_missing_with_referrers(issues: &[GraphIssue], cache: &IndCache) {
    if issues.is_empty() {
        return;
    }
    println!("   {RED}missing ({}):{RST}", issues.len());
    for issue in issues {
        println!(
            "    {}",
            fmt_item(&issue.fnode, &issue.title, &issue.rel_path, true)
        );
        let refs = cache
            .direct_referrers_for_fnode(&issue.fnode)
            .unwrap_or_default();
        for (f, t, p) in &refs {
            println!("      ← {}", fmt_item(f, t, p, false));
        }
    }
}

fn print_dep_report(
    anchor_label: &str,
    anchor: &DependencyItem,
    count_label: &str,
    items: &[DependencyItem],
    issues: &HashMap<String, GraphIssue>,
) {
    println!(
        "{BLD}{anchor_label}{RST}  {}",
        fmt_item(&anchor.fnode, &anchor.title, &anchor.rel_path, false)
    );
    if items.is_empty() {
        println!("   {DIM}no {count_label}{RST}");
        return;
    }
    println!("   {BLD}{}{RST} {count_label}", items.len());
    let w = items
        .iter()
        .map(|i| i.depth)
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    for item in items {
        let broken = issues.contains_key(&item.fnode);
        println!(
            "   [{:>w$}]  {}",
            item.depth,
            fmt_item(&item.fnode, &item.title, &item.rel_path, broken)
        );
    }
}
