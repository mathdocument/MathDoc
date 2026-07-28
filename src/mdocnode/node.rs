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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdocHead {
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MdocIdentity {
    pub fnode: Option<String>,
    pub title: Option<String>,
}

struct ParsedMdoc {
    fnode: String,
    title: String,
    depens: Vec<String>,
    blocks: Vec<SrcBlock>,
}

#[derive(Debug)]
pub struct MdocNode {
    pub mdcroot: PathBuf,
    pub path: PathBuf,
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub blocks: Vec<SrcBlock>,
    original: std::sync::Mutex<Option<crate::workspace::FileSnapshot>>,
}

impl MdocHead {
    /// Strictly parse the structural fields of an existing .mdoc file.
    pub fn load(path: &Path) -> Result<Self> {
        let snapshot = crate::workspace::FileSnapshot::capture(path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow::anyhow!("node file does not exist: {}", path.display()))?;
        Self::load_bytes(path, content)
    }

    pub(crate) fn load_bytes(path: &Path, content: &[u8]) -> Result<Self> {
        let content = std::str::from_utf8(content)
            .with_context(|| format!("reading {} as UTF-8", path.display()))?;
        let parsed = MdocNode::parse_content(path, content, false)?;
        Ok(Self {
            fnode: parsed.fnode,
            title: parsed.title,
            depens: parsed.depens,
        })
    }
}

impl MdocIdentity {
    /// Recover header identity from malformed content without interpreting block
    /// bodies as document-level structure.
    pub(crate) fn from_bytes(content: &[u8]) -> Self {
        let Ok(content) = std::str::from_utf8(content) else {
            return Self::default();
        };
        let mut identity = Self::default();

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Status {
            None,
            Dep,
            Src,
        }
        let mut status = Status::None;

        for raw_line in content.lines() {
            let line = raw_line.trim();
            match status {
                Status::Dep | Status::Src => {
                    if line.eq_ignore_ascii_case("@end") {
                        status = Status::None;
                    }
                    continue;
                }
                Status::None => {}
            }

            if line.eq_ignore_ascii_case("@dep:") {
                status = Status::Dep;
                continue;
            }
            if line
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("@src:"))
            {
                status = Status::Src;
                continue;
            }

            let Some((directive, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if identity.fnode.is_none() && directive.eq_ignore_ascii_case("@fnode") {
                identity.fnode = Some(value.to_string());
            } else if identity.title.is_none() && directive.eq_ignore_ascii_case("@title") {
                identity.title = Some(value.to_string());
            }
        }
        identity
    }

    pub(crate) fn complete(&self) -> Option<(&str, &str)> {
        Some((self.fnode.as_deref()?, self.title.as_deref()?))
    }
}

impl Clone for MdocNode {
    fn clone(&self) -> Self {
        let original = self
            .original
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Self {
            mdcroot: self.mdcroot.clone(),
            path: self.path.clone(),
            fnode: self.fnode.clone(),
            title: self.title.clone(),
            depens: self.depens.clone(),
            blocks: self.blocks.clone(),
            original: std::sync::Mutex::new(original),
        }
    }
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
            original: std::sync::Mutex::new(None),
        }
    }

    /// Load a node from an existing .mdoc file (full parse including blocks).
    pub fn load(mdcroot: &Path, path: &Path) -> Result<Self> {
        Self::load_inner(mdcroot, path)
    }

    pub(crate) fn load_bytes(mdcroot: &Path, path: &Path, content: &[u8]) -> Result<Self> {
        let content = std::str::from_utf8(content)
            .with_context(|| format!("reading {} as UTF-8", path.display()))?;
        let parsed = Self::parse_content(path, content, true)?;
        Ok(Self {
            mdcroot: mdcroot.to_path_buf(),
            path: path.to_path_buf(),
            fnode: parsed.fnode,
            title: parsed.title,
            depens: parsed.depens,
            blocks: parsed.blocks,
            original: std::sync::Mutex::new(None),
        })
    }

    pub fn add_dependency(&mut self, dep_fnode: &str) {
        if !self.depens.iter().any(|d| d == dep_fnode) {
            self.depens.push(dep_fnode.to_string());
        }
    }

    pub fn remove_dependency(&mut self, dep_fnode: &str) {
        self.depens.retain(|d| d != dep_fnode);
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

    /// Save node content to file.
    pub fn save(&self) -> Result<()> {
        self.save_inner(false)
    }

    /// Create a new node without replacing any path that appears concurrently.
    pub fn save_new(&self) -> Result<()> {
        self.save_inner(true)
    }

    fn save_inner(&self, create_new: bool) -> Result<()> {
        let payload = self.render_payload()?;
        let path = crate::workspace::resolve_mdoc_path(&self.mdcroot, &self.path)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dirs for {}", path.display()))?;
        }
        // Revalidate after directory creation so replacement never uses the raw,
        // potentially relative path supplied by a caller.
        let path = crate::workspace::resolve_mdoc_path(&self.mdcroot, &path)?;

        let mut original = self
            .original
            .lock()
            .map_err(|_| anyhow::anyhow!("node save state lock is poisoned"))?;
        let expected = if create_new {
            crate::workspace::FileSnapshot::Missing
        } else {
            original
                .clone()
                .unwrap_or(crate::workspace::FileSnapshot::Missing)
        };
        let applied = expected.replace(&path, payload.as_bytes())?;
        *original = Some(applied.into_after());
        Ok(())
    }

    pub(crate) fn render_payload(&self) -> Result<String> {
        self.validate_for_save()?;

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
            let srctype = crate::config::builtin_srctype(&block.srctype)?;
            lines.push(format_src_header(srctype, &block.metadata));
            if !block.content.is_empty() {
                lines.extend(block.content.lines().map(str::to_string));
            }
            lines.push("@end".to_string());
            lines.push(String::new());
        }

        Ok(lines.join("\n").trim_end().to_string() + "\n")
    }

    fn validate_for_save(&self) -> Result<()> {
        validate_fnode("@fnode", &self.fnode)?;
        validate_single_line("@title", &self.title)?;
        if self.fnode != self.fnode.trim() || self.title != self.title.trim() {
            bail!("fnode and title must not have leading or trailing whitespace");
        }

        let mut seen_deps: HashSet<&str> = HashSet::new();
        for dep in &self.depens {
            if dep == "@end" {
                bail!("dependency cannot be the reserved '@end' marker");
            }
            validate_fnode("dependency", dep)?;
            if dep != dep.trim() {
                bail!("dependency must not have leading or trailing whitespace");
            }
            if !seen_deps.insert(dep) {
                bail!("duplicate dependency '{dep}'");
            }
        }

        let mut seen_srctypes: HashSet<&str> = HashSet::new();
        for block in &self.blocks {
            validate_single_line("srctype", &block.srctype)?;
            let canonical_srctype = crate::config::builtin_srctype(&block.srctype)?;
            if !seen_srctypes.insert(canonical_srctype) {
                bail!("duplicate srctype '{}'", block.srctype);
            }
            for (key, value) in &block.metadata {
                validate_single_line("src metadata key", key)?;
                validate_no_controls(&format!("src metadata value for '{key}'"), value)?;
            }

            let header = format_src_header(canonical_srctype, &block.metadata);
            let payload = header.strip_prefix("@src:").unwrap_or_default().trim();
            let (srctype, metadata) = parse_src_header(payload, 0, &self.path)?;
            if srctype != canonical_srctype || metadata != block.metadata {
                bail!(
                    "@src header for '{}' cannot be represented without changing its value",
                    block.srctype
                );
            }

            if block.content.lines().any(|line| line.trim() == "@end") {
                bail!(
                    "source block '{}' cannot contain a line equal to the reserved '@end' marker",
                    block.srctype
                );
            }
        }
        Ok(())
    }

    fn load_inner(mdcroot: &Path, path: &Path) -> Result<Self> {
        let snapshot = crate::workspace::FileSnapshot::capture(path)?;
        let content = snapshot
            .content()
            .ok_or_else(|| anyhow::anyhow!("node file does not exist: {}", path.display()))?;
        let mut node = Self::load_bytes(mdcroot, path, content)?;
        node.original = std::sync::Mutex::new(Some(snapshot));
        Ok(node)
    }

    fn parse_content(path: &Path, content: &str, include_blocks: bool) -> Result<ParsedMdoc> {
        let mut fnode = String::new();
        let mut title = String::new();
        let mut depens: Vec<String> = Vec::new();
        let mut seen_dep_block = false;
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
            if status != Status::Src && raw_line.chars().any(char::is_control) {
                bail!(
                    "line {lineno}: control characters are not allowed in structural fields in {}",
                    path.display()
                );
            }
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
                    validate_fnode("dependency", line).with_context(|| {
                        format!("line {lineno}: invalid dependency in {}", path.display())
                    })?;
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
                validate_fnode("@fnode", val).with_context(|| {
                    format!("line {lineno}: invalid '@fnode' in {}", path.display())
                })?;
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
                validate_single_line("@title", val).with_context(|| {
                    format!("line {lineno}: invalid '@title' in {}", path.display())
                })?;
                title = val.to_string();
                continue;
            }

            if line == "@dep:" {
                if seen_dep_block {
                    bail!("line {lineno}: Duplicate '@dep' in {}", path.display());
                }
                seen_dep_block = true;
                status = Status::Dep;
                continue;
            }

            if let Some(rest) = line.strip_prefix("@src:") {
                let (srctype, metadata) = parse_src_header(rest.trim(), lineno, path)?;
                crate::config::validate_srctype_name(&srctype).map_err(|error| {
                    anyhow::anyhow!(
                        "line {lineno}: invalid srctype in {}: {error}",
                        path.display()
                    )
                })?;
                let srctype_identity = srctype.to_ascii_lowercase();
                if seen_srctypes.contains(&srctype_identity) {
                    bail!(
                        "line {lineno}: Duplicate '@src' srctype '{srctype}' in {}",
                        path.display()
                    );
                }
                seen_srctypes.insert(srctype_identity);
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

        Ok(ParsedMdoc {
            fnode,
            title,
            depens,
            blocks,
        })
    }
}

