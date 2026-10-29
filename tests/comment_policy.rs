use std::fs;
use std::path::{Path, PathBuf};

const MARKERS: [&str; 5] = ["TODO", "FIXME", "HACK", "PERF", "NOTE"];
const DOC_CONSUMING_DERIVES: [&str; 5] =
    ["JsonSchema", "Parser", "Subcommand", "Args", "ValueEnum"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentKind {
    Plain,
    Outer,
    Inner,
}

struct Comment {
    start: usize,
    end: usize,
    line: usize,
    own_line: bool,
    kind: CommentKind,
    text: String,
}

struct Lexed {
    comments: Vec<Comment>,
    code: Vec<u8>,
}

fn rust_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("build.rs")];
    for dir in ["src", "tests"] {
        collect_rust_files(&root.join(dir), &mut files);
    }
    files.sort();
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory is readable") {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn blank_out(code: &mut [u8], start: usize, end: usize) {
    for byte in &mut code[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn raw_string_prefix_len(src: &[u8], at: usize) -> Option<(usize, usize)> {
    if at > 0 && is_ident_byte(src[at - 1]) {
        return None;
    }
    let mut i = at;
    if matches!(src.get(i), Some(b'b' | b'c')) {
        i += 1;
    }
    if src.get(i) != Some(&b'r') {
        return None;
    }
    i += 1;
    let hashes = src[i..].iter().take_while(|&&b| b == b'#').count();
    (src.get(i + hashes) == Some(&b'"')).then_some((i + hashes + 1 - at, hashes))
}

fn char_literal_len(src: &[u8], at: usize) -> Option<usize> {
    let rest = &src[at + 1..];
    if rest.first() == Some(&b'\\') {
        let close = rest.iter().skip(2).position(|&b| b == b'\'')?;
        return (close < 12).then_some(close + 4);
    }
    let first = *rest.first()?;
    let width = match first {
        b'\'' | b'\n' => return None,
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (rest.get(width) == Some(&b'\'')).then_some(width + 2)
}

fn comment_kind(text: &str) -> CommentKind {
    if (text.starts_with("///") && !text.starts_with("////"))
        || (text.starts_with("/**") && !text.starts_with("/***") && text != "/**/")
    {
        CommentKind::Outer
    } else if text.starts_with("//!") || text.starts_with("/*!") {
        CommentKind::Inner
    } else {
        CommentKind::Plain
    }
}

fn lex(src: &str) -> Lexed {
    let bytes = src.as_bytes();
    let mut code = bytes.to_vec();
    let mut comments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.starts_with(b"//") || rest.starts_with(b"/*") {
            let end = if rest.starts_with(b"//") {
                bytes[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| i + n)
            } else {
                let mut depth = 0;
                let mut j = i;
                loop {
                    if bytes[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if bytes[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break j;
                        }
                    } else {
                        j += 1;
                    }
                }
            };
            let line_start = src[..i].rfind('\n').map_or(0, |n| n + 1);
            let text = src[i..end].to_string();
            comments.push(Comment {
                start: i,
                end,
                line: src[..i].matches('\n').count() + 1,
                own_line: src[line_start..i].trim().is_empty(),
                kind: comment_kind(&text),
                text,
            });
            blank_out(&mut code, i, end);
            i = end;
        } else if let Some((prefix, hashes)) = raw_string_prefix_len(bytes, i) {
            let terminator = [b"\"".as_slice(), &vec![b'#'; hashes]].concat();
            let body = i + prefix;
            let end = bytes[body..]
                .windows(terminator.len())
                .position(|w| w == terminator)
                .map_or(bytes.len(), |n| body + n + terminator.len());
            blank_out(&mut code, i, end);
            i = end;
        } else if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            blank_out(&mut code, i, j + 1);
            i = j + 1;
        } else if bytes[i] == b'\'' {
            match char_literal_len(bytes, i) {
                Some(len) => {
                    blank_out(&mut code, i, i + len);
                    i += len;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    Lexed { comments, code }
}

fn matching_close(code: &[u8], open_at: usize) -> usize {
    let (open, close) = match code[open_at] {
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        _ => (b'{', b'}'),
    };
    let mut depth = 0;
    for (offset, &byte) in code[open_at..].iter().enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return open_at + offset;
            }
        }
    }
    code.len()
}

fn skip_whitespace(code: &[u8], mut at: usize) -> usize {
    while at < code.len() && code[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn skip_visibility(code: &[u8], at: usize) -> usize {
    let Some(rest) = code[at..].strip_prefix(b"pub") else {
        return at;
    };
    let restriction = skip_whitespace(code, code.len() - rest.len());
    if code.get(restriction) == Some(&b'(') {
        matching_close(code, restriction) + 1
    } else {
        at
    }
}

fn consumes_docs(attribute: &str) -> bool {
    let attribute: String = attribute.split_whitespace().collect();
    attribute.starts_with("#[derive(")
        && attribute
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| DOC_CONSUMING_DERIVES.contains(&word))
}

fn attributes_consume_docs(code: &[u8], at: usize) -> (bool, usize) {
    let mut at = skip_whitespace(code, at);
    let mut consumes = false;
    while code[at..].starts_with(b"#[") {
        let close = matching_close(code, at + 1);
        consumes |= consumes_docs(&String::from_utf8_lossy(&code[at..=close]));
        at = skip_whitespace(code, close + 1);
    }
    (consumes, at)
}

fn doc_consuming_items(code: &[u8]) -> Vec<(usize, usize)> {
    let mut items = Vec::new();
    let mut at = 0;
    while let Some(offset) = code[at..].windows(2).position(|w| w == b"#[") {
        let start = at + offset;
        let (consumes, mut body) = attributes_consume_docs(code, start);
        at = body.max(start + 2);
        if !consumes {
            continue;
        }
        body = skip_visibility(code, body);
        while body < code.len() && !matches!(code[body], b'{' | b'(' | b';') {
            body += 1;
        }
        if body < code.len() && code[body] != b';' {
            items.push((start, matching_close(code, body)));
        }
    }
    items
}

fn starts_with_marker(text: &str) -> bool {
    let body = text
        .trim_start_matches('/')
        .trim_start_matches(['*', '!'])
        .trim_start();
    MARKERS.iter().any(|marker| {
        body.strip_prefix(marker)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

fn groups(comments: &[Comment]) -> Vec<&[Comment]> {
    let mut groups = Vec::new();
    let mut first = 0;
    for i in 1..=comments.len() {
        let continues = i < comments.len() && {
            let (prev, next) = (&comments[i - 1], &comments[i]);
            prev.own_line
                && next.own_line
                && prev.kind == next.kind
                && next.text.starts_with("//")
                && prev.text.starts_with("//")
                && next.line == prev.line + 1
        };
        if !continues {
            groups.push(&comments[first..i]);
            first = i;
        }
    }
    groups
}

fn violations(src: &str) -> Vec<(usize, String)> {
    let Lexed { comments, code } = lex(src);
    let items = doc_consuming_items(&code);
    let mut found = Vec::new();
    for group in groups(&comments) {
        let rejected: Vec<&Comment> = match group[0].kind {
            CommentKind::Inner => group.iter().collect(),
            CommentKind::Outer => {
                let inside = items
                    .iter()
                    .any(|&(s, e)| s <= group[0].start && group[0].start <= e);
                let attached = attributes_consume_docs(&code, group[group.len() - 1].end).0;
                if inside || attached {
                    Vec::new()
                } else {
                    group.iter().collect()
                }
            }
            CommentKind::Plain => group
                .iter()
                .take_while(|comment| !starts_with_marker(&comment.text))
                .collect(),
        };
        found.extend(rejected.into_iter().map(|c| (c.line, c.text.clone())));
    }
    found
}

#[test]
fn rust_sources_carry_only_allowed_comments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let report: Vec<String> = rust_sources()
        .iter()
        .flat_map(|path| {
            let src = fs::read_to_string(path).expect("source file is UTF-8");
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
            violations(&src)
                .into_iter()
                .map(move |(line, text)| format!("{relative}:{line}: {text}"))
        })
        .collect();
    assert!(
        report.is_empty(),
        "comments outside TODO/FIXME/HACK/PERF/NOTE markers and doc comments read by clap or schemars \
         (see AGENTS.md § Comments and Self-Documenting Code):\n{}",
        report.join("\n")
    );
}

#[test]
fn plain_comments_are_rejected() {
    let src = "fn f() {\n    // explains the code\n    let x = 1; // trailing\n}\n";
    assert_eq!(
        violations(src),
        vec![
            (2, "// explains the code".into()),
            (3, "// trailing".into())
        ]
    );
}

#[test]
fn marker_blocks_are_kept_with_their_continuation_lines() {
    let src = "// NOTE: sing-box exits 0 on an occupied port,\n// so a conflict only surfaces on run.\nfn f() {}\n// TODO(x) later\n";
    assert!(violations(src).is_empty());
}

#[test]
fn lines_before_a_marker_in_the_same_block_are_rejected() {
    let src = "// context\n// FIXME: broken\nfn f() {}\n";
    assert_eq!(violations(src), vec![(1, "// context".into())]);
}

#[test]
fn marker_must_be_a_whole_word() {
    assert_eq!(violations("// NOTEWORTHY\n").len(), 1);
}

#[test]
fn doc_comments_survive_only_where_clap_or_schemars_read_them() {
    let src = "/// Kept.\n#[derive(Debug, JsonSchema)]\n#[serde(tag = \"type\")]\nenum A {\n    /// Kept too.\n    B,\n}\n\n/// Rejected.\n#[derive(Debug)]\nstruct C {\n    /// Rejected too.\n    d: u8,\n}\n";
    assert_eq!(
        violations(src),
        vec![
            (9, "/// Rejected.".into()),
            (12, "/// Rejected too.".into())
        ]
    );
}

#[test]
fn restricted_visibility_does_not_hide_the_item_body() {
    let src = "#[derive(Debug, Subcommand)]\npub(super) enum A {\n    /// Kept.\n    B,\n}\n\n#[derive(JsonSchema)]\npub(in crate::x) struct C {\n    /// Kept too.\n    d: u8,\n}\n";
    assert!(violations(src).is_empty());
}

#[test]
fn module_docs_are_rejected() {
    assert_eq!(violations("//! Module.\nfn f() {}\n").len(), 1);
}

#[test]
fn comment_markers_inside_literals_are_ignored() {
    let src = "const A: &str = \"http://x\";\nconst B: &str = r#\"// not \"a\" comment\"#;\nconst C: char = '/';\nconst D: char = '\\'';\nconst E: &str = \"//\";\nfn f<'a>(s: &'a str) -> &'a str { s }\n";
    assert!(violations(src).is_empty());
}
