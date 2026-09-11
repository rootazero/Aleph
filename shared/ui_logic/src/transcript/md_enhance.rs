//! Text→text markdown pre-pass shared by both renderers. No regex crate: the
//! patterns are fixed machine text (fences, `> [!TYPE]`, `https://`,
//! `name.ext:NN`), so hand scanners are smaller for WASM and easier to test.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmonitionKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AdmonitionKind {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "NOTE" => Some(Self::Note),
            "TIP" => Some(Self::Tip),
            "IMPORTANT" => Some(Self::Important),
            "WARNING" => Some(Self::Warning),
            "CAUTION" => Some(Self::Caution),
            _ => None,
        }
    }
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Tip => "TIP",
            Self::Important => "IMPORTANT",
            Self::Warning => "WARNING",
            Self::Caution => "CAUTION",
        }
    }
    /// The marker the renderers match on the FIRST line of the blockquote.
    #[must_use]
    pub fn marker(self) -> String {
        format!("> **[!{}]**", self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A ```mermaid fence, replaced in `markdown` by a placeholder line the
    /// renderer swaps for its diagram (Panel: sandboxed iframe; TUI: source box).
    Mermaid { index: usize, source: String },
}

/// Placeholder a renderer looks for: `<!--aleph-mermaid:N-->`.
pub const MERMAID_PLACEHOLDER_PREFIX: &str = "<!--aleph-mermaid:";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Enhanced {
    pub markdown: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    pub start: usize,
    pub end: usize,
    pub path: String,
    pub line: u32,
    pub col: Option<u32>,
}

pub const KNOWN_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "swift", "c", "h", "cc", "cpp",
    "hpp", "cs", "rb", "php", "md", "toml", "yaml", "yml", "json", "css", "scss", "html", "sh",
    "ps1", "sql", "proto", "txt", "lock",
];

const URL_STOP: &[char] = &[
    ' ', '\t', '<', '>', '\'', '"', '|', '，', '。', '；', '：', '！', '？', '、', '」', '』',
    '】', '（', '）', '【', '《', '》', '『', '「',
];

/// Strip trailing punctuation, then unbalanced `)` / `]`.
#[must_use]
pub fn trim_url(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let Some(last) = s.chars().last() else { break };
        if ".,;:!?\"'》）}".contains(last) || last == '】' || last == '」' || last == '』' {
            end -= last.len_utf8();
            continue;
        }
        if last == ')' && s.matches(')').count() > s.matches('(').count() {
            end -= 1;
            continue;
        }
        if last == ']' && s.matches(']').count() > s.matches('[').count() {
            end -= 1;
            continue;
        }
        break;
    }
    &url[..end]
}

fn is_inside_inline_code(line: &str, at: usize) -> bool {
    line[..at].matches('`').count() % 2 == 1
}

