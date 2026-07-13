use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{
        self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

use super::{cwd, fmt_item, open_cache, require_mdcroot, BLD, CYN, GRN, RED, RST};

// ── Public entry point ────────────────────────────────────────────────────────

trait TerminalControl {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
}

struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), cursor::Show)
    }
}

struct TerminalGuard<C: TerminalControl> {
    control: C,
    raw: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
}

impl<C: TerminalControl> TerminalGuard<C> {
    fn new(control: C) -> Self {
        Self {
            control,
            raw: false,
            alternate_screen: false,
            cursor_hidden: false,
        }
    }

    fn enter(&mut self) -> io::Result<()> {
        self.control.enable_raw()?;
        self.raw = true;
        self.control.enter_alternate_screen()?;
        self.alternate_screen = true;
        self.control.hide_cursor()?;
        self.cursor_hidden = true;
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.cursor_hidden {
            if let Err(error) = self.control.show_cursor() {
                first_error = Some(error);
            }
            self.cursor_hidden = false;
        }
        if self.alternate_screen {
            if let Err(error) = self.control.leave_alternate_screen() {
                first_error.get_or_insert(error);
            }
            self.alternate_screen = false;
        }
        if self.raw {
            if let Err(error) = self.control.disable_raw() {
                first_error.get_or_insert(error);
            }
            self.raw = false;
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

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
        let roots = cache.global_root_items()?;
        roots
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no nodes in workspace"))?
            .fnode
    };

    let mut app = TuiApp::new(cache, mdcroot, start_fnode)?;

    let mut guard = TerminalGuard::new(CrosstermControl);
    guard.enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app);
    drop(terminal);
    let restore_result = guard.restore();
    result?;
    restore_result?;

    if !app.action_log.is_empty() {
        println!();
        for entry in &app.action_log {
            println!("{entry}");
        }
    }
    Ok(0)
}

// ── Data types ────────────────────────────────────────────────────────────────

type NodeInfo = crate::core::NodeSummary;

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
        fnode: String,
    },
}

struct TuiApp {
    mdcroot: PathBuf,
    cache: crate::indcache::IndCache,

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
    notify: Option<(String, bool, std::time::Instant)>, // (message, is_success, shown_at)
}

// ── TuiApp impl ───────────────────────────────────────────────────────────────

impl TuiApp {
    fn new(cache: crate::indcache::IndCache, mdcroot: PathBuf, fnode: String) -> Result<Self> {
        let mut app = TuiApp {
            mdcroot,
            cache,
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
            notify: None,
        };
        app.load_view(&fnode)?;
        Ok(app)
    }

