use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, CYN, GRN, RED, RST};

// ── Public entry point ────────────────────────────────────────────────────────

pub(super) fn cmd_graph_tui(source: Option<String>) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;

    let start_fnode = if let Some(ref s) = source {
        match cache.resolve_ref(s, Some(&cwd())) {
            Ok((f, _, _)) => f,
            Err(e) => anyhow::bail!("cannot resolve '{}': {}", s, e),
        }
    } else {
        let mut roots = cache.global_root_items()?;
        if roots.is_empty() {
            anyhow::bail!("no nodes in workspace");
        }
        let topo = cache.all_topo_depths().unwrap_or_default();
        roots.sort_by(|a, b| {
            let da = topo.get(&a.fnode).copied().unwrap_or(0);
            let db = topo.get(&b.fnode).copied().unwrap_or(0);
            db.cmp(&da).then(b.component_size.cmp(&a.component_size))
        });
        roots.into_iter().next().unwrap().fnode
    };

    let mut app = TuiApp::new(cache, mdcroot, start_fnode)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result?;

    if !app.action_log.is_empty() {
        println!();
        for entry in &app.action_log {
            println!("{entry}");
        }
    }
    Ok(0)
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct NodeInfo {
    fnode: String,
    title: String,
    rel_path: String,
    broken: bool,
    depth: u32,
}

#[derive(Clone, PartialEq)]
enum PreSel {
    None,
    Referrer(usize),
    Child(usize),
}

#[derive(PartialEq)]
enum CreateStep {
    Title,
    File,
}

/// All overlay states. `None` = normal browse.
enum Overlay {
    None,
    Search {
        input: String,
        results: Vec<NodeInfo>,
        sel: usize,
    },
    ActionMenu,
    AddDep {
        input: String,
        results: Vec<NodeInfo>,
        sel: usize,
    },
    RmDep {
        selected: Vec<bool>,
        cursor: usize,
    },
    CreateDep {
        step: CreateStep,
        title: String,
        file: String,
        default_file: String,
    },
}

struct TuiApp {
    mdcroot: PathBuf,
    cache: crate::indcache::IndCache,
    topo_depths: HashMap<String, u32>,

    focused: NodeInfo,
    referrers: Vec<NodeInfo>,
    children: Vec<NodeInfo>,

    ref_offset: usize,
    child_offset: usize,

    presel: PreSel,
    overlay: Overlay,
    cards_per_row: usize,

    preview_lines: Vec<String>,
    preview_offset: usize,
    in_preview: bool,
    action_log: Vec<String>,
}

// ── TuiApp impl ───────────────────────────────────────────────────────────────

impl TuiApp {
    fn new(cache: crate::indcache::IndCache, mdcroot: PathBuf, fnode: String) -> Result<Self> {
        let topo_depths = cache.all_topo_depths().unwrap_or_default();
        let mut app = TuiApp {
            mdcroot,
            cache,
            topo_depths,
            focused: NodeInfo {
                fnode: fnode.clone(),
                title: String::new(),
                rel_path: String::new(),
                broken: false,
                depth: 0,
            },
            referrers: vec![],
            children: vec![],
            ref_offset: 0,
            child_offset: 0,
            presel: PreSel::None,
            overlay: Overlay::None,
            cards_per_row: 4,
            preview_lines: vec![],
            preview_offset: 0,
            in_preview: false,
            action_log: vec![],
        };
        app.load_view(&fnode)?;
        Ok(app)
    }

