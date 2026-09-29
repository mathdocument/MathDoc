//! Source references affected by changing a node's qualified name.
use std::ops::Range;

/// Read header tokens without touching comments, strings or proof bodies.
fn header_token(source: &str, pos: &mut usize) -> Option<Range<usize>> {
    loop {
        let rest = &source[*pos..];
        if rest.is_empty() {
            return None;
        }
        if let Some(comment) = rest.strip_prefix("--") {
            *pos += 2 + comment.find('\n').unwrap_or(comment.len());
        } else if rest.starts_with("/-") {
            let mut depth = 1;
            *pos += 2;
            while *pos < source.len() && depth > 0 {
                if source[*pos..].starts_with("/-") {
                    depth += 1;
                    *pos += 2;
                } else if source[*pos..].starts_with("-/") {
                    depth -= 1;
                    *pos += 2;
                } else {
                    *pos += source[*pos..].chars().next()?.len_utf8();
                }
            }
        } else if rest.starts_with(|c: char| c.is_whitespace() || c == '\u{feff}') {
            *pos += rest.chars().next()?.len_utf8();
        } else {
            break;
        }
    }
    let start = *pos;
    while let Some(ch) = source[*pos..].chars().next() {
        if ch == '«' {
            let end = source[*pos..].find('»')?;
            *pos += end + '»'.len_utf8();
        } else if ch.is_alphanumeric() || matches!(ch, '_' | '.' | '\'') {
            *pos += ch.len_utf8();
        } else {
            break;
        }
    }
    if start == *pos {
        *pos += source[*pos..].chars().next()?.len_utf8();
    }
    Some(start..*pos)
}

pub fn rename_imports(source: &str, old: &str, new: &str) -> String {
    let mut pos = 0;
    let mut replacements = vec![];
    while let Some(token) = header_token(source, &mut pos) {
        match &source[token] {
            "module" | "prelude" | "public" | "meta" => (),
            "import" => {
                let Some(mut name) = header_token(source, &mut pos) else {
                    break;
                };
                if &source[name.clone()] == "all" {
                    let Some(next) = header_token(source, &mut pos) else {
                        break;
                    };
                    name = next;
                }
                if crate::store::module_parts(&source[name.clone()])
                    .is_ok_and(|parts| parts.join(".") == old)
                {
                    replacements.push(name);
                }
            }
            _ => break,
        }
    }
    let mut result = source.to_owned();
    for range in replacements.into_iter().rev() {
        result.replace_range(range, new);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_preserves_import_modes_comments_and_body() {
        let source = "/- import A.X /- nested -/ -/\nmodule\npublic meta import A.X\nimport all «A».X\nimport A.XYZ\n-- import A.X\nnamespace A.X\ndef text := \"import A.X\"\n";
        assert_eq!(rename_imports(source, "A.X", "B.Result"),
            "/- import A.X /- nested -/ -/\nmodule\npublic meta import B.Result\nimport all B.Result\nimport A.XYZ\n-- import A.X\nnamespace A.X\ndef text := \"import A.X\"\n");
    }
}
