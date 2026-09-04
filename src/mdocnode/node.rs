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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MdocHead {
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub source_types: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MdocIdentity {
    pub fnode: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MdocNode {
    pub path: PathBuf,
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub blocks: Vec<SrcBlock>,
}

impl MdocHead {
    pub(crate) fn load_bytes(path: &Path, content: &[u8]) -> Result<Self> {
        let parsed = super::codec::parse(path, content, false)?;
        Ok(Self {
            fnode: parsed.fnode,
            title: parsed.title,
            depens: parsed.depens,
            source_types: parsed.source_types,
        })
    }

    pub(crate) fn has_source_block(&self, srctype: &str) -> bool {
        self.source_types
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(srctype))
    }
}

impl MdocIdentity {
    /// Recover header identity from malformed content without interpreting block
    /// bodies as document-level structure.
    pub(crate) fn from_bytes(content: &[u8]) -> Self {
        super::codec::identity(content)
    }

    pub(crate) fn complete(&self) -> Option<(&str, &str)> {
        Some((self.fnode.as_deref()?, self.title.as_deref()?))
    }
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
        let snapshot = crate::workspace::FileSnapshot::capture(path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow::anyhow!("node file does not exist: {}", path.display()))?;
        Self::load_bytes(path, content)
    }

    pub(crate) fn load_bytes(path: &Path, content: &[u8]) -> Result<Self> {
        let parsed = super::codec::parse(path, content, true)?;
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
    fn load_head_returns_explicit_structural_type() {
        let path = Path::new("c.mdoc");
        let head = MdocHead::load_bytes(
            path,
            b"@fnode: ff\n@title: C\n\n@src: lean\nbig content\n@end\n",
        )
        .unwrap();
        assert_eq!(head.fnode, "ff");
        assert!(head.depens.is_empty());
        assert_eq!(head.source_types, ["lean"]);
        assert!(head.has_source_block("LEAN"));
    }

    #[test]
    fn fallback_identity_is_case_insensitive_but_block_aware() {
        let identity = MdocIdentity::from_bytes(
            b"@FNODE: real-node\n@TITLE: Real Title\n\
              @src: \"unterminated\n@fnode: fake-src\n@title: Fake Src\n",
        );
        assert_eq!(identity.complete(), Some(("real-node", "Real Title")));

        let identity = MdocIdentity::from_bytes(
            b"@dep:\nnot a dependency\n@fnode: fake-dep\n@title: Fake Dep\n@end\n",
        );
        assert_eq!(identity, MdocIdentity::default());
    }

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
