use std::collections::HashSet;
use std::io::{self, BufRead, Write};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
    Terminal,
};

use crate::core::DependencyItem;
use crate::depgraph::DepGraph;
use crate::mdoc::MdocNode;

use super::{
    cwd, eprintln_warn, fmt_item, open_cache, print_cycle, print_dep_report, require_mdcroot, BOLD,
    RED, RESET, TITLE_WIDTH,
};

// ── cmd: dep show ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_show(source: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.refresh_reachable_from_path(&src_path, depth);
    }
    let source_item = cache
        .resolve_ref(&source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    let report = cache.dependency_report(&source_item.fnode, depth)?;
    print_dep_report(
        "source",
        &source_item,
        "depens",
        &report.items,
        &report.issues_by_fnode,
    );
    print_cycles_if_any(&report.cycles, &cache);
    Ok(0)
}

// ── cmd: dep leaf ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_leaf(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&source, Some(&cwd())) {
        let _ = cache.refresh_reachable_from_path(&src_path, -1);
    }
    let source_item = cache
        .resolve_ref(&source, Some(&cwd()))
        .map(|(f, t, p)| DependencyItem {
            depth: 0,
            fnode: f,
            title: t,
            rel_path: crate::workspace::to_rel_path(&mdcroot, &p),
        })?;
    let report = cache.leaf_dependency_report(&source_item.fnode)?;
    print_dep_report(
        "source",
        &source_item,
        "leaves",
        &report.items,
        &report.issues_by_fnode,
    );
    print_cycles_if_any(&report.cycles, &cache);
    Ok(0)
}

// ── cmd: dep add ──────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_add(source: String, query: String, max_results: usize) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot)?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;

    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(anyhow::anyhow!("query cannot be empty"));
    }
    let all_rows = graph.cache.search(&q)?;
    let existing_fnodes: HashSet<String> = {
        let direct = graph.direct_dependency_fnodes().unwrap_or_default();
        std::iter::once(source_item.fnode.clone())
            .chain(direct)
            .collect()
    };
    let candidates: Vec<_> = all_rows
        .iter()
        .filter(|(f, _, _)| !existing_fnodes.contains(f))
        .take(max_results)
        .collect();

    if candidates.is_empty() {
        print!("\nNo results for '{q}'. Create a new note titled '{q}'? [y/N]: ");
        io::stdout().flush()?;
        let answer = io::stdin()
            .lock()
            .lines()
            .next()
            .and_then(|l| l.ok())
            .unwrap_or_default();
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Canceled");
            return Ok(0);
        }
        let mdcroot = graph.mdcroot.clone();
        let mut new_node = MdocNode::new_at_path(&mdcroot, &mdcroot, &q);
        new_node.path = mdcroot.join(format!("{}.mdoc", new_node.fnode));
        new_node.save()?;
        graph.cache.upsert_path(&new_node.path)?;
        let new_fnode = new_node.fnode.clone();
        let node_path = new_node.path.clone();
        graph
            .state
            .nodes_by_fnode
            .insert(new_fnode.clone(), new_node);
        graph.state.dep_graph.entry(new_fnode.clone()).or_default();
        let (added, _, _) = graph.add_direct_dependencies(vec![new_fnode.clone()])?;
        let root_path = graph.root_path()?;
        if let Err(e) = graph.cache.upsert_path(&root_path) {
            eprintln_warn(&format!("index update failed: {e}"));
        }
        if !added.is_empty() {
            let rel = crate::workspace::to_rel_path(&graph.mdcroot, &node_path);
            println!(
                "created and added  {}",
                fmt_item(&new_fnode, &q, &rel, false)
            );
        }
        return Ok(0);
    }

    let items: Vec<(&str, &str, &str, bool)> = candidates
        .iter()
        .map(|(f, t, p)| (f.as_str(), t.as_str(), p.as_str(), false))
        .collect();
    let selected = match select_tui("Select dependencies to add", &items)? {
        None => {
            println!("Canceled");
            return Ok(0);
        }
        Some(v) if v.is_empty() => {
            println!("No dependencies selected");
            return Ok(0);
        }
        Some(v) => v,
    };

    let selected_fnodes: Vec<String> = selected.iter().map(|&i| candidates[i].0.clone()).collect();
    let (added, _, _) = graph.add_direct_dependencies(selected_fnodes)?;

    let root_path = graph.root_path()?;
    if let Err(e) = graph.cache.upsert_path(&root_path) {
        eprintln_warn(&format!("index update failed: {e}"));
    }

    println!(
        "added {BOLD}{}{RESET} dep{}",
        added.len(),
        if added.len() == 1 { "" } else { "s" }
    );
    for fnode in &added {
        let label = candidates
            .iter()
            .find(|(f, _, _)| f == fnode)
            .map(|(f, t, p)| fmt_item(f, t, p, false))
            .unwrap_or_else(|| fnode.clone());
        println!("  + {label}");
    }
    Ok(0)
}

