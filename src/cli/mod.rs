//! CLI entry point, clap command structure, shared helpers, and output utilities.

mod cmd_core;
mod cmd_deps;
mod cmd_eval;
mod cmd_graph;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::core::{DependencyItem, GraphIssue};
use crate::indcache::IndCache;
use crate::workspace::find_mdcroot;

// ── ANSI color helpers ────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

fn eprintln_err(msg: &str) {
    eprintln!("{RED}error:{RESET} {msg}");
}

fn eprintln_warn(msg: &str) {
    eprintln!("{YELLOW}warn:{RESET} {msg}");
}

fn short_fnode(fnode: &str) -> &str {
    let s = fnode.trim_matches(|c| c == '<' || c == '>');
    &s[..s.len().min(8)]
}

const TITLE_WIDTH: usize = 32;

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

fn fmt_item(fnode: &str, title: &str, rel_path: &str, broken: bool) -> String {
    let sf = short_fnode(fnode);
    let marker = if broken {
        format!("{RED}✗{RESET} ")
    } else {
        String::new()
    };
    let title_col = truncate_title(title);
    format!("{DIM}{sf}{RESET}  {marker}{BOLD}{title_col}{RESET}  {DIM}{rel_path}{RESET}")
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

    /// Open a mdoc with $EDITOR and refresh its index entry.
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

    /// Manage mdoc dependencies.
    Dep {
        #[command(subcommand)]
        command: DepCommands,
    },
}

#[derive(Subcommand)]
enum GraphCommands {
    /// Scan the whole repo and report graph issues.
    Check {
        /// Refresh the workspace index before checking.
        #[arg(long)]
        full: bool,
    },
    /// List all global root nodes with no incoming dependencies.
    Roots {
        /// Refresh the workspace index first.
        #[arg(long)]
        refresh: bool,
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
        /// Refresh the cached dependency subgraph first.
        #[arg(long)]
        refresh: bool,
    },
    /// Show all leaf dependencies (no further deps).
    Leaf {
        source: String,
        #[arg(long)]
        refresh: bool,
    },
    /// Interactively remove dependencies from a mdoc.
    Rm { source: String },
    /// Show reverse dependencies (who depends on this).
    Refs {
        target: String,
        #[arg(short, long, default_value = "1", allow_hyphen_values = true)]
        depth: i32,
        #[arg(long)]
        refresh: bool,
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
            GraphCommands::Check { full } => cmd_graph::cmd_graph_check(full),
            GraphCommands::Roots { refresh } => cmd_graph::cmd_graph_roots(refresh),
        },
        Commands::Eval { source, depth } => cmd_eval::cmd_eval(source, depth),
        Commands::Dep { command } => match command {
            DepCommands::Add {
                source,
                query,
                max_results,
            } => cmd_deps::cmd_dep_add(source, query, max_results),
            DepCommands::Show {
                source,
                depth,
                refresh,
            } => cmd_deps::cmd_dep_show(source, depth, refresh),
            DepCommands::Leaf { source, refresh } => cmd_deps::cmd_dep_leaf(source, refresh),
            DepCommands::Rm { source } => cmd_deps::cmd_dep_rm(source),
            DepCommands::Refs {
                target,
                depth,
                refresh,
            } => cmd_deps::cmd_dep_refs(target, depth, refresh),
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

fn print_dep_report(
    anchor_label: &str,
    anchor: &DependencyItem,
    count_label: &str,
    items: &[DependencyItem],
    issues: &HashMap<String, GraphIssue>,
) {
    println!(
        "{BOLD}{anchor_label}{RESET}  {}",
        fmt_item(&anchor.fnode, &anchor.title, &anchor.rel_path, false)
    );
    if items.is_empty() {
        println!("  {DIM}no {count_label}{RESET}");
        return;
    }
    println!("  {BOLD}{}{RESET} {count_label}", items.len());
    for item in items {
        let broken = issues.contains_key(&item.fnode);
        println!(
            "  [{}]  {}",
            item.depth,
            fmt_item(&item.fnode, &item.title, &item.rel_path, broken)
        );
    }
}