/// `https://…` not already in `<…>` or `](…)` → `[url](url)`.
#[must_use]
pub fn linkify_bare_urls(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let hit = ["https://", "http://"].iter().any(|p| rest.starts_with(*p));
        if hit {
            let prev = line[..i].chars().last();
            let preceded_by_link = line[..i].ends_with("](") || prev == Some('<');
            if !preceded_by_link && !is_inside_inline_code(line, i) {
                let stop = rest
                    .find(|c: char| URL_STOP.contains(&c))
                    .unwrap_or(rest.len());
                let raw = &rest[..stop];
                let url = trim_url(raw);
                out.push_str(&format!("[{url}]({url})"));
                out.push_str(&raw[url.len()..]);
                i += stop;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `src/a.rs:12` / `src/a.rs:12:5` outside inline code, extension in `KNOWN_EXTENSIONS`.
#[must_use]
pub fn find_path_refs(line: &str) -> Vec<PathRef> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let is_path_char =
        |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'\\' | b'.' | b'_' | b'-' | b'~');
    let mut i = 0;
    while i < b.len() {
        if !is_path_char(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && is_path_char(b[i]) {
            i += 1;
        }
        let token = &line[start..i];
        // token must be `name.ext` and be followed by `:digits`
        let Some(dot) = token.rfind('.') else {
            continue;
        };
        let ext = &token[dot + 1..];
        if !KNOWN_EXTENSIONS.contains(&ext) || i >= b.len() || b[i] != b':' {
            continue;
        }
        let mut j = i + 1;
        let ls = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == ls {
            continue;
        }
        let Ok(line_no) = line[ls..j].parse::<u32>() else {
            continue;
        };
        let mut col = None;
        let mut end = j;
        if j < b.len() && b[j] == b':' {
            let cs = j + 1;
            let mut k = cs;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > cs {
                col = line[cs..k].parse().ok();
                end = k;
            }
        }
        if !is_inside_inline_code(line, start) {
            out.push(PathRef {
                start,
                end,
                path: token.to_string(),
                line: line_no,
                col,
            });
        }
        i = end;
    }
    out
}

fn normalize_multiline_links(md: &str) -> String {
    // `[label\n  more](url)` → `[label more](url)`; fenced code exempt.
    // Invariant: a fence marker line is always emitted as its own line and
    // is never appended to a pending unclosed-link label. So a fence
    // marker line flushes any pending label first (as its own line, since
    // it's stored without its trailing `\n`), then toggles fence state and
    // is emitted unchanged — before the label-continuation logic below
    // ever gets a chance to swallow it.
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    let mut pending: Option<String> = None;
    for line in md.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            if let Some(p) = pending.take() {
                out.push_str(&p);
                out.push('\n');
            }
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if let Some(mut p) = pending.take() {
            if !in_fence && !line.contains(']') && !line.trim().is_empty() {
                p.push(' ');
                p.push_str(line.trim());
                pending = Some(p);
                continue;
            }
            p.push_str(if !in_fence && line.contains("](") {
                line.trim_start()
            } else {
                line
            });
            out.push_str(&p);
            continue;
        }
        if !in_fence && line.contains('[') && !line.contains(']') {
            pending = Some(line.trim_end_matches('\n').to_string());
            continue;
        }
        out.push_str(line);
    }
    if let Some(p) = pending {
        out.push_str(&p);
    }
    out
}

/// The pre-pass. Streaming: only link normalisation. Complete: everything.
#[must_use]
pub fn enhance(markdown: &str, streaming: bool) -> Enhanced {
    let md = normalize_multiline_links(markdown);
    if streaming {
        return Enhanced {
            markdown: md,
            blocks: Vec::new(),
        };
    }
    let mut out = String::with_capacity(md.len());
    let mut blocks = Vec::new();
    let mut lines = md.lines().peekable();
    let mut in_fence: Option<String> = None; // fence info string
    let mut mermaid_buf: Option<String> = None;
    // Did the line just processed end with the admonition block's own
    // `\n\n` paragraph separator? That separator is deliberate formatting,
    // not a per-line artifact of `.lines()` splitting, so the trailing-
    // newline correction below must leave it alone.
    let mut admonition_tail = false;
    while let Some(line) = lines.next() {
        admonition_tail = false;
        let t = line.trim_start();
        if let Some(info) = in_fence.as_ref() {
            if t.starts_with("```") {
                if info == "mermaid" {
                    let idx = blocks.len();
                    blocks.push(Block::Mermaid {
                        index: idx,
                        source: mermaid_buf.take().unwrap_or_default(),
                    });
                    out.push_str(&format!("{MERMAID_PLACEHOLDER_PREFIX}{idx}-->\n"));
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
                in_fence = None;
                continue;
            }
            if info == "mermaid" {
                mermaid_buf.get_or_insert_with(String::new).push_str(line);
                mermaid_buf.as_mut().unwrap().push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        if t.starts_with("```") {
            let info = t
                .trim_start_matches('`')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let info = match info.as_str() {
                "mermaid" | "mmd" => "mermaid".to_string(),
                other => other.to_string(),
            };
            if info != "mermaid" {
                out.push_str(line);
                out.push('\n');
            }
            in_fence = Some(info);
            continue;
        }
        // Admonition opener: `> [!TYPE] rest`
        if let Some(after) = t.strip_prefix('>') {
            let a = after.trim_start();
            if let Some(inner) = a.strip_prefix("[!") {
                if let Some(close) = inner.find(']') {
                    if let Some(kind) = AdmonitionKind::parse(inner[..close].trim()) {
                        let mut body: Vec<String> = Vec::new();
                        let first = inner[close + 1..].trim();
                        if !first.is_empty() {
                            body.push(linkify_bare_urls(first));
                        }
                        while let Some(next) = lines.peek() {
                            let n = next.trim_start();
                            let Some(q) = n.strip_prefix('>') else { break };
                            if q.trim_start().starts_with("[!") {
                                break;
                            }
                            body.push(linkify_bare_urls(q.trim()));
                            lines.next();
                        }
                        while body.last().is_some_and(|s| s.is_empty()) {
                            body.pop();
                        }
                        out.push_str(&kind.marker());
                        if !body.is_empty() {
                            out.push(' ');
                            out.push_str(&body.join(" "));
                        }
                        out.push_str("\n\n");
                        admonition_tail = true;
                        continue;
                    }
                }
            }
        }
        out.push_str(&linkify_bare_urls(line));
        out.push('\n');
    }
    if let Some(buf) = mermaid_buf {
        // unclosed mermaid fence: emit as a plain fence
        out.push_str("```mermaid\n");
        out.push_str(&buf);
        out.push_str("```\n");
    }
    // `.lines()` drops the source's own trailing newline, and every ordinary
    // line gets exactly one `\n` re-added, so pop it back off to mirror a
    // source that had none — except when the tail is an admonition's
    // intentional blank-line separator, which isn't a splitting artifact.
    if !markdown.ends_with('\n') && !admonition_tail {
        out.pop();
    }
    Enhanced {
        markdown: out,
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_admonitions_become_a_marked_blockquote_and_swallow_continuation_lines() {
        let e = enhance("> [!WARNING] 磁盘不足\n> 续行\n\ntext", false);
        assert!(e.markdown.starts_with("> **[!WARNING]** 磁盘不足 续行\n\n"));
        assert!(e.markdown.ends_with("text"));
        // Two adjacent admonitions do not swallow each other.
        let e = enhance("> [!NOTE] a\n> [!tip] b", false);
        assert_eq!(e.markdown, "> **[!NOTE]** a\n\n> **[!TIP]** b\n\n");
        // A plain quote is untouched.
        assert_eq!(enhance("> quote", false).markdown, "> quote");
    }

    #[test]
    fn bare_urls_are_linkified_with_balanced_trimming_and_code_is_exempt() {
        assert_eq!(
            linkify_bare_urls("see https://a.b/c, ok"),
            "see [https://a.b/c](https://a.b/c), ok"
        );
        assert_eq!(
            linkify_bare_urls("wiki https://en.wikipedia.org/wiki/A_(B) x"),
            "wiki [https://en.wikipedia.org/wiki/A_(B)](https://en.wikipedia.org/wiki/A_(B)) x"
        );
        assert_eq!(
            linkify_bare_urls("(https://x.y/z)"),
            "([https://x.y/z](https://x.y/z))"
        );
        assert_eq!(
            linkify_bare_urls("`https://x.y` and [t](https://x.y)"),
            "`https://x.y` and [t](https://x.y)"
        );
        assert_eq!(
            linkify_bare_urls("v6 https://[::1]:8080/x！"),
            "v6 [https://[::1]:8080/x](https://[::1]:8080/x)！"
        );
    }

    #[test]
    fn path_refs_need_a_known_extension_and_a_line_number() {
        let r = find_path_refs("see src/a.rs:12 and lib/b.ts:3:7 but not foo:3 or x.unknownext:9");
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].path.as_str(), r[0].line, r[0].col),
            ("src/a.rs", 12, None)
        );
        assert_eq!(
            (r[1].path.as_str(), r[1].line, r[1].col),
            ("lib/b.ts", 3, Some(7))
        );
        assert!(
            find_path_refs("`src/a.rs:12`").is_empty(),
            "inline code is exempt"
        );
    }

    #[test]
    fn mermaid_fences_become_blocks_with_a_placeholder_and_only_when_complete() {
        let md = "before\n```mermaid\ngraph TD; A-->B;\n```\nafter";
        let e = enhance(md, false);
        assert_eq!(
            e.blocks,
            vec![Block::Mermaid {
                index: 0,
                source: "graph TD; A-->B;\n".into()
            }]
        );
        assert!(e.markdown.contains("<!--aleph-mermaid:0-->"));
        assert!(!e.markdown.contains("graph TD"));
        // While streaming, nothing but link normalisation runs.
        let s = enhance(md, true);
        assert!(s.blocks.is_empty());
        assert!(s.markdown.contains("```mermaid"));
        // An unclosed fence at completion is emitted as a normal fence, not lost.
        let u = enhance("```mermaid\ngraph TD;", false);
        assert!(u.markdown.contains("```mermaid\ngraph TD;"));
        assert!(u.blocks.is_empty());
    }

    #[test]
    fn other_fences_and_urls_inside_them_are_left_alone() {
        let md = "```sh\ncurl https://x.y/z\n```";
        assert_eq!(enhance(md, false).markdown, md);
    }

    #[test]
    fn multiline_link_labels_are_joined_even_while_streaming() {
        let e = enhance("前缀 [\n](https://example.com)", true);
        assert_eq!(e.markdown, "前缀 [](https://example.com)");
    }

    #[test]
    fn admonition_bodies_get_bare_url_autolink_like_ordinary_lines_do() {
        let e = enhance("> [!NOTE] see https://example.com/x", false);
        assert_eq!(
            e.markdown,
            "> **[!NOTE]** see [https://example.com/x](https://example.com/x)\n\n"
        );
    }

    #[test]
    fn an_unclosed_link_bracket_before_a_fence_does_not_fuse_onto_the_fence_marker() {
        let src = "[abc\n```\nx\n```\n";
        // The fence marker line must survive `normalize_multiline_links` as
        // its own line, not fused onto the abandoned `[abc` label.
        assert_eq!(normalize_multiline_links(src), src);
        // And `enhance` must therefore still treat the fence body as fenced
        // (untouched, not run through admonition/URL-linkify handling) and
        // parity must not flip: the real closing fence must not be
        // misread as a second opener.
        let e = enhance(src, false);
        assert_eq!(e.markdown, "[abc\n```\nx\n```\n");
    }
}