fn validate_single_line(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must be non-empty");
    }
    if value.contains(['\r', '\n']) {
        bail!("{field} must be a single line");
    }
    validate_no_controls(field, value)
}

fn validate_no_controls(field: &str, value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{field} must not contain control characters");
    }
    Ok(())
}

/// Persisted node identifiers are lowercase ASCII words separated by hyphens.
/// Keeping one grammar makes exact graph keys and case-insensitive user lookup agree.
fn validate_fnode(field: &str, value: &str) -> Result<()> {
    validate_single_line(field, value)?;
    let bytes = value.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !is_word(bytes[0])
        || !is_word(*bytes.last().expect("non-empty value checked above"))
        || !bytes.iter().all(|byte| is_word(*byte) || *byte == b'-')
    {
        bail!("{field} must contain only lowercase ASCII letters, digits, and internal hyphens");
    }
    Ok(())
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

    let srctype = crate::config::canonical_srctype(&tokens[0]).to_string();
    let mut metadata = HashMap::new();

    for token in &tokens[1..] {
        match token.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => {
                let key = key.trim();
                validate_single_line("src metadata key", key)?;
                validate_no_controls(&format!("src metadata value for '{key}'"), value)?;
                if metadata.contains_key(key) {
                    bail!(
                        "line {lineno}: Duplicate src metadata key '{key}' in {}",
                        path.display()
                    );
                }
                metadata.insert(key.to_string(), value.to_string());
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
    let mut metadata: Vec<_> = metadata.iter().collect();
    metadata.sort_unstable_by_key(|(key, _)| *key);
    let meta_tokens: Vec<String> = metadata
        .into_iter()
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
    fn load_head_returns_explicit_structural_type() {
        let dir = TempDir::new().unwrap();
        let path = write_mdoc(
            &dir,
            "c.mdoc",
            "@fnode: ff\n@title: C\n\n@src: lean\nbig content\n@end\n",
        );
        let head = MdocHead::load(&path).unwrap();
        assert_eq!(head.fnode, "ff");
        assert!(head.depens.is_empty());
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
    fn load_rejects_malformed_duplicate_directives() {
        let dir = TempDir::new().unwrap();
        for (name, body, expected) in [
            (
                "empty-duplicate-dep.mdoc",
                "@dep:\n@end\n@dep:\n@end\n",
                "Duplicate '@dep'",
            ),
            (
                "trailing-dep.mdoc",
                "@dep: trailing text\n@end\n",
                "Unrecognized line",
            ),
            (
                "duplicate-metadata.mdoc",
                "@src: text mode=one mode=two\n@end\n",
                "Duplicate src metadata key 'mode'",
            ),
        ] {
            let content = format!("@fnode: parse-node\n@title: Parse Node\n\n{body}");
            let path = write_mdoc(&dir, name, &content);
            let error = MdocNode::load(dir.path(), &path).unwrap_err().to_string();
            assert!(error.contains(expected), "unexpected error: {error}");
        }
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
    fn load_head_errors_on_missing_file() {
        let path = Path::new("/nonexistent/path.mdoc");
        assert!(MdocHead::load(path).is_err());
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
