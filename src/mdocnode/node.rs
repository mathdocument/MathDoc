use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrcBlock {
    pub srctype: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdocNode {
    pub mdcroot: PathBuf,
    pub path: PathBuf,
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub blocks: Vec<SrcBlock>,
}

impl MdocNode {
    /// Create a brand-new node at the given path with a fresh UUID fnode.
    pub fn new_at_path(mdcroot: &Path, path: &Path, title: &str) -> Self {
        MdocNode {
            mdcroot: mdcroot.to_path_buf(),
            path: path.to_path_buf(),
            fnode: Uuid::new_v4().to_string(),
            title: title.to_string(),
            depens: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// Load a node from an existing .mdoc file (full parse including blocks).
    pub fn load(mdcroot: &Path, path: &Path) -> Result<Self> {
        Self::load_inner(mdcroot, path, true)
    }

    /// Load a node from an existing .mdoc file, skipping block content.
    pub fn load_head(mdcroot: &Path, path: &Path) -> Result<Self> {
        Self::load_inner(mdcroot, path, false)
    }

    pub fn add_dependency(&mut self, dep_fnode: &str) {
        if !self.depens.iter().any(|d| d == dep_fnode) {
            self.depens.push(dep_fnode.to_string());
        }
    }

    pub fn remove_dependency(&mut self, dep_fnode: &str) {
        self.depens.retain(|d| d != dep_fnode);
    }

    /// Save node content to file.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dirs for {}", self.path.display()))?;
        }

        let mut lines: Vec<String> = vec![
            format!("@fnode: {}", self.fnode),
            format!("@title: {}", self.title),
            String::new(),
        ];

        if !self.depens.is_empty() {
            lines.push("@dep:".to_string());
            lines.extend(self.depens.iter().cloned());
            lines.push("@end".to_string());
            lines.push(String::new());
        }

        for block in &self.blocks {
            lines.push(format_src_header(&block.srctype, &block.metadata));
            if !block.content.is_empty() {
                lines.extend(block.content.lines().map(str::to_string));
            }
            lines.push("@end".to_string());
            lines.push(String::new());
        }

        let payload = lines.join("\n").trim_end().to_string() + "\n";
        std::fs::write(&self.path, payload.as_bytes())
            .with_context(|| format!("writing {}", self.path.display()))
    }

    fn load_inner(mdcroot: &Path, path: &Path, include_blocks: bool) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

        let mut fnode = String::new();
        let mut title = String::new();
        let mut depens: Vec<String> = Vec::new();
        let mut blocks: Vec<SrcBlock> = Vec::new();
        let mut seen_srctypes: HashSet<String> = HashSet::new();

        #[derive(PartialEq)]
        enum Status {
            None,
            Dep,
            Src,
        }
        let mut status = Status::None;

        for (idx, raw_line) in content.lines().enumerate() {
            let lineno = idx + 1;
            let line = raw_line.trim();

            match status {
                Status::Dep => {
                    if line == "@end" {
                        status = Status::None;
                        continue;
                    }
                    if line.is_empty() {
                        bail!(
                            "line {lineno}: Invalid dependency format in {}: '{line}'",
                            path.display()
                        );
                    }
                    if depens.iter().any(|d| d == line) {
                        bail!(
                            "line {lineno}: Duplicate dependency '{line}' in {}",
                            path.display()
                        );
                    }
                    depens.push(line.to_string());
                    continue;
                }
                Status::Src => {
                    if line == "@end" {
                        status = Status::None;
                        continue;
                    }
                    if include_blocks {
                        let last = blocks.last_mut().unwrap();
                        last.content.push_str(raw_line);
                        last.content.push('\n');
                    }
                    continue;
                }
                Status::None => {}
            }

            if line.is_empty() {
                continue;
            }

            if let Some(rest) = line.strip_prefix("@fnode:") {
                if !fnode.is_empty() {
                    bail!("line {lineno}: Duplicate '@fnode' in {}", path.display());
                }
                let val = rest.trim();
                if val.is_empty() {
                    bail!(
                        "line {lineno}: '@fnode' must be non-empty in {}",
                        path.display()
                    );
                }
                fnode = val.to_string();
                continue;
            }

            if let Some(rest) = line.strip_prefix("@title:") {
                if !title.is_empty() {
                    bail!("line {lineno}: Duplicate '@title' in {}", path.display());
                }
                let val = rest.trim();
                if val.is_empty() {
                    bail!(
                        "line {lineno}: '@title' must be non-empty in {}",
                        path.display()
                    );
                }
                title = val.to_string();
                continue;
            }

            if line.starts_with("@dep:") {
                if !depens.is_empty() {
                    bail!("line {lineno}: Duplicate '@dep' in {}", path.display());
                }
                status = Status::Dep;
                continue;
            }

            if let Some(rest) = line.strip_prefix("@src:") {
                let (srctype, metadata) = parse_src_header(rest.trim(), lineno, path)?;
                if seen_srctypes.contains(&srctype) {
                    bail!(
                        "line {lineno}: Duplicate '@src' srctype '{srctype}' in {}",
                        path.display()
                    );
                }
                seen_srctypes.insert(srctype.clone());
                if include_blocks {
                    blocks.push(SrcBlock {
                        srctype,
                        content: String::new(),
                        metadata,
                    });
                }
                status = Status::Src;
                continue;
            }

            bail!(
                "line {lineno}: Unrecognized line in {}: '{line}'",
                path.display()
            );
        }

        if status != Status::None {
            let tag = match status {
                Status::Dep => "@dep",
                Status::Src => "@src",
                Status::None => unreachable!(),
            };
            bail!("Unclosed block '{tag}' in {}", path.display());
        }
        if fnode.is_empty() {
            bail!("'@fnode' must exist and be non-empty in {}", path.display());
        }
        if title.is_empty() {
            bail!("'@title' must exist and be non-empty in {}", path.display());
        }

        Ok(MdocNode {
            mdcroot: mdcroot.to_path_buf(),
            path: path.to_path_buf(),
            fnode,
            title,
            depens,
            blocks,
        })
    }
}

