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

/// Rewrite literal qualified labels in reference commands. Keep comments,
/// verbatim text, local labels, declarations and ordinary prose intact.
pub fn rename_latex_refs(source: &str, old: &str, new: &str) -> String {
    let mut pos = 0;
    let mut replacements = vec![];
    let prefix = format!("{old}::");
    while pos < source.len() {
        let rest = &source[pos..];
        if rest.starts_with('%') {
            pos += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        if !rest.starts_with('\\') {
            pos += rest.chars().next().unwrap().len_utf8();
            continue;
        }
        pos += 1;
        let start = pos;
        while source
            .as_bytes()
            .get(pos)
            .is_some_and(u8::is_ascii_alphabetic)
        {
            pos += 1;
        }
        if start == pos {
            // Escaped %, braces or backslashes are literal characters.
            pos += source[pos..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        let command = &source[start..pos];
        if source[pos..].starts_with('*') {
            pos += 1;
        }
        if command == "verb" {
            if let Some(delimiter) = source[pos..].chars().next() {
                pos += delimiter.len_utf8();
                pos += source[pos..]
                    .find(delimiter)
                    .map_or(source.len() - pos, |n| n + delimiter.len_utf8());
            }
            continue;
        }
        loop {
            while source[pos..].starts_with(char::is_whitespace) {
                pos += source[pos..].chars().next().unwrap().len_utf8();
            }
            if !source[pos..].starts_with('%') {
                break;
            }
            pos += source[pos..].find('\n').unwrap_or(source.len() - pos);
        }
        if !source[pos..].starts_with('{') {
            continue;
        }
        let arg_start = pos + 1;
        let Some(end) = source[arg_start..].find('}').map(|n| arg_start + n) else {
            break;
        };
        let argument = &source[arg_start..end];
        if command == "begin"
            && matches!(argument, "verbatim" | "verbatim*" | "lstlisting" | "minted")
        {
            let close = format!("\\end{{{argument}}}");
            pos = source[end + 1..]
                .find(&close)
                .map_or(source.len(), |n| end + 1 + n + close.len());
        } else if matches!(command, "ref" | "cref" | "Cref" | "nameref" | "eqref") {
            let mut offset = arg_start;
            for key in argument.split(',') {
                let mut trimmed = key.trim_start();
                while trimmed.starts_with('%') {
                    trimmed = trimmed
                        .find('\n')
                        .map_or("", |end| trimmed[end..].trim_start());
                }
                if trimmed.starts_with(&prefix) {
                    let start = offset + key.len() - trimmed.len();
                    replacements.push(start..start + old.len());
                }
                offset += key.len() + 1;
            }
            pos = end + 1;
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
    fn rename_qualified_latex_references_only() {
        let source = r"\cref{ A.X::first,local, A.XYZ::second,A.X::third,% comment
A.X::fourth } \Cref*{A.X::first}
\nameref% comment
{A.X::first} \eqref{A.X::eq} \ref{A.X::first}
% \ref{A.X::comment}
\verb|\ref{A.X::literal}| \begin{verbatim}\ref{A.X::literal}\end{verbatim}
\label{A.X::local} A.X::ordinary \% \ref{A.X::active}";
        let renamed = rename_latex_refs(source, "A.X", "B.Result");
        assert!(renamed.contains(
            r"\cref{ B.Result::first,local, A.XYZ::second,B.Result::third,% comment
B.Result::fourth }"
        ));
        assert!(renamed.contains(r"\Cref*{B.Result::first}"));
        for key in ["first", "eq", "active"] {
            assert!(renamed.contains(&format!("{{B.Result::{key}}}")));
        }
        for key in ["comment", "literal", "local"] {
            assert!(renamed.contains(&format!("{{A.X::{key}}}")));
        }
        assert!(renamed.contains("A.X::ordinary"));
        assert_eq!(rename_latex_refs(&renamed, "B.Result", "A.X"), source);
    }
    #[test]
    fn rename_preserves_import_modes_comments_and_body() {
        let source = "/- import A.X /- nested -/ -/\nmodule\npublic meta import A.X\nimport all «A».X\nimport A.XYZ\n-- import A.X\nnamespace A.X\ndef text := \"import A.X\"\n";
        assert_eq!(rename_imports(source, "A.X", "B.Result"),
            "/- import A.X /- nested -/ -/\nmodule\npublic meta import B.Result\nimport all B.Result\nimport A.XYZ\n-- import A.X\nnamespace A.X\ndef text := \"import A.X\"\n");
    }
}
