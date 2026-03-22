use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::Config;
use crate::mdocnode::MdocNode;

use super::DepGraph;

// ── Comment prefix per srctype ───────────────────────────────────────────────

fn comment_prefix(srctype: &str) -> &'static str {
    match srctype {
        "latex" => "%",
        "lean" => "--",
        "rocq" => "(*",
        _ => "#",
    }
}

fn marker_line(srctype: &str, tag: &str) -> String {
    let prefix = comment_prefix(srctype);
    if prefix == "(*" {
        format!("(* mdc: {} *)", tag)
    } else {
        format!("{} mdc: {}", prefix, tag)
    }
}

// ── merge_work_files ─────────────────────────────────────────────────────────

/// Generate work file content for each srctype present in the dependency subgraph.
/// Returns a map of srctype → assembled file content.
///
/// Node ordering follows `depens`/`reverse_depens` from config, matching `eval_blocks`.
pub fn merge_work_files(
    graph: &mut DepGraph,
    depth: i32,
    config: &Config,
) -> Result<HashMap<String, String>> {
    let nodes = graph.ordered_nodes(depth)?;
    if nodes.is_empty() {
        return Ok(HashMap::new());
    }

    let root_fnode = graph.state.root_fnode.clone();

    // Collect all srctypes that appear in any node's blocks (even empty content).
    let mut srctypes: HashSet<String> = HashSet::new();
    for node in &nodes {
        for block in &node.blocks {
            srctypes.insert(block.srctype.clone());
        }
    }

    let mut result: HashMap<String, String> = HashMap::new();

    for srctype in &srctypes {
        let src_cfg = config.src_config(srctype);

        // Split into root and dep nodes, same as eval_blocks.
        let dep_nodes: Vec<&MdocNode> = nodes.iter().filter(|n| n.fnode != root_fnode).collect();
        let root_node = match nodes.iter().find(|n| n.fnode == root_fnode) {
            Some(n) => n,
            None => continue,
        };

        // Determine node order based on reverse_depens.
        // reverse_depens=true: root first, then deps nearest→deepest (reversed topo).
        // reverse_depens=false: deps deepest→nearest (topo order), then root last.
        let ordered: Vec<&MdocNode> = if src_cfg.effective_depens(srctype) && !dep_nodes.is_empty()
        {
            if src_cfg.effective_reverse_depens() {
                let mut v = vec![root_node];
                v.extend(dep_nodes.iter().rev().copied());
                v
            } else {
                let mut v: Vec<&MdocNode> = dep_nodes;
                v.push(root_node);
                v
            }
        } else {
            // depens=false: only root node.
            vec![root_node]
        };

        let mut out = String::new();

        // Preamble
        let preamble = crate::config::read_preamble(&graph.mdcroot, srctype);
        if !preamble.is_empty() {
            out.push_str(&marker_line(srctype, "preamble"));
            out.push('\n');
            out.push_str(preamble.trim_end_matches('\n'));
            out.push('\n');
            out.push_str(&marker_line(srctype, "end"));
            out.push_str("\n\n");
        }

        // Node sections
        for (i, node) in ordered.iter().enumerate() {
            let short = &node.fnode[..node.fnode.len().min(8)];
            out.push_str(&marker_line(srctype, &format!("fnode: {}", short)));
            out.push('\n');
            out.push_str(&marker_line(srctype, &format!("title: {}", node.title)));
            out.push('\n');

            // Find the block content for this srctype in this node.
            let content: String = node
                .blocks
                .iter()
                .filter(|b| b.srctype == *srctype)
                .map(|b| b.content.trim_end_matches('\n'))
                .collect::<Vec<_>>()
                .join("\n");
            if !content.is_empty() {
                out.push_str(&content);
                out.push('\n');
            }

            out.push_str(&marker_line(srctype, "end"));
            if i < ordered.len() - 1 {
                out.push_str("\n\n");
            } else {
                out.push('\n');
            }
        }

        // Postamble
        let postamble = crate::config::read_postamble(&graph.mdcroot, srctype);
        if !postamble.is_empty() {
            out.push('\n');
            out.push_str(&marker_line(srctype, "postamble"));
            out.push('\n');
            out.push_str(postamble.trim_end_matches('\n'));
            out.push('\n');
            out.push_str(&marker_line(srctype, "end"));
            out.push('\n');
        }

        result.insert(srctype.clone(), out);
    }

    Ok(result)
}

// ── extract_work_file ────────────────────────────────────────────────────────

pub struct ExtractedContent {
    /// (fnode_prefix, content) pairs in file order.
    pub nodes: Vec<(String, String)>,
    /// Preamble content (if present in the work file).
    pub preamble: Option<String>,
    /// Postamble content (if present in the work file).
    pub postamble: Option<String>,
    /// Lines of content found outside any marker block.
    pub warnings: Vec<String>,
}