// ── cmd: dep rm ───────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_rm(source: String) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let cache = open_cache(mdcroot)?;
    let (mut graph, _) = DepGraph::from_ref(cache, &source, Some(&cwd()))?;
    let source_item = graph.root_item()?;
    let dep_items = graph.direct_dependency_items()?;

    if dep_items.is_empty() {
        println!(
            "source  {}",
            fmt_item(
                &source_item.fnode,
                &source_item.title,
                &source_item.rel_path,
                false
            )
        );
        println!("  No dependencies to remove");
        return Ok(0);
    }

    let items: Vec<(&str, &str, &str, bool)> = dep_items
        .iter()
        .map(|item| {
            let broken = graph.is_broken_fnode(&item.fnode);
            (
                item.fnode.as_str(),
                item.title.as_str(),
                item.rel_path.as_str(),
                broken,
            )
        })
        .collect();

    let selected = match select_tui("Select dependencies to remove", &items)? {
        None => {
            println!("Canceled");
            return Ok(0);
        }
        Some(v) if v.is_empty() => {
            println!("No dependencies selected");
            return Ok(0);
        }
        Some(v) => v,
    };

    let selected_fnodes: Vec<String> = selected
        .iter()
        .map(|&i| dep_items[i].fnode.clone())
        .collect();
    let removed = graph.remove_direct_dependencies(selected_fnodes)?;

    let root_path = graph.root_path()?;
    if let Err(e) = graph.cache.upsert_path(&root_path) {
        eprintln_warn(&format!("index update failed: {e}"));
    }

    println!(
        "removed {BOLD}{}{RESET} dep{}",
        removed.len(),
        if removed.len() == 1 { "" } else { "s" }
    );
    for fnode in &removed {
        let label = dep_items
            .iter()
            .find(|item| &item.fnode == fnode)
            .map(|item| fmt_item(&item.fnode, &item.title, &item.rel_path, false))
            .unwrap_or_else(|| fnode.clone());
        println!("  - {label}");
    }
    Ok(0)
}

// ── cmd: dep refs ─────────────────────────────────────────────────────────────

pub(super) fn cmd_dep_refs(target: String, depth: i32) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache = open_cache(mdcroot.clone())?;
    if let Ok(src_path) = cache.resolve_edit_target_path(&target, Some(&cwd())) {
        let _ = cache.upsert_path(&src_path);
    }
    let (fnode, title, path) = cache.resolve_ref(&target, Some(&cwd()))?;
    let rel_path = crate::workspace::to_rel_path(&mdcroot, &path);
    let target_item = DependencyItem {
        depth: 0,
        fnode: fnode.clone(),
        title,
        rel_path,
    };
    let ref_items = cache.referrer_items(&fnode, depth)?;
    print_dep_report(
        "target",
        &target_item,
        "refers",
        &ref_items,
        &std::collections::HashMap::new(),
    );
    Ok(0)
}

// ── Cycle display helper ──────────────────────────────────────────────────────

fn print_cycles_if_any(cycles: &[Vec<String>], cache: &crate::indcache::IndCache) {
    if cycles.is_empty() {
        return;
    }
    println!("   {RED}cycles ({}):{RESET}", cycles.len());

    for cycle in cycles {
        let fnode_refs: Vec<&str> = cycle.iter().map(|s| s.as_str()).collect();
        let label_map = cache.lookup_by_fnode(&fnode_refs).unwrap_or_default();
        print_cycle(cycle, &label_map);
    }
}

// ── Interactive multi-select (ratatui TUI) ────────────────────────────────────

/// Presents an interactive checkbox list in the alternate screen.
/// Returns `None` on cancel (q/Esc), `Some(sorted_indices)` on Enter.
fn select_tui(prompt: &str, items: &[(&str, &str, &str, bool)]) -> Result<Option<Vec<usize>>> {
    if items.is_empty() {
        return Ok(Some(vec![]));
    }

    enable_raw_mode()?;
    if let Err(e) = execute!(io::stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut cursor = 0usize;
    let mut selected: HashSet<usize> = HashSet::new();
    let mut canceled = false;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(area);

            f.render_widget(
                Paragraph::new(prompt).style(Style::default().add_modifier(Modifier::BOLD)),
                chunks[0],
            );
            f.render_widget(
                Paragraph::new(
                    "↑/↓/j/k: navigate   space: toggle   enter: confirm   q/esc: cancel",
                )
                .style(Style::default().add_modifier(Modifier::DIM)),
                chunks[2],
            );

            let list_items: Vec<ListItem> = items
                .iter()
                .enumerate()
                .map(|(i, (fnode, title, rel_path, broken))| {
                    let checked = selected.contains(&i);
                    let checkbox = if checked { "[x]" } else { "[ ]" };
                    let sf = {
                        let s = fnode.trim_matches(|c| c == '<' || c == '>');
                        &s[..s.len().min(8)]
                    };
                    let title_col = {
                        let chars: Vec<char> = title.chars().collect();
                        if chars.len() <= TITLE_WIDTH {
                            format!("{}{}", title, " ".repeat(TITLE_WIDTH - chars.len()))
                        } else {
                            let t: String = chars[..TITLE_WIDTH - 1].iter().collect();
                            format!("{t}…")
                        }
                    };
                    let cb_style = if checked {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().add_modifier(Modifier::DIM)
                    };
                    let mut spans = vec![
                        Span::styled(format!("{checkbox} "), cb_style),
                        Span::styled(
                            format!("{sf}  "),
                            Style::default().add_modifier(Modifier::DIM),
                        ),
                    ];
                    if *broken {
                        spans.push(Span::styled(
                            "✗ ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                    }
                    spans.push(Span::styled(
                        title_col,
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        rel_path.to_string(),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                    ListItem::new(Line::from(spans))
                })
                .collect();

            let mut list_state = ListState::default();
            list_state.select(Some(cursor));
            f.render_stateful_widget(
                List::new(list_items)
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                chunks[1],
                &mut list_state,
            );
        })?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        canceled = true;
                        break;
                    }
                    KeyCode::Enter => break,
                    KeyCode::Char(' ') => {
                        if selected.contains(&cursor) {
                            selected.remove(&cursor);
                        } else {
                            selected.insert(cursor);
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        cursor = cursor.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if cursor + 1 < items.len() {
                            cursor += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    if canceled {
        return Ok(None);
    }
    let mut result: Vec<usize> = selected.into_iter().collect();
    result.sort_unstable();
    Ok(Some(result))
}