    fn load_view(&mut self, fnode: &str) -> Result<()> {
        let (f, t, p) = self
            .cache
            .resolve_ref(fnode, None)
            .unwrap_or_else(|_| (fnode.to_string(), "<unknown>".into(), PathBuf::new()));
        let rel_path = crate::workspace::to_rel_path(&self.mdcroot, &p);
        let broken = self.cache.has_issues(&f).unwrap_or(false);
        let depth = self.topo_depths.get(&f).copied().unwrap_or(0);
        self.focused = NodeInfo {
            fnode: f.clone(),
            title: t,
            rel_path,
            broken,
            depth,
        };

        self.referrers = {
            let mut v: Vec<NodeInfo> = self
                .cache
                .direct_referrers_for_fnode(&f)?
                .into_iter()
                .map(|(rf, rt, rp)| {
                    let broken = self.cache.has_issues(&rf).unwrap_or(false);
                    let depth = self.topo_depths.get(&rf).copied().unwrap_or(0);
                    NodeInfo {
                        fnode: rf,
                        title: rt,
                        rel_path: rp,
                        broken,
                        depth,
                    }
                })
                .collect();
            v.sort_by_key(|n| std::cmp::Reverse(n.depth));
            v
        };

        self.children = {
            let report = self.cache.dependency_report(&f, 1)?;
            let mut v: Vec<NodeInfo> = report
                .items
                .into_iter()
                .filter(|i| i.depth == 1)
                .map(|i| {
                    let broken = report.issues_by_fnode.contains_key(&i.fnode);
                    let depth = self.topo_depths.get(&i.fnode).copied().unwrap_or(0);
                    NodeInfo {
                        broken,
                        fnode: i.fnode,
                        title: i.title,
                        rel_path: i.rel_path,
                        depth,
                    }
                })
                .collect();
            v.sort_by_key(|n| std::cmp::Reverse(n.depth));
            v
        };

        self.ref_offset = 0;
        self.child_offset = 0;
        self.presel = PreSel::None;
        self.in_preview = false;
        self.preview_offset = 0;

        self.preview_lines = if !self.focused.rel_path.is_empty() {
            let abs = self.mdcroot.join(&self.focused.rel_path);
            match crate::mdoc::MdocNode::load(&self.mdcroot, &abs) {
                Ok(node) => {
                    let mut lines: Vec<String> = Vec::new();
                    for (i, block) in node.blocks.iter().enumerate() {
                        if i > 0 {
                            lines.push(String::new());
                        }
                        lines.push(format!("@src: {}", block.srctype));
                        for line in block.content.lines() {
                            lines.push(line.to_string());
                        }
                    }
                    lines
                }
                Err(_) => vec![],
            }
        } else {
            vec![]
        };

        Ok(())
    }

    fn navigate_to(&mut self, fnode: &str) -> Result<()> {
        self.load_view(fnode)
    }

    fn clamp_offsets(&mut self) {
        let w = self.cards_per_row.max(1);
        if let PreSel::Referrer(i) = self.presel {
            if i < self.ref_offset {
                self.ref_offset = i;
            } else if i >= self.ref_offset + w {
                self.ref_offset = i + 1 - w;
            }
        }
        if let PreSel::Child(i) = self.presel {
            if i < self.child_offset {
                self.child_offset = i;
            } else if i >= self.child_offset + w {
                self.child_offset = i + 1 - w;
            }
        }
    }

    fn visible_referrers(&self) -> &[NodeInfo] {
        let w = self.cards_per_row.max(1);
        let start = self.ref_offset.min(self.referrers.len());
        let end = (start + w).min(self.referrers.len());
        &self.referrers[start..end]
    }

    fn visible_children(&self) -> &[NodeInfo] {
        let w = self.cards_per_row.max(1);
        let start = self.child_offset.min(self.children.len());
        let end = (start + w).min(self.children.len());
        &self.children[start..end]
    }

    // ── Dep operations ────────────────────────────────────────────────────────

    fn refresh_after_op(&mut self) -> Result<()> {
        self.cache.refresh_workspace_index()?;
        self.topo_depths = self.cache.all_topo_depths().unwrap_or_default();
        let fnode = self.focused.fnode.clone();
        self.load_view(&fnode)
    }

    fn do_add_dep(
        &mut self,
        dep_fnode: String,
        dep_title: String,
        dep_rel: String,
        dep_broken: bool,
    ) -> Result<()> {
        let mut graph = crate::depgraph::DepGraph::new(self.mdcroot.clone(), &self.focused.fnode)?;
        let (added, _, _) = graph.add_direct_dependencies(vec![dep_fnode.clone()])?;
        let root_path = graph.root_path()?;
        let _ = graph.cache.upsert_path(&root_path);
        if !added.is_empty() {
            let src = fmt_item(
                &self.focused.fnode,
                &self.focused.title,
                &self.focused.rel_path,
                self.focused.broken,
            );
            let dst = fmt_item(&dep_fnode, &dep_title, &dep_rel, dep_broken);
            self.action_log.push(format!(
                "  {GRN}+{RST} {BLD}dep add{RST}  {src}\n    → {dst}"
            ));
        }
        self.refresh_after_op()
    }

