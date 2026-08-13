//! Skill content preprocessing — template-variable substitution and opt-in
//! inline-shell expansion applied when a skill's instructions are loaded
//! (Level-2 progressive disclosure, via `skill_read`).
//!
//! This maps hermes-agent's `agent/skill_preprocessing.py` onto Aleph's Rust
//! core. Two capabilities the reference provides that the raw `fs::read_to_string`
//! path lacked:
//!
//! 1. **Template variables** — `${ALEPH_SKILL_DIR}` resolves to the skill's own
//!    directory, so instructions can point at bundled scripts/resources.
//!    An unknown token is left literal — matching the reference, which leaves
//!    it in place rather than erroring.
//! 2. **Inline shell** — `` !`cmd` `` snippets are executed with the skill
//!    directory as the working directory and replaced by the command's stdout,
//!    so a skill can embed live context (e.g. `` !`git rev-parse HEAD` ``).
//!
//! ## Differences from the reference (Rust advantages)
//!
//! - The common case (no template token, no opt-in) is allocation-free: the
//!   input is returned borrowed and untouched, so every existing skill renders
//!   byte-for-byte identically.
//! - Inline-shell snippets run **concurrently** via `futures::join_all` rather
//!   than the reference's sequential loop — N snippets cost ~1 snippet of
//!   wall-clock instead of N.
//! - Inline shell is gated behind an explicit per-skill frontmatter opt-in
//!   (`allow-inline-shell: true`); without it the shell path is never entered.
//!   This keeps arbitrary command execution off by default (P7 defensive
//!   design) while still giving skill authors the capability when they ask.

use crate::utils::no_window::NoWindow;
use std::borrow::Cow;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// Template token resolving to the skill's own directory.
// rust-doctor-disable-next-line hardcoded-secrets
// Not a secret: this is the literal placeholder name used in skill templates.
const TOKEN_SKILL_DIR: &str = "${ALEPH_SKILL_DIR}";
/// Default per-snippet wall-clock budget for inline-shell expansion.
const DEFAULT_INLINE_SHELL_TIMEOUT: Duration = Duration::from_secs(10);
/// Default cap on captured stdout per snippet, in bytes. Mirrors the
/// reference's 4000-char ceiling; oversized output is truncated on a char
/// boundary with a marker.
const DEFAULT_INLINE_SHELL_MAX_OUTPUT: usize = 4000;

/// Context for preprocessing a single skill file.
#[derive(Debug, Clone)]
pub struct SkillPreprocessContext {
    /// Absolute path to the skill's directory (the parent of `SKILL.md`).
    pub skill_dir: PathBuf,
    /// Per-snippet timeout for inline-shell expansion.
    pub timeout: Duration,
    /// Per-snippet stdout cap, in bytes.
    pub max_output: usize,
}

impl SkillPreprocessContext {
    /// Build a context for a skill rooted at `skill_dir`, with inline-shell
    /// limits at their defaults.
    pub fn new(skill_dir: impl Into<PathBuf>) -> Self {
        Self {
            skill_dir: skill_dir.into(),
            timeout: DEFAULT_INLINE_SHELL_TIMEOUT,
            max_output: DEFAULT_INLINE_SHELL_MAX_OUTPUT,
        }
    }
}

/// Preprocess a skill file's content.
///
/// Template variables are always expanded (a no-op for content that contains
/// none). Inline-shell snippets are expanded only when the skill's frontmatter
/// opts in with `allow-inline-shell: true`; otherwise the content is returned
/// after template expansion alone.
pub async fn preprocess_skill_content(content: &str, ctx: &SkillPreprocessContext) -> String {
    // Template vars first, so `${ALEPH_SKILL_DIR}` is usable inside a snippet.
    let expanded = expand_template_vars(content, ctx);

    // Inline shell is opt-in per skill, checked against the *original* content's
    // frontmatter (template expansion never touches the frontmatter keys).
    if frontmatter_allows_inline_shell(content) {
        expand_inline_shell(&expanded, ctx).await
    } else {
        expanded.into_owned()
    }
}

