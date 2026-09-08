use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SrcBlock {
    pub srctype: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct MdocNode {
    pub path: PathBuf,
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub blocks: Vec<SrcBlock>,
}

impl MdocNode {
    /// Create a brand-new node at the given path with a fresh UUID fnode.
    pub fn new_at_path(path: &Path, title: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            fnode: Uuid::new_v4().to_string(),
            title: title.to_string(),
            depens: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// Load a node from an existing .mdoc file (full parse including blocks).
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_bytes(path, &std::fs::read(path)?)
    }

    pub(crate) fn load_bytes(path: &Path, content: &[u8]) -> Result<Self> {
        let parsed = super::codec::parse(path, content)?;
        Ok(Self {
            path: path.to_path_buf(),
            fnode: parsed.fnode,
            title: parsed.title,
            depens: parsed.depens,
            blocks: parsed.blocks,
        })
    }

    pub fn add_dependency(&mut self, dep_fnode: &str) {
        if !self.depens.iter().any(|dependency| dependency == dep_fnode) {
            self.depens.push(dep_fnode.to_string());
        }
    }

    pub fn remove_dependency(&mut self, dep_fnode: &str) {
        self.depens.retain(|dependency| dependency != dep_fnode);
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn source_block(&self, srctype: &str) -> Option<&SrcBlock> {
        self.blocks
            .iter()
            .find(|block| block.srctype.eq_ignore_ascii_case(srctype))
    }

    pub fn upsert_source_block(&mut self, srctype: &str, content: String) -> Result<()> {
        let srctype = crate::config::builtin_srctype(srctype)?;
        let mut content = content;
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        if let Some(block) = self
            .blocks
            .iter_mut()
            .find(|block| block.srctype.eq_ignore_ascii_case(srctype))
        {
            block.content = content;
        } else {
            self.blocks.push(SrcBlock {
                srctype: srctype.to_string(),
                content,
                metadata: HashMap::new(),
            });
        }
        Ok(())
    }

    pub fn remove_source_block(&mut self, srctype: &str) -> bool {
        let Some(index) = self
            .blocks
            .iter()
            .position(|block| block.srctype.eq_ignore_ascii_case(srctype))
        else {
            return false;
        };
        self.blocks.remove(index);
        true
    }

    /// Validate and render this document without performing filesystem I/O.
    pub fn render(&self) -> Result<String> {
        super::codec::render(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_source_block_uses_parser_canonical_trailing_newline() {
        let mut node = MdocNode::new_at_path(Path::new("node.mdoc"), "Node");
        node.upsert_source_block("lean", "#check Nat".to_string())
            .unwrap();
        assert_eq!(node.source_block("lean").unwrap().content, "#check Nat\n");

        node.upsert_source_block("lean", "#check Nat\n\n".to_string())
            .unwrap();
        assert_eq!(node.source_block("lean").unwrap().content, "#check Nat\n\n");
    }
}
