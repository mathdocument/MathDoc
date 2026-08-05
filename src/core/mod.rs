mod algorithms;
mod models;

pub use algorithms::{
    all_topo_depths, representative_cycles, strongly_connected_components, weak_component_sizes,
};
pub use models::{
    DependencyCandidates, DependencyCandidatesEmpty, DependencyItem, DependencyTraversalReport,
    FormalCodeStatus, FormalizationStatus, GraphCheckReport, GraphIssue, GraphRootItem, IssueKind,
    NodeDegrees, NodeSummary,
};

/// Return at most eight Unicode scalar values from an fnode for display.
pub fn short_fnode(fnode: &str) -> &str {
    let value = fnode.trim_matches(|c| c == '<' || c == '>');
    value
        .char_indices()
        .nth(8)
        .map_or(value, |(byte_index, _)| &value[..byte_index])
}

/// Make untrusted text inert before writing it to a terminal.
pub fn escape_terminal(value: &str) -> String {
    use std::fmt::Write;

    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            write!(escaped, "\\u{{{:x}}}", character as u32).expect("writing to a string");
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{escape_terminal, short_fnode};

    #[test]
    fn short_fnode_is_unicode_safe() {
        assert_eq!(short_fnode("abcdefghijk"), "abcdefgh");
        assert_eq!(short_fnode("数学节点编号很长啊"), "数学节点编号很长");
        assert_eq!(short_fnode("<无效节点>"), "无效节点");
    }

    #[test]
    fn terminal_controls_are_escaped() {
        assert_eq!(
            escape_terminal("bad\u{1b}]0;title\u{7}"),
            "bad\\u{1b}]0;title\\u{7}"
        );
    }
}