/// Replace `${ALEPH_SKILL_DIR}` tokens.
///
/// Returns the input borrowed and unchanged when no resolvable token is
/// present, so the overwhelmingly common case allocates nothing. Unknown
/// `${ALEPH_SESSION_ID}` tokens are left literal — the wire that would feed
/// them has been severed (the sole production caller never sets one), and the
/// `retain` matches the upstream hermes-agent reference.
#[must_use]
pub fn expand_template_vars<'a>(content: &'a str, ctx: &SkillPreprocessContext) -> Cow<'a, str> {
    if !content.contains(TOKEN_SKILL_DIR) {
        return Cow::Borrowed(content);
    }

    let mut out = content.to_string();
    out = out.replace(TOKEN_SKILL_DIR, &ctx.skill_dir.to_string_lossy());
    Cow::Owned(out)
}

/// Whether the skill's YAML frontmatter sets `allow-inline-shell: true`.
///
/// Cheap and self-contained: reuses the manifest frontmatter splitter and
/// parses a single optional boolean. Any parse failure (no frontmatter, bad
/// YAML) is treated as "not allowed".
#[must_use]
pub fn frontmatter_allows_inline_shell(content: &str) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Probe {
        #[serde(default)]
        allow_inline_shell: bool,
    }

    match crate::skill::manifest::split_frontmatter(content) {
        Ok((yaml, _body)) => {
            serde_yaml::from_str::<Probe>(&yaml).is_ok_and(|p| p.allow_inline_shell)
        }
        Err(_) => false,
    }
}

/// A located `` !`cmd` `` span in the source string, with byte offsets.
struct InlineSpan {
    /// Offset of the leading `!`.
    full_start: usize,
    /// Offset just past the closing backtick.
    full_end: usize,
    /// Offset of the first byte of the command.
    cmd_start: usize,
    /// Offset just past the last byte of the command.
    cmd_end: usize,
}

/// Locate every `` !`cmd` `` snippet. The `!` and both backticks are ASCII, so
/// all recorded offsets fall on char boundaries even when the command body is
/// non-ASCII. An unterminated snippet (no closing backtick) ends the scan and
/// is left literal.
fn find_inline_spans(content: &str) -> Vec<InlineSpan> {
    let bytes = content.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'!' && bytes[i + 1] == b'`' {
            let cmd_start = i + 2;
            match content[cmd_start..].find('`') {
                Some(rel) => {
                    let cmd_end = cmd_start + rel;
                    let full_end = cmd_end + 1;
                    spans.push(InlineSpan {
                        full_start: i,
                        full_end,
                        cmd_start,
                        cmd_end,
                    });
                    i = full_end;
                }
                None => break,
            }
        } else {
            i += 1;
        }
    }
    spans
}

/// Execute every inline-shell snippet concurrently and splice the results back
/// into the source. Snippet failures become non-fatal `[inline-shell error: …]`
/// markers so a broken command never aborts skill loading.
async fn expand_inline_shell(content: &str, ctx: &SkillPreprocessContext) -> String {
    let spans = find_inline_spans(content);
    if spans.is_empty() {
        return content.to_string();
    }

    let results = futures::future::join_all(
        spans
            .iter()
            .map(|s| run_snippet(&content[s.cmd_start..s.cmd_end], ctx)),
    )
    .await;

    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    for (span, rendered) in spans.iter().zip(results) {
        out.push_str(&content[cursor..span.full_start]);
        out.push_str(&rendered);
        cursor = span.full_end;
    }
    out.push_str(&content[cursor..]);
    out
}