    /// Create a new node (already path-resolved) and add it as a dependency.
    fn do_create_and_add_dep(&mut self, new_node: crate::mdoc::MdocNode) -> Result<()> {
        let mut graph = crate::depgraph::DepGraph::new(self.mdcroot.clone(), &self.focused.fnode)?;
        new_node.save()?;
        graph.cache.upsert_path(&new_node.path)?;
        let new_fnode = new_node.fnode.clone();
        let node_path = new_node.path.clone();
        let node_title = new_node.title.clone();
        graph
            .state
            .nodes_by_fnode
            .insert(new_fnode.clone(), new_node);
        graph.state.dep_graph.entry(new_fnode.clone()).or_default();
        let (added, _, _) = graph.add_direct_dependencies(vec![new_fnode.clone()])?;
        let root_path = graph.root_path()?;
        let _ = graph.cache.upsert_path(&root_path);
        if !added.is_empty() {
            let rel = crate::workspace::to_rel_path(&self.mdcroot, &node_path);
            let focused_fnode = self.focused.fnode.clone();
            let focused_title = self.focused.title.clone();
            let focused_rel = self.focused.rel_path.clone();
            let focused_broken = self.focused.broken;
            self.action_log.push(format!(
                "  {GRN}+{RST} {BLD}new{RST}      {}",
                fmt_item(&new_fnode, &node_title, &rel, false),
            ));
            self.action_log.push(format!(
                "  {GRN}+{RST} {BLD}dep add{RST}  {}\n    → {}",
                fmt_item(&focused_fnode, &focused_title, &focused_rel, focused_broken),
                fmt_item(&new_fnode, &node_title, &rel, false),
            ));
        }
        self.refresh_after_op()
    }

    fn do_rm_deps(&mut self, fnodes: Vec<String>) -> Result<()> {
        if fnodes.is_empty() {
            return Ok(());
        }
        let mut graph = crate::depgraph::DepGraph::new(self.mdcroot.clone(), &self.focused.fnode)?;
        let removed = graph.remove_direct_dependencies(fnodes)?;
        let root_path = graph.root_path()?;
        let _ = graph.cache.upsert_path(&root_path);
        for fnode in &removed {
            let (title, rel, broken) = self
                .children
                .iter()
                .find(|c| &c.fnode == fnode)
                .map(|c| (c.title.clone(), c.rel_path.clone(), c.broken))
                .unwrap_or_default();
            let src = fmt_item(
                &self.focused.fnode,
                &self.focused.title,
                &self.focused.rel_path,
                self.focused.broken,
            );
            let dst = fmt_item(fnode, &title, &rel, broken);
            self.action_log.push(format!(
                "  {RED}-{RST} {BLD}dep rm{RST}   {src}\n    → {dst}"
            ));
        }
        self.refresh_after_op()
    }
}

const NEW_NODE_SENTINEL: &str = "\x00new";