/// Parse a work file and extract the content of each fnode-marked section.
/// Preamble/postamble sections are recognized and skipped (not returned).
pub fn extract_work_file(path: &Path, srctype: &str) -> Result<ExtractedContent> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;

    let prefix = comment_prefix(srctype);
    let rocq = prefix == "(*";

    let mut nodes: Vec<(String, String)> = Vec::new();
    let mut preamble: Option<String> = None;
    let mut postamble: Option<String> = None;
    let mut warnings: Vec<String> = Vec::new();

    #[derive(PartialEq)]
    enum State {
        Outside,
        InPreamble,
        InPostamble,
        InFnode(String), // fnode prefix
    }

    let mut state = State::Outside;
    let mut current_content = String::new();
    let mut title_seen = false; // only one title: allowed per fnode block

    for line in text.lines() {
        let tag = parse_marker_tag(line, prefix, rocq);

        match tag {
            Some(ref t) if t == "preamble" || t == "postamble" || t.starts_with("fnode: ") => {
                // Opening a new block — must not already be inside one.
                if state != State::Outside {
                    let ctx = match &state {
                        State::InFnode(f) => format!("fnode {f}"),
                        State::InPreamble => "preamble".to_string(),
                        State::InPostamble => "postamble".to_string(),
                        State::Outside => unreachable!(),
                    };
                    warnings.push(format!("new block opened while {ctx} is still unclosed"));
                }
                if t == "preamble" {
                    state = State::InPreamble;
                    current_content.clear();
                } else if t == "postamble" {
                    state = State::InPostamble;
                    current_content.clear();
                } else {
                    let fnode = t.trim_start_matches("fnode: ").to_string();
                    state = State::InFnode(fnode);
                    current_content.clear();
                    title_seen = false;
                }
                continue;
            }
            Some(ref t) if t == "end" => {
                match state {
                    State::InFnode(ref fnode) => {
                        // Trim trailing newline from content.
                        let content = current_content.trim_end_matches('\n').to_string();
                        nodes.push((fnode.clone(), content));
                        current_content.clear();
                    }
                    State::InPreamble => {
                        preamble = Some(current_content.trim_end_matches('\n').to_string());
                        current_content.clear();
                    }
                    State::InPostamble => {
                        postamble = Some(current_content.trim_end_matches('\n').to_string());
                        current_content.clear();
                    }
                    State::Outside => {
                        warnings.push("stray end marker outside any block".to_string());
                    }
                }
                state = State::Outside;
                continue;
            }
            Some(ref t) if t.starts_with("title: ") => {
                // Exactly one title: is expected right after fnode:.
                if let State::InFnode(_) = state {
                    if !title_seen {
                        title_seen = true;
                        continue;
                    }
                    warnings.push(format!("duplicate title marker in fnode block: {}", t));
                    continue;
                }
                warnings.push(format!("title marker outside fnode block: {}", t));
                continue;
            }
            _ => {}
        }

        // Regular content line.
        match state {
            State::InFnode(_) => {
                current_content.push_str(line);
                current_content.push('\n');
            }
            State::InPreamble | State::InPostamble => {
                current_content.push_str(line);
                current_content.push('\n');
            }
            State::Outside => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    warnings.push(format!("content outside markers: {}", trimmed));
                }
            }
        }
    }

    // Handle unclosed block at EOF.
    match &state {
        State::InFnode(fnode) => {
            warnings.push(format!("unclosed block for fnode {}", fnode));
            let content = current_content.trim_end_matches('\n').to_string();
            nodes.push((fnode.clone(), content));
        }
        State::InPreamble => {
            warnings.push("unclosed preamble at end of file".to_string());
        }
        State::InPostamble => {
            warnings.push("unclosed postamble at end of file".to_string());
        }
        State::Outside => {}
    }

    Ok(ExtractedContent {
        nodes,
        preamble,
        postamble,
        warnings,
    })
}

/// Try to parse a marker tag from a line.
/// Returns the tag content (e.g., "fnode: aaaaaaaa", "end", "preamble") or None.
fn parse_marker_tag(line: &str, prefix: &str, rocq: bool) -> Option<String> {
    if rocq {
        // (* mdc: TAG *)
        let trimmed = line.trim();
        let inner = trimmed.strip_prefix("(* mdc: ")?.strip_suffix(" *)")?;
        Some(inner.to_string())
    } else {
        // {prefix} mdc: TAG
        let trimmed = line.trim();
        let after_prefix = trimmed.strip_prefix(prefix)?.trim_start();
        let tag = after_prefix.strip_prefix("mdc: ")?;
        Some(tag.to_string())
    }
}