/// Run a single snippet via the platform shell, bounded by the context's
/// timeout and output cap. Returns the trimmed stdout, or an error marker.
async fn run_snippet(cmd: &str, ctx: &SkillPreprocessContext) -> String {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return String::new();
    }

    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut command = tokio::process::Command::new(shell);
    command
        .arg(flag)
        .arg(cmd)
        .current_dir(&ctx.skill_dir)
        .env("ALEPH_SKILL_DIR", &ctx.skill_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match tokio::time::timeout(ctx.timeout, command.no_window().output()).await {
        Ok(Ok(output)) => {
            let mut s = String::from_utf8_lossy(&output.stdout).into_owned();
            // Strip trailing newlines like POSIX command substitution.
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            if s.len() > ctx.max_output {
                let mut end = ctx.max_output;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
                s.push_str("…[truncated]");
            }
            s
        }
        Ok(Err(e)) => format!("[inline-shell error: {e}]"),
        Err(_) => format!(
            "[inline-shell error: timed out after {}s]",
            ctx.timeout.as_secs()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx_for(dir: &Path) -> SkillPreprocessContext {
        SkillPreprocessContext::new(dir.to_path_buf())
    }

    #[test]
    fn template_no_token_is_borrowed_unchanged() {
        let ctx = ctx_for(Path::new("/skills/demo"));
        let out = expand_template_vars("plain body, no tokens", &ctx);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "plain body, no tokens");
    }

    #[test]
    fn template_expands_skill_dir() {
        let ctx = ctx_for(Path::new("/skills/demo"));
        let out = expand_template_vars("run ${ALEPH_SKILL_DIR}/scripts/go.py now", &ctx);
        assert_eq!(out, "run /skills/demo/scripts/go.py now");
    }

    #[test]
    fn template_session_left_literal_when_unknown() {
        // `${ALEPH_SESSION_ID}` is no longer expanded by this module — the
        // wire that would feed the session id has been severed (no production
        // caller sets one). Tokens are now always left literal, matching the
        // hermes-agent reference's "unknown → literal" semantics.
        let ctx = ctx_for(Path::new("/skills/demo"));
        let out = expand_template_vars("session=${ALEPH_SESSION_ID}", &ctx);
        assert_eq!(out, "session=${ALEPH_SESSION_ID}");
    }

    #[test]
    fn frontmatter_optin_detected() {
        let allowed = "---\nname: Demo\ndescription: d\nallow-inline-shell: true\n---\nbody";
        let denied = "---\nname: Demo\ndescription: d\n---\nbody";
        let absent = "no frontmatter at all";
        assert!(frontmatter_allows_inline_shell(allowed));
        assert!(!frontmatter_allows_inline_shell(denied));
        assert!(!frontmatter_allows_inline_shell(absent));
    }

    #[test]
    fn spans_locate_multiple_snippets() {
        let content = "a !`one` b !`two` c";
        let spans = find_inline_spans(content);
        assert_eq!(spans.len(), 2);
        assert_eq!(&content[spans[0].cmd_start..spans[0].cmd_end], "one");
        assert_eq!(&content[spans[1].cmd_start..spans[1].cmd_end], "two");
    }

    #[test]
    fn spans_ignore_plain_backticks_and_unterminated() {
        // A plain code span has no leading `!` and must not match.
        assert!(find_inline_spans("inline `code` here").is_empty());
        // Unterminated snippet leaves the scan with nothing captured.
        assert!(find_inline_spans("dangling !`oops").is_empty());
    }

    #[tokio::test]
    async fn inline_shell_runs_when_opted_in() {
        let dir = tempfile::TempDir::new().unwrap();
        let content =
            "---\nname: Demo\ndescription: d\nallow-inline-shell: true\n---\nvalue=!`echo hello`.";
        let ctx = ctx_for(dir.path());
        let out = preprocess_skill_content(content, &ctx).await;
        assert!(out.contains("value=hello."), "got: {out}");
        // Frontmatter is untouched by inline-shell expansion.
        assert!(out.contains("allow-inline-shell: true"));
    }

    #[tokio::test]
    async fn inline_shell_skipped_without_optin() {
        let dir = tempfile::TempDir::new().unwrap();
        let content = "---\nname: Demo\ndescription: d\n---\nvalue=!`echo hello`.";
        let ctx = ctx_for(dir.path());
        let out = preprocess_skill_content(content, &ctx).await;
        // No opt-in → snippet preserved literally, no execution.
        assert!(out.contains("!`echo hello`"), "got: {out}");
    }

    #[tokio::test]
    async fn inline_shell_error_is_non_fatal() {
        let dir = tempfile::TempDir::new().unwrap();
        let content =
            "---\nname: Demo\ndescription: d\nallow-inline-shell: true\n---\n!`exit 1; echo never`";
        let ctx = ctx_for(dir.path());
        // A failing command yields empty stdout but never aborts; loading succeeds.
        let out = preprocess_skill_content(content, &ctx).await;
        assert!(out.contains("---"), "skill body should still load: {out}");
    }

    #[tokio::test]
    async fn preprocess_expands_dir_inside_snippet() {
        let dir = tempfile::TempDir::new().unwrap();
        // The snippet echoes the expanded skill-dir token.
        let content = "---\nname: Demo\ndescription: d\nallow-inline-shell: true\n---\ndir=!`echo ${ALEPH_SKILL_DIR}`";
        let ctx = ctx_for(dir.path());
        let out = preprocess_skill_content(content, &ctx).await;
        assert!(
            out.contains(&format!("dir={}", dir.path().display())),
            "got: {out}"
        );
    }
}