/// Free function so it can be called while `app.overlay` is mutably borrowed.
fn adddep_search_fields(
    cache: &crate::indcache::IndCache,
    topo_depths: &HashMap<String, u32>,
    focused_fnode: &str,
    children: &[NodeInfo],
    q: &str,
) -> Vec<NodeInfo> {
    let existing: std::collections::HashSet<&str> = std::iter::once(focused_fnode)
        .chain(children.iter().map(|c| c.fnode.as_str()))
        .collect();
    let mut results: Vec<NodeInfo> = cache
        .search(q)
        .unwrap_or_default()
        .into_iter()
        .filter(|(f, _, _)| !existing.contains(f.as_str()))
        .take(20)
        .map(|(f, t, p)| NodeInfo {
            depth: topo_depths.get(&f).copied().unwrap_or(0),
            fnode: f,
            title: t,
            rel_path: p,
            broken: false,
        })
        .collect();
    if results.is_empty() && !q.is_empty() {
        results.push(NodeInfo {
            fnode: NEW_NODE_SENTINEL.to_string(),
            title: format!("✦ Create new: {q}"),
            rel_path: String::new(),
            broken: false,
            depth: 0,
        });
    }
    results
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut TuiApp) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match &mut app.overlay {
            // ── Search overlay ────────────────────────────────────────────────
            Overlay::Search {
                input,
                results,
                sel,
            } => match key.code {
                KeyCode::Esc => app.overlay = Overlay::None,
                KeyCode::Enter => {
                    if let Overlay::Search { results, sel, .. } = &app.overlay {
                        if let Some(node) = results.get(*sel) {
                            let fnode = node.fnode.clone();
                            app.navigate_to(&fnode)?;
                        }
                    }
                    app.overlay = Overlay::None;
                }
                KeyCode::Down => {
                    if *sel + 1 < results.len() {
                        *sel += 1;
                    }
                }
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    input.pop();
                    let q = input.clone();
                    let rows = app.cache.search(&q).unwrap_or_default();
                    *results = rows
                        .into_iter()
                        .take(20)
                        .map(|(f, t, p)| NodeInfo {
                            depth: app.topo_depths.get(&f).copied().unwrap_or(0),
                            fnode: f,
                            title: t,
                            rel_path: p,
                            broken: false,
                        })
                        .collect();
                    *sel = 0;
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    let q = input.clone();
                    let rows = app.cache.search(&q).unwrap_or_default();
                    *results = rows
                        .into_iter()
                        .take(20)
                        .map(|(f, t, p)| NodeInfo {
                            depth: app.topo_depths.get(&f).copied().unwrap_or(0),
                            fnode: f,
                            title: t,
                            rel_path: p,
                            broken: false,
                        })
                        .collect();
                    *sel = 0;
                }
                _ => {}
            },

            // ── Action menu ───────────────────────────────────────────────────
            Overlay::ActionMenu => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => app.overlay = Overlay::None,
                KeyCode::Char('a') if !app.focused.broken => {
                    app.overlay = Overlay::AddDep {
                        input: String::new(),
                        results: vec![],
                        sel: 0,
                    };
                }
                KeyCode::Char('r') if !app.focused.broken => {
                    if !app.children.is_empty() {
                        let selected = vec![false; app.children.len()];
                        app.overlay = Overlay::RmDep {
                            selected,
                            cursor: 0,
                        };
                    }
                }
                KeyCode::Char('e') => {
                    let rel = app.focused.rel_path.clone();
                    if !rel.is_empty() {
                        let abs_path = app.mdcroot.join(&rel);
                        disable_raw_mode()?;
                        execute!(io::stdout(), LeaveAlternateScreen)?;
                        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                        let _ = std::process::Command::new(&editor).arg(&abs_path).status();
                        execute!(io::stdout(), EnterAlternateScreen)?;
                        enable_raw_mode()?;
                        terminal.clear()?;
                        let _ = app.cache.upsert_path(&abs_path);
                        app.topo_depths = app.cache.all_topo_depths().unwrap_or_default();
                        let fnode = app.focused.fnode.clone();
                        app.load_view(&fnode)?;
                        app.action_log.push(format!(
                            "  {CYN}~{RST} {BLD}edit{RST}     {}",
                            fmt_item(
                                &app.focused.fnode,
                                &app.focused.title,
                                &rel,
                                app.focused.broken
                            ),
                        ));
                    }
                    app.overlay = Overlay::None;
                }
                _ => {}
            },

            // ── Add dep overlay ───────────────────────────────────────────────
            Overlay::AddDep {
                input,
                results,
                sel,
            } => match key.code {
                KeyCode::Esc => app.overlay = Overlay::ActionMenu,
                KeyCode::Enter => {
                    let (add_dep, next_overlay) = if let Overlay::AddDep {
                        results,
                        sel,
                        input,
                        ..
                    } = &app.overlay
                    {
                        if let Some(node) = results.get(*sel) {
                            let q = input.clone();
                            if node.fnode == NEW_NODE_SENTINEL {
                                let default_file = format!("{}.mdoc", Uuid::new_v4());
                                (
                                    None,
                                    Overlay::CreateDep {
                                        step: CreateStep::Title,
                                        title: q,
                                        file: String::new(),
                                        default_file,
                                    },
                                )
                            } else {
                                let dep = (
                                    node.fnode.clone(),
                                    node.title.clone(),
                                    node.rel_path.clone(),
                                    node.broken,
                                );
                                (Some(dep), Overlay::None)
                            }
                        } else {
                            (None, Overlay::None)
                        }
                    } else {
                        (None, Overlay::None)
                    };
                    app.overlay = next_overlay;
                    if let Some((fnode, title, rel, broken)) = add_dep {
                        app.do_add_dep(fnode, title, rel, broken)?;
                    }
                }
                KeyCode::Down => {
                    if *sel + 1 < results.len() {
                        *sel += 1;
                    }
                }
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    input.pop();
                    let q = input.clone();
                    *results = adddep_search_fields(
                        &app.cache,
                        &app.topo_depths,
                        &app.focused.fnode,
                        &app.children,
                        &q,
                    );
                    *sel = 0;
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    let q = input.clone();
                    *results = adddep_search_fields(
                        &app.cache,
                        &app.topo_depths,
                        &app.focused.fnode,
                        &app.children,
                        &q,
                    );
                    *sel = 0;
                }
                _ => {}
            },

            // ── Remove dep overlay ────────────────────────────────────────────
            Overlay::RmDep { selected, cursor } => match key.code {
                KeyCode::Esc => app.overlay = Overlay::ActionMenu,
                KeyCode::Char('j') | KeyCode::Down => {
                    if *cursor + 1 < app.children.len() {
                        *cursor += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                }
                KeyCode::Char(' ') => {
                    if *cursor < selected.len() {
                        selected[*cursor] = !selected[*cursor];
                    }
                }
                KeyCode::Enter => {
                    let fnodes: Vec<String> = if let Overlay::RmDep { selected, .. } = &app.overlay
                    {
                        selected
                            .iter()
                            .enumerate()
                            .filter(|(_, &s)| s)
                            .map(|(i, _)| app.children[i].fnode.clone())
                            .collect()
                    } else {
                        vec![]
                    };
                    app.do_rm_deps(fnodes)?;
                    app.overlay = Overlay::None;
                }
                _ => {}
            },

            // ── Create dep overlay ────────────────────────────────────────────
            Overlay::CreateDep { .. } => match key.code {
                KeyCode::Esc => {
                    let q = if let Overlay::CreateDep { title, .. } = &app.overlay {
                        title.clone()
                    } else {
                        String::new()
                    };
                    let results = adddep_search_fields(
                        &app.cache,
                        &app.topo_depths,
                        &app.focused.fnode,
                        &app.children,
                        &q,
                    );
                    app.overlay = Overlay::AddDep {
                        input: q,
                        results,
                        sel: 0,
                    };
                }
                KeyCode::Enter => {
                    let is_title = if let Overlay::CreateDep { step, .. } = &app.overlay {
                        *step == CreateStep::Title
                    } else {
                        false
                    };
                    if is_title {
                        if let Overlay::CreateDep { step, .. } = &mut app.overlay {
                            *step = CreateStep::File;
                        }
                    } else {
                        let data = if let Overlay::CreateDep {
                            title,
                            file,
                            default_file,
                            ..
                        } = &app.overlay
                        {
                            let actual = if file.is_empty() {
                                default_file.clone()
                            } else {
                                file.clone()
                            };
                            Some((title.clone(), actual))
                        } else {
                            None
                        };
                        if let Some((title, actual_file)) = data {
                            let file_path = if actual_file.ends_with(".mdoc") {
                                app.mdcroot.join(&actual_file)
                            } else {
                                app.mdcroot.join(format!("{actual_file}.mdoc"))
                            };
                            app.overlay = Overlay::None;
                            let new_node = crate::mdoc::MdocNode::new_at_path(
                                &app.mdcroot,
                                &file_path,
                                &title,
                            );
                            app.do_create_and_add_dep(new_node)?;
                        }
                    }
                }
                KeyCode::Backspace => {
                    if let Overlay::CreateDep {
                        step, title, file, ..
                    } = &mut app.overlay
                    {
                        if *step == CreateStep::Title {
                            title.pop();
                        } else {
                            file.pop();
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Overlay::CreateDep {
                        step, title, file, ..
                    } = &mut app.overlay
                    {
                        if *step == CreateStep::Title {
                            title.push(c);
                        } else {
                            file.push(c);
                        }
                    }
                }
                _ => {}
            },

            // ── Browse mode ───────────────────────────────────────────────────
            Overlay::None => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('/') => {
                    app.overlay = Overlay::Search {
                        input: String::new(),
                        results: vec![],
                        sel: 0,
                    };
                }
                KeyCode::Char('k') => {
                    if app.in_preview && app.presel == PreSel::None {
                        app.preview_offset = app.preview_offset.saturating_sub(1);
                    } else {
                        match app.presel.clone() {
                            PreSel::Child(_) => {
                                app.presel = PreSel::None;
                                app.in_preview = false;
                            }
                            PreSel::None => {
                                if !app.referrers.is_empty() {
                                    let mid = app.ref_offset + app.cards_per_row / 2;
                                    app.presel = PreSel::Referrer(mid.min(app.referrers.len() - 1));
                                    app.in_preview = false;
                                    app.clamp_offsets();
                                }
                            }
                            PreSel::Referrer(_) => {}
                        }
                    }
                }
                KeyCode::Char('j') => {
                    if app.in_preview && app.presel == PreSel::None {
                        let max_off = app.preview_lines.len().saturating_sub(1);
                        app.preview_offset = (app.preview_offset + 1).min(max_off);
                    } else {
                        match app.presel.clone() {
                            PreSel::Referrer(_) => {
                                app.presel = PreSel::None;
                                app.in_preview = false;
                            }
                            PreSel::None => {
                                if !app.children.is_empty() {
                                    let mid = app.child_offset + app.cards_per_row / 2;
                                    app.presel = PreSel::Child(mid.min(app.children.len() - 1));
                                    app.in_preview = false;
                                    app.clamp_offsets();
                                }
                            }
                            PreSel::Child(_) => {}
                        }
                    }
                }
                KeyCode::Char('h') => match app.presel.clone() {
                    PreSel::None => {
                        app.in_preview = false;
                    }
                    PreSel::Referrer(i) if i > 0 => {
                        app.presel = PreSel::Referrer(i - 1);
                        app.clamp_offsets();
                    }
                    PreSel::Child(i) if i > 0 => {
                        app.presel = PreSel::Child(i - 1);
                        app.clamp_offsets();
                    }
                    _ => {}
                },
                KeyCode::Char('l') => match app.presel.clone() {
                    PreSel::None => {
                        app.in_preview = true;
                    }
                    PreSel::Referrer(i) if i + 1 < app.referrers.len() => {
                        app.presel = PreSel::Referrer(i + 1);
                        app.clamp_offsets();
                    }
                    PreSel::Child(i) if i + 1 < app.children.len() => {
                        app.presel = PreSel::Child(i + 1);
                        app.clamp_offsets();
                    }
                    _ => {}
                },
                KeyCode::Char(' ') | KeyCode::Enter => match &app.presel {
                    PreSel::None => {
                        if !app.focused.broken {
                            app.overlay = Overlay::ActionMenu;
                        }
                    }
                    PreSel::Referrer(i) => {
                        if let Some(node) = app.referrers.get(*i) {
                            if !node.broken {
                                let fnode = node.fnode.clone();
                                app.navigate_to(&fnode)?;
                            }
                        }
                    }
                    PreSel::Child(i) => {
                        if let Some(node) = app.children.get(*i) {
                            if !node.broken {
                                let fnode = node.fnode.clone();
                                app.navigate_to(&fnode)?;
                            }
                        }
                    }
                },
                _ => {}
            },
        }
    }
}