/// Quick header read: returns (fnode, title) or None on error/missing fields.
/// Case-insensitive matching of @fnode/@title, stops early once both found.
pub fn read_mdoc_head(path: &Path) -> Option<(String, String)> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut fnode = String::new();
    let mut title = String::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("@fnode:") && fnode.is_empty() {
            fnode = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
        } else if lower.starts_with("@title:") && title.is_empty() {
            title = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
        }
        if !fnode.is_empty() && !title.is_empty() {
            break;
        }
    }

    if fnode.is_empty() || title.is_empty() {
        return None;
    }
    Some((fnode, title))
}

/// Parse the content after `@src:` — returns (srctype, metadata).
/// Uses shlex-style tokenization to handle quoted values.
fn parse_src_header(
    payload: &str,
    lineno: usize,
    path: &Path,
) -> Result<(String, HashMap<String, String>)> {
    if payload.is_empty() {
        bail!(
            "line {lineno}: Missing srctype after '@src:' in {}",
            path.display()
        );
    }

    let tokens = shlex_split(payload)
        .with_context(|| format!("line {lineno}: Invalid '@src' header in {}", path.display()))?;

    if tokens.is_empty() {
        bail!("line {lineno}: Invalid '@src' header in {}", path.display());
    }

    let srctype = tokens[0].clone();
    let mut metadata = HashMap::new();

    for token in &tokens[1..] {
        match token.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => {
                metadata.insert(key.trim().to_string(), value.to_string());
            }
            _ => {
                bail!(
                    "line {lineno}: Invalid src metadata token: '{token}' in {}",
                    path.display()
                );
            }
        }
    }

    Ok((srctype, metadata))
}

/// Format a `@src:` header line for saving.
fn format_src_header(srctype: &str, metadata: &HashMap<String, String>) -> String {
    if metadata.is_empty() {
        return format!("@src: {srctype}");
    }
    let meta_tokens: Vec<String> = metadata
        .iter()
        .map(|(k, v)| {
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{k}=\"{escaped}\"")
        })
        .collect();
    format!("@src: {srctype} {}", meta_tokens.join(" "))
}