    fn load_view(&mut self, fnode: &str) -> Result<()> {
        let f = self
            .cache
            .resolve_ref(fnode, None)
            .map(|(fnode, _, _)| fnode)
            .unwrap_or_else(|_| fnode.to_string());
        self.focused = self.cache.node_summary(&f)?;

        self.referrers = {
            let mut v = self.cache.direct_referrer_summaries(&f)?;
            v.sort_by_key(|n| std::cmp::Reverse(n.depth));
            v
        };

        self.children = {
            let mut v = self.cache.direct_dependency_summaries(&f)?;
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
            match crate::mdocnode::MdocNode::load(&self.mdcroot, &abs) {
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
        // The calling operation already called upsert_path on any modified file,
        // so the index and derived data are already up to date for that file.
        // Pick up concurrent external changes before rebuilding the view.
        self.cache.discover_workspace_changes()?;
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
        let (added, _, _) = graph.add_direct_dependency_ref(&dep_fnode)?;
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
    fn do_create_and_add_dep(&mut self, new_node: crate::mdocnode::MdocNode) -> Result<()> {
        let mut graph = crate::depgraph::DepGraph::new(self.mdcroot.clone(), &self.focused.fnode)?;
        let new_fnode = new_node.fnode.clone();
        let node_path = new_node.path.clone();
        let node_title = new_node.title.clone();
        let added = graph.create_and_add_dependency(new_node)?;
        if added {
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

    fn set_notify(&mut self, msg: impl Into<String>, success: bool) {
        self.notify = Some((msg.into(), success, std::time::Instant::now()));
    }
}

fn refresh_after_edit(app: &mut TuiApp, path: &std::path::Path) -> Result<()> {
    app.cache.upsert_path(path)?;
    let fnode = app.focused.fnode.clone();
    app.load_view(&fnode)
}

fn record_edit_result(app: &mut TuiApp, rel_path: &str, result: Result<()>) {
    match result {
        Ok(()) => {
            app.action_log.push(format!(
                "  {CYN}~{RST} {BLD}edit{RST}     {}",
                fmt_item(
                    &app.focused.fnode,
                    &app.focused.title,
                    rel_path,
                    app.focused.broken
                ),
            ));
            app.set_notify("file edited", true);
        }
        Err(error) => app.set_notify(format!("edit failed: {error}"), false),
    }
}

const NEW_NODE_SENTINEL: &str = "\x00new";

fn prepare_new_dependency_node(
    mdcroot: &std::path::Path,
    title: &str,
    raw_target: &str,
    fnode: &str,
) -> Result<crate::mdocnode::MdocNode> {
    let target = if raw_target.trim().is_empty() {
        "."
    } else {
        raw_target
    };
    let path = crate::depgraph::resolve_new_node_path(mdcroot, target, fnode)?;
    let mut node = crate::mdocnode::MdocNode::new_at_path(mdcroot, &path, title);
    node.fnode = fnode.to_string();
    Ok(node)
}

fn search_fields(cache: &crate::indcache::IndCache, q: &str) -> Result<Vec<NodeInfo>> {
    cache.search_with_metadata(q, 20)
}

/// Free function so it can be called while `app.overlay` is mutably borrowed.
fn adddep_search_fields(
    cache: &crate::indcache::IndCache,
    focused_fnode: &str,
    children: &[NodeInfo],
    q: &str,
) -> Result<Vec<NodeInfo>> {
    let existing: std::collections::HashSet<&str> = std::iter::once(focused_fnode)
        .chain(children.iter().map(|c| c.fnode.as_str()))
        .collect();
    let raw = cache.search_with_metadata(q, usize::MAX)?;
    let raw_had_matches = !raw.is_empty();
    let mut results: Vec<NodeInfo> = raw
        .into_iter()
        .filter(|item| !existing.contains(item.fnode.as_str()))
        .take(20)
        .collect();
    // Only offer to create if the raw search had zero matches — not if
    // all matches were filtered out because they're already dependencies.
    if results.is_empty() && !q.is_empty() && !raw_had_matches {
        results.push(NodeInfo {
            fnode: NEW_NODE_SENTINEL.to_string(),
            title: format!("✦ Create new: {q}"),
            rel_path: String::new(),
            broken: false,
            depth: 0,
        });
    }
    Ok(results)
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut TuiApp) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if !event::poll(std::time::Duration::from_millis(50))? {
            // Auto-expire notification after 3 seconds
            if app
                .notify
                .as_ref()
                .is_some_and(|(_, _, t)| t.elapsed().as_secs() >= 3)
            {
                app.notify = None;
            }
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        app.notify = None;

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
                KeyCode::Down if *sel + 1 < results.len() => {
                    *sel += 1;
                }
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    input.pop();
                    let q = input.clone();
                    *results = search_fields(&app.cache, &q)?;
                    *sel = 0;
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    let q = input.clone();
                    *results = search_fields(&app.cache, &q)?;
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
                KeyCode::Char('r') if !app.focused.broken && !app.children.is_empty() => {
                    let selected = vec![false; app.children.len()];
                    app.overlay = Overlay::RmDep {
                        selected,
                        cursor: 0,
                    };
                }
                KeyCode::Char('e') => {
                    let rel = app.focused.rel_path.clone();
                    if !rel.is_empty() {
                        let abs_path = app.mdcroot.join(&rel);
                        // Clear the alternate screen before leaving it so the transition
                        // shows a blank terminal rather than a flash of the TUI content.
                        execute!(
                            io::stdout(),
                            terminal::Clear(terminal::ClearType::All),
                            cursor::MoveTo(0, 0),
                        )?;
                        disable_raw_mode()?;
                        execute!(io::stdout(), LeaveAlternateScreen, cursor::Show)?;
                        let edit_result = super::cmd_core::launch_editor(&abs_path);
                        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
                        enable_raw_mode()?;
                        terminal.clear()?;
                        let edit_result =
                            edit_result.and_then(|_| refresh_after_edit(app, &abs_path));
                        record_edit_result(app, &rel, edit_result);
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
                                (
                                    None,
                                    Overlay::CreateDep {
                                        step: CreateStep::Title,
                                        title: q,
                                        file: String::new(),
                                        fnode: Uuid::new_v4().to_string(),
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
                        match app.do_add_dep(fnode, title, rel, broken) {
                            Ok(()) => {
                                app.set_notify("dep added", true);
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                app.action_log
                                    .push(format!("  {RED}✗{RST} {BLD}dep add failed:{RST} {msg}"));
                                let short = msg.lines().next().unwrap_or("dep add failed");
                                app.set_notify(short, false);
                            }
                        }
                    }
                }
                KeyCode::Down if *sel + 1 < results.len() => {
                    *sel += 1;
                }
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    input.pop();
                    let q = input.clone();
                    *results =
                        adddep_search_fields(&app.cache, &app.focused.fnode, &app.children, &q)?;
                    *sel = 0;
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    let q = input.clone();
                    *results =
                        adddep_search_fields(&app.cache, &app.focused.fnode, &app.children, &q)?;
                    *sel = 0;
                }
                _ => {}
            },

            // ── Remove dep overlay ────────────────────────────────────────────
            Overlay::RmDep { selected, cursor } => match key.code {
                KeyCode::Esc => app.overlay = Overlay::ActionMenu,
                KeyCode::Char('j') | KeyCode::Down if *cursor + 1 < app.children.len() => {
                    *cursor += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                }
                KeyCode::Char(' ') if *cursor < selected.len() => {
                    selected[*cursor] = !selected[*cursor];
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
                    match app.do_rm_deps(fnodes) {
                        Ok(()) => {
                            app.set_notify("deps removed", true);
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            app.action_log
                                .push(format!("  {RED}✗{RST} {BLD}dep rm failed:{RST} {msg}"));
                            let short = msg.lines().next().unwrap_or("dep rm failed");
                            app.set_notify(short, false);
                        }
                    }
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
                    let results =
                        adddep_search_fields(&app.cache, &app.focused.fnode, &app.children, &q)?;
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
                            title, file, fnode, ..
                        } = &app.overlay
                        {
                            Some((title.clone(), file.clone(), fnode.clone()))
                        } else {
                            None
                        };
                        if let Some((title, file, fnode)) = data {
                            app.overlay = Overlay::None;
                            let result =
                                prepare_new_dependency_node(&app.mdcroot, &title, &file, &fnode)
                                    .and_then(|node| app.do_create_and_add_dep(node));
                            match result {
                                Ok(()) => {
                                    app.set_notify("node created and added", true);
                                }
                                Err(e) => {
                                    let msg = e.to_string();
                                    app.action_log.push(format!(
                                        "  {RED}✗{RST} {BLD}dep add failed:{RST} {msg}"
                                    ));
                                    let short = msg.lines().next().unwrap_or("dep add failed");
                                    app.set_notify(short, false);
                                }
                            }
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

fn render(f: &mut ratatui::Frame, app: &mut TuiApp) {
    let area = f.area();

    let usable = area.width.saturating_sub(4);
    app.cards_per_row = ((usable + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).max(1) as usize;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(CARD_H),
            Constraint::Length(CARD_GAP),
            Constraint::Fill(1),
            Constraint::Length(CARD_GAP),
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
    // Draw operation notification in top-right corner (above all overlays)
    if let Some((ref msg, success, _)) = app.notify {
        render_notify(f, area, msg, success);
    }
}

// ── Notification rendering ────────────────────────────────────────────────────

fn render_notify(f: &mut ratatui::Frame, area: Rect, msg: &str, success: bool) {
    let color = if success { Color::Green } else { Color::Red };
    let icon = if success { "✓" } else { "✗" };
    let max_w = (area.width / 2).clamp(20, 60);
    let text: String = msg
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(max_w as usize - 4)
        .collect();
    let w = (text.chars().count() as u16 + 6).min(max_w);
    let h = 3u16;
    if area.width < w || area.height < h {
        return;
    }
    let x = area.x + area.width - w;
    let y = area.y;
    let r = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{icon} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(color)),
        ])),
        inner,
    );
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
            let h = (results.len().saturating_add(4)).clamp(5, 16) as u16;
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
            let h = (results.len().saturating_add(4)).clamp(6, 16) as u16;
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
            fnode,
        } => {
            let default_file = format!("{fnode}.mdoc");
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
            let h = (app.children.len().saturating_add(4)).clamp(6, 20) as u16;
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
        let tint = if is_referrers {
            Color::Magenta
        } else {
            Color::Blue
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
            tint,
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

fn render_card(f: &mut ratatui::Frame, area: Rect, node: &NodeInfo, selected: bool, tint: Color) {
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
            Style::default().fg(tint),
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
    crate::core::short_fnode(fnode)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct FakeTerminalControl {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail_on: Option<&'static str>,
    }

    impl FakeTerminalControl {
        fn call(&self, name: &'static str) -> io::Result<()> {
            self.calls.borrow_mut().push(name);
            if self.fail_on == Some(name) {
                Err(io::Error::other(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalControl for FakeTerminalControl {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.call("enable")
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.call("disable")
        }

        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.call("enter")
        }

        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.call("leave")
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.call("hide")
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.call("show")
        }
    }

    fn fake_guard(
        fail_on: Option<&'static str>,
    ) -> (
        TerminalGuard<FakeTerminalControl>,
        Rc<RefCell<Vec<&'static str>>>,
    ) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        (
            TerminalGuard::new(FakeTerminalControl {
                calls: Rc::clone(&calls),
                fail_on,
            }),
            calls,
        )
    }

    #[test]
    fn terminal_guard_restores_partial_initialization() {
        for (failure, expected) in [
            ("enable", vec!["enable"]),
            ("enter", vec!["enable", "enter", "disable"]),
            ("hide", vec!["enable", "enter", "hide", "leave", "disable"]),
        ] {
            let (mut guard, calls) = fake_guard(Some(failure));
            assert!(guard.enter().is_err());
            drop(guard);
            assert_eq!(*calls.borrow(), expected, "failure at {failure}");
        }
    }

    #[test]
    fn terminal_guard_restores_after_event_error_and_panic() {
        let (mut guard, calls) = fake_guard(None);
        guard.enter().unwrap();
        drop(guard);
        assert_eq!(
            *calls.borrow(),
            vec!["enable", "enter", "hide", "show", "leave", "disable"]
        );

        let (mut guard, calls) = fake_guard(None);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            guard.enter().unwrap();
            panic!("injected event-loop panic");
        }));
        assert!(panic.is_err());
        assert_eq!(
            *calls.borrow(),
            vec!["enable", "enter", "hide", "show", "leave", "disable"]
        );
    }

    #[test]
    fn terminal_guard_attempts_every_cleanup_after_failure() {
        for failure in ["show", "leave", "disable"] {
            let (mut guard, calls) = fake_guard(Some(failure));
            guard.enter().unwrap();
            calls.borrow_mut().clear();
            assert!(guard.restore().is_err());
            assert_eq!(
                *calls.borrow(),
                vec!["show", "leave", "disable"],
                "cleanup failure at {failure}"
            );
        }
    }

    #[test]
    fn default_dependency_path_uses_the_node_fnode() {
        let dir = tempfile::tempdir().unwrap();
        let fnode = "new-node-0001";
        let node = prepare_new_dependency_node(dir.path(), "New Node", "", fnode).unwrap();

        assert_eq!(node.fnode, fnode);
        assert_eq!(
            node.path.parent().unwrap(),
            dir.path().canonicalize().unwrap()
        );
        assert_eq!(
            node.path.file_name().unwrap(),
            std::ffi::OsStr::new(&format!("{fnode}.mdoc"))
        );
    }

    #[test]
    fn search_results_preserve_broken_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".mdc")).unwrap();
        std::fs::write(
            dir.path().join("invalid.mdoc"),
            "@fnode: invalid-node\n@title: Invalid Node\n@unknown: value\n",
        )
        .unwrap();
        let mut cache = crate::indcache::IndCache::open(dir.path().to_path_buf()).unwrap();
        cache.refresh_all().unwrap();

        let results = search_fields(&cache, "Invalid").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].broken);
    }

    fn test_app() -> (tempfile::TempDir, TuiApp, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join("node.mdoc");
        let node = crate::mdocnode::MdocNode::new_at_path(root, &path, "Editor");
        let fnode = node.fnode.clone();
        node.save().unwrap();
        let mut cache = crate::indcache::IndCache::open(root.to_path_buf()).unwrap();
        cache.refresh_all().unwrap();
        let app = TuiApp::new(cache, root.to_path_buf(), fnode).unwrap();
        (dir, app, path)
    }

    #[cfg(unix)]
    #[test]
    fn failed_editor_does_not_report_tui_success() {
        let (_dir, mut app, path) = test_app();
        let editor = which::which("false").unwrap();
        let result = super::super::cmd_core::launch_editor_with(editor.as_os_str(), &path);
        record_edit_result(&mut app, "node.mdoc", result);

        assert!(app.action_log.is_empty());
        let (message, success, _) = app.notify.as_ref().unwrap();
        assert!(!success);
        assert!(message.contains("editor exited"));
    }

    #[cfg(unix)]
    #[test]
    fn cache_refresh_failure_is_shown_as_tui_error() {
        use std::os::unix::fs::symlink;

        let (dir, mut app, path) = test_app();
        let outside = dir.path().join("outside.mdoc");
        std::fs::write(&outside, "@fnode: outside\n@title: Outside\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();

        let result = refresh_after_edit(&mut app, &path);
        assert!(result.is_err());
        record_edit_result(&mut app, "node.mdoc", result);
        assert!(app.action_log.is_empty());
        let (message, success, _) = app.notify.as_ref().unwrap();
        assert!(!success);
        assert!(message.contains("edit failed"));
    }
}