// ── Main render ───────────────────────────────────────────────────────────────

const CARD_WIDTH: u16 = 30;
const CARD_GAP: u16 = 2;
const CARD_H: u16 = 6; // border + fnode + up to 4 title lines (wrapped) + border
const CENTER_H: u16 = 10;

fn render(f: &mut ratatui::Frame, app: &mut TuiApp) {
    let area = f.area();

    let usable = area.width.saturating_sub(4);
    app.cards_per_row = ((usable + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).max(1) as usize;

    let fixed = CARD_H + CARD_H + CENTER_H + 1; // +1 = status bar
    let flex = area.height.saturating_sub(fixed);
    let edge_h = (flex / 2).max(2);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(CARD_H),
            Constraint::Length(edge_h),
            Constraint::Length(CENTER_H),
            Constraint::Length(edge_h),
            Constraint::Length(CARD_H),
            Constraint::Length(1),
        ])
        .split(area);

    render_node_row(f, chunks[0], app, true);
    // render_edges(f, chunks[1], app, true);
    render_center(f, chunks[2], app);
    // render_edges(f, chunks[3], app, false);
    render_node_row(f, chunks[4], app, false);
    render_status(f, chunks[5], app);

    // Draw overlays on top (do not disturb layout)
    render_overlay(f, area, app);
}

