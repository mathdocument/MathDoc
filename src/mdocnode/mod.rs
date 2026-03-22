mod node;

pub use node::{MdocNode, SrcBlock};

/// Quick header read: returns (fnode, title) without full parse, or None on error.
pub fn read_mdoc_head(path: &std::path::Path) -> Option<(String, String)> {
    node::read_mdoc_head(path)
}
