use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{MdocNode, SrcBlock};

#[derive(Debug)]
pub(super) struct ParsedMdoc {
    pub fnode: String,
    pub title: String,
    pub depens: Vec<String>,
    pub blocks: Vec<SrcBlock>,
}

#[derive(Default)]
pub(super) struct ParsedIdentity {
    pub fnode: Option<String>,
    pub title: Option<String>,
}

pub(super) fn parse(path: &Path, content: &[u8], include_blocks: bool) -> Result<ParsedMdoc> {
    let content = std::str::from_utf8(content)
        .with_context(|| format!("reading {} as UTF-8", path.display()))?;
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
                    let last = blocks.last_mut().expect("source block exists");
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
            crate::config::builtin_srctype(&srctype).map_err(|error| {
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

pub(super) fn identity(content: &[u8]) -> ParsedIdentity {
    let Ok(content) = std::str::from_utf8(content) else {
        return ParsedIdentity::default();
    };
    let mut identity = ParsedIdentity::default();

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

pub(super) fn render(node: &MdocNode) -> Result<String> {
    validate_for_render(node)?;

    let mut lines: Vec<String> = vec![
        format!("@fnode: {}", node.fnode),
        format!("@title: {}", node.title),
        String::new(),
    ];

    if !node.depens.is_empty() {
        lines.push("@dep:".to_string());
        lines.extend(node.depens.iter().cloned());
        lines.push("@end".to_string());
        lines.push(String::new());
    }

    for block in &node.blocks {
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

fn validate_for_render(node: &MdocNode) -> Result<()> {
    validate_fnode("@fnode", &node.fnode)?;
    validate_single_line("@title", &node.title)?;
    if node.fnode != node.fnode.trim() || node.title != node.title.trim() {
        bail!("fnode and title must not have leading or trailing whitespace");
    }

    let mut seen_deps: HashSet<&str> = HashSet::new();
    for dep in &node.depens {
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
    for block in &node.blocks {
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
        let (srctype, metadata) = parse_src_header(payload, 0, &node.path)?;
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

fn format_src_header(srctype: &str, metadata: &HashMap<String, String>) -> String {
    if metadata.is_empty() {
        return format!("@src: {srctype}");
    }
    let mut metadata: Vec<_> = metadata.iter().collect();
    metadata.sort_unstable_by_key(|(key, _)| *key);
    let meta_tokens: Vec<String> = metadata
        .into_iter()
        .map(|(key, value)| {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{key}=\"{escaped}\"")
        })
        .collect();
    format!("@src: {srctype} {}", meta_tokens.join(" "))
}

fn shlex_split(input: &str) -> Result<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    while let Some(&character) = chars.peek() {
        match character {
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
                            Some(character) => {
                                current.push('\\');
                                current.push(character);
                            }
                            None => bail!("unterminated escape sequence"),
                        },
                        Some('"') => break,
                        Some(character) => current.push(character),
                    }
                }
            }
            '\'' => {
                chars.next();
                loop {
                    match chars.next() {
                        None => bail!("unterminated single-quoted string"),
                        Some('\'') => break,
                        Some(character) => current.push(character),
                    }
                }
            }
            _ => {
                chars.next();
                current.push(character);
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

    #[test]
    fn parse_rejects_duplicate_dependencies_and_unclosed_blocks() {
        let path = Path::new("invalid.mdoc");
        for (body, expected) in [
            ("@dep:\ndep\ndep\n@end\n", "Duplicate dependency"),
            ("@dep:\ndep\n", "Unclosed block '@dep'"),
            ("@src: text\nbody\n", "Unclosed block '@src'"),
        ] {
            let content = format!("@fnode: parse-node\n@title: Parse Node\n\n{body}");
            let error = parse(path, content.as_bytes(), true).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn parse_rejects_malformed_duplicate_directives() {
        let path = Path::new("invalid.mdoc");
        for (body, expected) in [
            ("@dep:\n@end\n@dep:\n@end\n", "Duplicate '@dep'"),
            ("@dep: trailing text\n@end\n", "Unrecognized line"),
            (
                "@src: text mode=one mode=two\n@end\n",
                "Duplicate src metadata key 'mode'",
            ),
        ] {
            let content = format!("@fnode: parse-node\n@title: Parse Node\n\n{body}");
            let error = parse(path, content.as_bytes(), true).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "unexpected error: {error}"
            );
        }
    }
}