// ── Overlay rendering ─────────────────────────────────────────────────────────

fn render_overlay(f: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    match &app.overlay {
        Overlay::None => {}
        Overlay::Search {
            input,
            results,
            sel,
        } => {
            let h = (results.len() as u16 + 4).min(16).max(5);
            let r = overlay_rect(area, 70, h);
            f.render_widget(Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(" search ", Style::default().fg(Color::Yellow)));
            let inner = block.inner(r);
            f.render_widget(block, r);
            render_list_with_input(
                f,
                inner,
                &format!("/{input}█"),
                results,
                *sel,
                Color::Yellow,
            );
        }
        Overlay::ActionMenu => {
            let is_broken = app.focused.broken;
            let has_add = !is_broken;
            let has_rm = !is_broken && !app.children.is_empty();
            let has_edit = !app.focused.rel_path.is_empty();
            let item_count = if has_add { 1 } else { 0 }
                + if has_rm { 1 } else { 0 }
                + if has_edit { 1 } else { 0 };
            let h = 2 + item_count + 1; // border + items + esc hint
            let r = overlay_rect(area, 40, h.max(4));
            f.render_widget(Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " actions ",
                    Style::default().fg(Color::DarkGray),
                ));
            let inner = block.inner(r);
            f.render_widget(block, r);

            let mut lines = vec![];
            if has_add {
                lines.push(Line::from(vec![
                    Span::styled(
                        "[a] ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("add dependency"),
                ]));
            }
            if has_rm {
                lines.push(Line::from(vec![
                    Span::styled(
                        "[r] ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("remove dependency"),
                ]));
            }
            if has_edit {
                lines.push(Line::from(vec![
                    Span::styled(
                        "[e] ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("edit file"),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("[Esc] ", Style::default().fg(Color::DarkGray)),
                Span::styled("cancel", Style::default().fg(Color::DarkGray)),
            ]));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::AddDep {
            input,
            results,
            sel,
        } => {
            let h = (results.len() as u16 + 4).min(16).max(6);
            let r = overlay_rect(area, 70, h);
            f.render_widget(Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " add dependency ",
                    Style::default().fg(Color::Cyan),
                ));
            let inner = block.inner(r);
            f.render_widget(block, r);
            render_list_with_input(f, inner, &format!("/{input}█"), results, *sel, Color::Cyan);
        }
        Overlay::CreateDep {
            step,
            title,
            file,
            default_file,
        } => {
            let r = overlay_rect(area, 60, 7);
            f.render_widget(Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(" new node ", Style::default().fg(Color::Cyan)));
            let inner = block.inner(r);
            f.render_widget(block, r);

            let active_title = *step == CreateStep::Title;
            let title_line = {
                let (prefix, style) = if active_title {
                    (
                        "▶ ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("  ", Style::default().fg(Color::Gray))
                };
                let s = if active_title {
                    format!("title  {}█", title)
                } else {
                    format!("title  {}", title)
                };
                Line::from(Span::styled(format!("{prefix}{s}"), style))
            };
            let file_line = if !active_title {
                // Active: cursor at start; if nothing typed, show default as gray hint
                if file.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "▶ file   ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(default_file.as_str(), Style::default().fg(Color::DarkGray)),
                    ])
                } else {
                    Line::from(Span::styled(
                        format!("▶ file   {}█", file),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ))
                }
            } else {
                let fd = if file.is_empty() {
                    default_file.as_str()
                } else {
                    file.as_str()
                };
                Line::from(Span::styled(
                    format!("  file   {}", fd),
                    Style::default().fg(Color::DarkGray),
                ))
            };
            let hint = Line::from(Span::styled(
                "Enter:next/create  Esc:back",
                Style::default().fg(Color::DarkGray),
            ));
            f.render_widget(
                Paragraph::new(vec![title_line, file_line, Line::from(""), hint]),
                inner,
            );
        }
        Overlay::RmDep { selected, cursor } => {
            let n = app.children.len() as u16;
            let h = (n + 4).min(20).max(6);
            let r = overlay_rect(area, 70, h);
            f.render_widget(Clear, r);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title(Span::styled(
                    " remove dependencies ",
                    Style::default().fg(Color::Red),
                ));
            let inner = block.inner(r);
            f.render_widget(block, r);

            let max_visible = inner.height.saturating_sub(1) as usize;
            let scroll = if *cursor >= max_visible {
                cursor + 1 - max_visible
            } else {
                0
            };

            let mut lines: Vec<Line> = app
                .children
                .iter()
                .enumerate()
                .skip(scroll)
                .take(max_visible)
                .map(|(i, node)| {
                    let checked = if selected[i] { "✓" } else { " " };
                    let is_cur = i == *cursor;
                    let sf = short_fnode_display(&node.fnode);
                    let label = format!("[{checked}] [{}] {sf}  {}", node.depth, &node.title);
                    let style = if is_cur {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if selected[i] {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    Line::from(Span::styled(label, style))
                })
                .collect();
            lines.push(Line::from(Span::styled(
                "Space:toggle  Enter:confirm  Esc:cancel",
                Style::default().fg(Color::DarkGray),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
    }
}

/// Render a search-style list with an input prompt line.
fn render_list_with_input(
    f: &mut ratatui::Frame,
    area: Rect,
    prompt: &str,
    results: &[NodeInfo],
    sel: usize,
    accent: Color,
) {
    let max_results = area.height.saturating_sub(2) as usize; // reserve 1 prompt + 1 hint
    let mut lines = vec![Line::from(Span::styled(
        prompt,
        Style::default().fg(accent),
    ))];
    for (i, node) in results.iter().take(max_results).enumerate() {
        let is_sel = i == sel;
        let sf = short_fnode_display(&node.fnode);
        let label = if node.fnode == NEW_NODE_SENTINEL {
            node.title.clone()
        } else {
            format!("[{}] {sf}  {}", node.depth, node.title)
        };
        let style = if is_sel {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if is_sel { "▶ " } else { "  " };
        lines.push(Line::from(Span::styled(format!("{prefix}{label}"), style)));
    }
    if results.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no results)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(Span::styled(
        "↑↓:select  Enter:confirm  Esc:back",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines), area);
}

/// Center an overlay of given width% and fixed height in the area.
fn overlay_rect(area: Rect, percent_w: u16, height: u16) -> Rect {
    let w = area.width * percent_w / 100;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: height.min(area.height),
    }
}

// ── Node row / card rendering ─────────────────────────────────────────────────

fn render_node_row(f: &mut ratatui::Frame, area: Rect, app: &TuiApp, is_referrers: bool) {
    let nodes = if is_referrers {
        app.visible_referrers()
    } else {
        app.visible_children()
    };
    let offset = if is_referrers {
        app.ref_offset
    } else {
        app.child_offset
    };
    let total = if is_referrers {
        app.referrers.len()
    } else {
        app.children.len()
    };

    let left_arrow = offset > 0;
    let right_arrow = offset + app.cards_per_row < total;

    let x_start = area.x + 2;
    let mut x = x_start;

    for (local_idx, node) in nodes.iter().enumerate() {
        if x + CARD_WIDTH > area.x + area.width.saturating_sub(2) {
            break;
        }
        let abs_idx = offset + local_idx;
        let is_presel = if is_referrers {
            app.presel == PreSel::Referrer(abs_idx)
        } else {
            app.presel == PreSel::Child(abs_idx)
        };
        render_card(
            f,
            Rect {
                x,
                y: area.y,
                width: CARD_WIDTH,
                height: CARD_H,
            },
            node,
            is_presel,
        );
        x += CARD_WIDTH + CARD_GAP;
    }

    if left_arrow {
        f.render_widget(
            Paragraph::new("◄").style(Style::default().fg(Color::Yellow)),
            Rect {
                x: area.x,
                y: area.y + 1,
                width: 1,
                height: 1,
            },
        );
    }
    if right_arrow {
        let ax = (area.x + area.width).saturating_sub(2);
        f.render_widget(
            Paragraph::new("►").style(Style::default().fg(Color::Yellow)),
            Rect {
                x: ax,
                y: area.y + 1,
                width: 1,
                height: 1,
            },
        );
    }
}

fn render_card(f: &mut ratatui::Frame, area: Rect, node: &NodeInfo, selected: bool) {
    let border_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if node.broken {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let inner_w = (CARD_WIDTH as usize).saturating_sub(2);
    let sf = short_fnode_display(&node.fnode);
    let depth_fnode = format!("[{}] {}", node.depth, sf);

    let (fnode_style, title_style) = if selected {
        (
            Style::default().fg(Color::Cyan),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else if node.broken {
        (
            Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
            Style::default().fg(Color::Red),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Gray),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Fnode + depth on first line; title wrapped across remaining lines
    let fnode_line = Line::from(Span::styled(
        truncate_str(&depth_fnode, inner_w),
        fnode_style,
    ));
    let title_para = Paragraph::new(vec![
        fnode_line,
        Line::from(Span::styled(node.title.clone(), title_style)),
    ])
    .wrap(Wrap { trim: true });
    f.render_widget(title_para, inner);
}

fn render_center(f: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
        .split(area);
    render_center_info(f, chunks[0], app);
    if chunks[1].width > 4 {
        render_center_preview(f, chunks[1], app);
    }
}

fn render_center_info(f: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let node = &app.focused;
    let is_center_active =
        app.presel == PreSel::None && !app.in_preview && matches!(app.overlay, Overlay::None);
    let border_style = if node.broken {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if is_center_active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            " focused ",
            Style::default().fg(Color::DarkGray),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sf = node.fnode.get(..8).unwrap_or(&node.fnode);
    let depth_fnode = format!("[{}] {}", node.depth, sf);
    let text = vec![
        Line::from(Span::styled(
            depth_fnode,
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            node.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            node.rel_path.clone(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
}

fn render_center_preview(f: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let is_preview_active =
        app.in_preview && app.presel == PreSel::None && matches!(app.overlay, Overlay::None);
    let border_color = if is_preview_active {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(" preview ", Style::default().fg(border_color)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = app
        .preview_lines
        .iter()
        .map(|l| {
            let style = if l.starts_with("@src:") {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            Line::from(Span::styled(l.as_str(), style))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.preview_offset as u16, 0)),
        inner,
    );
}

// ── Status bar ────────────────────────────────────────────────────────────────

fn render_status(f: &mut ratatui::Frame, area: Rect, app: &TuiApp) {
    let presel_hint = match &app.presel {
        PreSel::None => "center".to_string(),
        PreSel::Referrer(i) => app
            .referrers
            .get(*i)
            .map(|n| format!("ref [{}] {}", n.depth, short_fnode_display(&n.fnode)))
            .unwrap_or_else(|| "referrer".to_string()),
        PreSel::Child(i) => app
            .children
            .get(*i)
            .map(|n| format!("dep [{}] {}", n.depth, short_fnode_display(&n.fnode)))
            .unwrap_or_else(|| "child".to_string()),
    };
    let hint = format!(
        " sel:{presel_hint}  jk:↑↓  hl:←→  space:enter/action  /:search  q:quit  │  {} ref  {} dep",
        app.referrers.len(),
        app.children.len(),
    );
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

// ── Display helpers ───────────────────────────────────────────────────────────

fn short_fnode_display(fnode: &str) -> &str {
    let s = fnode.trim_matches(|c| c == '<' || c == '>');
    s.get(..8).unwrap_or(s)
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars == 0 {
        String::new()
    } else {
        let t: String = chars[..max_chars - 1].iter().collect();
        format!("{t}…")
    }
}