/// Minimal shlex-like tokenizer: splits on whitespace, respects `"..."` and `'...'` quoting.
fn shlex_split(s: &str) -> Result<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '"' => {
                chars.next();
                loop {
                    match chars.next() {
                        None => bail!("unterminated double-quoted string"),
                        Some('\\') => match chars.next() {
                            Some('\\') => current.push('\\'),
                            Some('"') => current.push('"'),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => bail!("unterminated escape sequence"),
                        },
                        Some('"') => break,
                        Some(c) => current.push(c),
                    }
                }
            }
            '\'' => {
                chars.next();
                loop {
                    match chars.next() {
                        None => bail!("unterminated single-quoted string"),
                        Some('\'') => break,
                        Some(c) => current.push(c),
                    }
                }
            }
            _ => {
                chars.next();
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_mdoc(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn load_minimal() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(&dir, "a.mdoc", "@fnode: abc123\n@title: Test Title\n");
        let node = MdocNode::load(dir.path(), &path).unwrap();
        assert_eq!(node.fnode, "abc123");
        assert_eq!(node.title, "Test Title");
        assert!(node.depens.is_empty());
        assert!(node.blocks.is_empty());
    }

    #[test]
    fn load_with_deps_and_block() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "b.mdoc",
            "@fnode: xyz\n@title: B\n\n@dep:\ndep1\ndep2\n@end\n\n@src: latex\ncontent line\n@end\n",
        );
        let node = MdocNode::load(dir.path(), &path).unwrap();
        assert_eq!(node.depens, vec!["dep1", "dep2"]);
        assert_eq!(node.blocks.len(), 1);
        assert_eq!(node.blocks[0].srctype, "latex");
        assert!(node.blocks[0].content.contains("content line"));
    }

    #[test]
    fn load_head_skips_blocks() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "c.mdoc",
            "@fnode: ff\n@title: C\n\n@src: lean\nbig content\n@end\n",
        );
        let node = MdocNode::load_head(dir.path(), &path).unwrap();
        assert_eq!(node.fnode, "ff");
        assert!(node.blocks.is_empty());
    }

    #[test]
    fn save_and_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.mdoc");
        let mut node = MdocNode::new_at_path(dir.path(), &path, "Save Test");
        node.fnode = "savefnode".to_string();
        node.add_dependency("dep1");
        node.blocks.push(SrcBlock {
            srctype: "latex".to_string(),
            content: "x = 1\n".to_string(),
            metadata: HashMap::new(),
        });
        node.save().unwrap();

        let loaded = MdocNode::load(dir.path(), &path).unwrap();
        assert_eq!(loaded.fnode, "savefnode");
        assert_eq!(loaded.title, "Save Test");
        assert_eq!(loaded.depens, vec!["dep1"]);
        assert_eq!(loaded.blocks[0].srctype, "latex");
        assert!(loaded.blocks[0].content.contains("x = 1"));
    }

    #[test]
    fn save_with_metadata() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meta.mdoc");
        let mut node = MdocNode::new_at_path(dir.path(), &path, "Meta");
        node.fnode = "metafnode".to_string();
        let mut meta = HashMap::new();
        meta.insert("preamble".to_string(), "/path/to file".to_string());
        node.blocks.push(SrcBlock {
            srctype: "latex".to_string(),
            content: String::new(),
            metadata: meta,
        });
        node.save().unwrap();

        let loaded = MdocNode::load(dir.path(), &path).unwrap();
        assert_eq!(
            loaded.blocks[0].metadata.get("preamble").unwrap(),
            "/path/to file"
        );
    }

    #[test]
    fn load_error_missing_fnode() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(&dir, "bad.mdoc", "@title: Only Title\n");
        assert!(MdocNode::load(dir.path(), &path).is_err());
    }

    #[test]
    fn load_error_duplicate_dep() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "dup.mdoc",
            "@fnode: f\n@title: T\n\n@dep:\ndep1\ndep1\n@end\n",
        );
        assert!(MdocNode::load(dir.path(), &path).is_err());
    }

    #[test]
    fn load_error_unclosed_block() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "unclosed.mdoc",
            "@fnode: f\n@title: T\n\n@dep:\ndep1\n",
        );
        assert!(MdocNode::load(dir.path(), &path).is_err());
    }

    #[test]
    fn read_head_returns_none_on_missing_file() {
        let path = Path::new("/nonexistent/path.mdoc");
        assert!(read_mdoc_head(path).is_none());
    }

    #[test]
    fn read_head_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "ci.mdoc",
            "@FNODE: theid\n@TITLE: The Title\n@src: latex\nstuff\n@end\n",
        );
        let (fnode, title) = read_mdoc_head(&path).unwrap();
        assert_eq!(fnode, "theid");
        assert_eq!(title, "The Title");
    }

    #[test]
    fn shlex_basic() {
        let tokens = shlex_split("latex preamble=\"/some path\"").unwrap();
        assert_eq!(tokens, vec!["latex", "preamble=/some path"]);
    }

    #[test]
    fn shlex_escaped_quote() {
        let tokens = shlex_split(r#"lean version="4\"0""#).unwrap();
        assert_eq!(tokens, vec!["lean", "version=4\"0"]);
    }
}
