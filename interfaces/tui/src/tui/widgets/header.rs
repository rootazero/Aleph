// The transcript's first entry: who you are talking to, on what, about which
// folder.
//
// `ℵ Aleph 26.9.1 · claude-opus-5 · ~/proj (main)`
//
// Not stored in `AppState::messages`. It is derived at paint time from the
// state it describes, because every part of it can change after the entry
// would have been created: the model arrives with the session snapshot, and
// `/session` can move this screen to a conversation scoped to another folder.
// A stored first entry would be a second copy of those facts, going stale the
// moment either moved (判据 §1).

use std::path::Path;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::compact::fit_line;
use crate::tui::theme::theme;

/// The version of the binary the user launched, from the workspace `VERSION`
/// file via this crate's `build.rs` — not `CARGO_PKG_VERSION`, which is the
/// hand-synced copy.
pub const VERSION: &str = env!("ALEPH_VERSION");

const SEP: &str = " \u{b7} "; // ·

/// Build the header line.
///
/// Every argument after the model is optional and *absent* rather than
/// guessed: a conversation scoped to no folder shows no folder, and a folder
/// this machine cannot read shows no branch. The alternative — printing this
/// terminal's own `cwd` — would be the header stating a fact about a
/// different machine's filesystem (判据 §1, the copy that describes another
/// subsystem's behaviour).
#[must_use]
pub fn header_line(
    version: &str,
    model: &str,
    project_root: Option<&str>,
    branch: Option<&str>,
) -> Line<'static> {
    let colors = theme();
    let dim = Style::default().fg(colors.muted);
    let mut spans = vec![
        Span::styled(
            "\u{2135} ".to_string(), // ℵ
            Style::default()
                .fg(colors.user)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("Aleph {version}"),
            Style::default()
                .fg(colors.heading)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(SEP.to_string(), dim),
        Span::styled(model.to_string(), Style::default().fg(colors.tool_name)),
    ];
    if let Some(root) = project_root {
        spans.push(Span::styled(SEP.to_string(), dim));
        spans.push(Span::styled(
            abbreviate_home(root, home_dir().as_deref()),
            dim,
        ));
        if let Some(b) = branch {
            spans.push(Span::styled(format!(" ({b})"), dim));
        }
    }
    Line::from(spans)
}

/// Empty conversations get a welcome surface; history keeps the one-line identity.
/// Widths are terminal cells, not font sizes. All facts still come from the caller.
#[must_use]
pub fn header_lines(
    version: &str,
    model: &str,
    project_root: Option<&str>,
    branch: Option<&str>,
    width: u16,
    welcome: bool,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let identity = header_line(version, model, project_root, branch);
    if !welcome {
        return vec![fit_line(identity, width)];
    }
    let colors = theme();
    let title = Style::default()
        .fg(colors.heading)
        .add_modifier(Modifier::BOLD);
    let accent = Style::default()
        .fg(colors.user)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(colors.muted);
    let command = Style::default().fg(colors.tool_name);
    let mut lines = vec![Line::default()];
    if width >= 68 {
        let left = [
            "Welcome to Aleph".to_owned(),
            "".to_owned(),
            " ▄▀█ █   █▀▀ █▀█ █ █".to_owned(),
            " █▀█ █▄▄ ██▄ █▀▀ █▀█".to_owned(),
            format!(" v{version}"),
        ];
        let right = [
            ("Get started", ""),
            ("", "Ask a question to begin"),
            ("/help", "Commands & shortcuts"),
            ("/session", "Browse conversations"),
            ("/agents", "Background agents"),
        ];
        for (idx, (left, (key, description))) in left.into_iter().zip(right).enumerate() {
            let padding = 32usize.saturating_sub(left.width());
            lines.push(Line::from(vec![
                Span::styled(
                    left,
                    if idx == 0 {
                        title
                    } else if idx == 4 {
                        muted
                    } else {
                        accent
                    },
                ),
                Span::raw(" ".repeat(padding + 3)),
                Span::styled(key.to_owned(), if idx == 0 { title } else { command }),
                Span::raw(" ".repeat(10usize.saturating_sub(key.len()))),
                Span::styled(description.to_owned(), muted),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled("ℵ Welcome to Aleph", title)));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Ask a question to begin", muted)));
        for (key, description) in [
            ("/help", "Commands & shortcuts"),
            ("/session", "Conversations"),
            ("/agents", "Background agents"),
        ] {
            let mut spans = vec![Span::styled(key.to_owned(), command)];
            if width >= 32 {
                spans.push(Span::styled(format!("  {description}"), muted));
            }
            lines.push(Line::from(spans));
        }
    }
    lines.push(Line::default());
    lines.push(identity);
    lines
        .into_iter()
        .map(|line| fit_line(line, width))
        .collect()
}

/// `C:\Users\me\proj` → `~\proj`. Cosmetic only: a path that does not start
/// with the home directory is printed as it came.
///
/// `home` is a parameter rather than an ambient read so its test does not have
/// to set a process-wide environment variable — which in this test binary
/// would race every other test that reads one.
fn abbreviate_home(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if home.is_empty() || !path.starts_with(home) {
        return path.to_string();
    }
    let rest = &path[home.len()..];
    if rest.is_empty() {
        return "~".to_string();
    }
    // Only when the match ends on a separator — `/home/meagain` must not
    // become `~again`.
    if rest.starts_with('/') || rest.starts_with('\\') {
        return format!("~{rest}");
    }
    path.to_string()
}

/// This machine's home directory, as a string to compare a path prefix
/// against.
///
/// Delegates to [`aleph_protocol::paths::home_dir`] rather than reading the
/// variables here. B6 wrote its own `HOME`-then-`USERPROFILE` version, which
/// was the shared one minus its `HOMEDRIVE` + `HOMEPATH` arm — a copy that is
/// born weaker than its original and agrees with it on every machine except
/// the ones the missing arm exists for (判据 §1).
fn home_dir() -> Option<String> {
    aleph_protocol::paths::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|h| !h.is_empty())
}

/// The git branch checked out at `root`, read from this machine's filesystem.
///
/// # Why this is allowed to be wrong, and how it fails when it is
///
/// `project_root` is the server's answer — the folder the conversation is
/// scoped to. The branch is not on the wire at all, and no RPC reports it, so
/// the only way to know it is to read the path here. When the gateway runs on
/// this machine (loopback, which is the default and the overwhelming case)
/// that is the same filesystem and the answer is exact. When it does not, the
/// path almost always fails to resolve and the branch is simply absent — the
/// fail-closed direction. The residual: a remote gateway whose `project_root`
/// happens to name a path that *also* exists here would show this machine's
/// branch for it. Left standing rather than gated on a loopback check the
/// client has no accessor for, and recorded here so the next reader does not
/// have to re-derive it.
///
/// Returns the branch name, or the short commit for a detached HEAD. `None`
/// for anything it cannot read — never a placeholder, and never the string
/// `HEAD`.
#[must_use]
pub fn git_branch_of(root: &str) -> Option<String> {
    let git = Path::new(root).join(".git");
    // A worktree (and a submodule) has `.git` as a FILE holding
    // `gitdir: <path>`. This repository is itself a worktree, so the case is
    // not exotic.
    let head = if git.is_dir() {
        git.join("HEAD")
    } else {
        let pointer = std::fs::read_to_string(&git).ok()?;
        let dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        Path::new(dir).join("HEAD")
    };
    let contents = std::fs::read_to_string(head).ok()?;
    let contents = contents.trim();
    if let Some(reference) = contents.strip_prefix("ref:") {
        let name = reference.trim().rsplit('/').next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }
    // Detached HEAD: the raw object id. Short form, because the long one is
    // 40 columns of a header that has other things to say.
    let short: String = contents.chars().take(7).collect();
    (short.len() == 7 && short.chars().all(|c| c.is_ascii_hexdigit())).then_some(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn welcome_reflows_without_inventing_inventory_or_overflowing_cells() {
        for width in [0, 1, 20, 31, 32, 67, 68, 80, 120] {
            let lines = header_lines(
                "26.9.1",
                "model",
                Some("/w/proj"),
                Some("main"),
                width,
                true,
            );
            assert_eq!(lines.len(), if width >= 68 { 8 } else { 9 });
            assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
            let rendered = lines.iter().map(text).collect::<Vec<_>>().join("\n");
            assert!(!rendered.contains("skills"));
            if width >= 20 {
                assert!(rendered.contains("Welcome to Aleph"));
                assert!(rendered.contains("/help"));
                assert!(rendered.contains("/session"));
                assert!(rendered.contains("/agents"));
            }
        }
    }

    #[test]
    fn existing_conversations_keep_one_compact_identity_line() {
        let lines = header_lines("26.9.1", "model", Some("/w/proj"), Some("main"), 80, false);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            text(&lines[0]),
            text(&header_line(
                "26.9.1",
                "model",
                Some("/w/proj"),
                Some("main")
            ))
        );
        assert!(!text(&lines[0]).contains("Welcome"));
    }

    /// The full shape, and the one thing about it that is easy to get
    /// wrong: the version is the one `build.rs` derived from `VERSION`, not
    /// `CARGO_PKG_VERSION`.
    #[test]
    fn the_header_names_the_app_the_model_and_the_folder() {
        let line = header_line("26.9.1", "claude-opus-5", Some("/w/proj"), Some("main"));
        assert_eq!(
            text(&line),
            "\u{2135} Aleph 26.9.1 \u{b7} claude-opus-5 \u{b7} /w/proj (main)"
        );
    }

    /// A conversation scoped to no folder says nothing about a folder. It
    /// must NOT fall back to this terminal's own working directory: on a
    /// remote gateway that is a different machine's filesystem, and the
    /// reader has no way to tell the two apart.
    #[test]
    fn an_unscoped_conversation_shows_no_folder_and_no_branch() {
        let line = header_line("26.9.1", "m", None, Some("main"));
        assert_eq!(text(&line), "\u{2135} Aleph 26.9.1 \u{b7} m");
        let unreadable = header_line("26.9.1", "m", Some("/w/proj"), None);
        assert_eq!(
            text(&unreadable),
            "\u{2135} Aleph 26.9.1 \u{b7} m \u{b7} /w/proj"
        );
    }

    #[test]
    fn the_version_comes_from_the_version_file_not_the_cargo_copy() {
        // Both are CalVer strings, so a wrong wiring would still *look*
        // right; what pins it is that this is the file the release process
        // writes.
        // Anchor the path on the package manifest dir so the test works
        // regardless of the cwd cargo happens to invoke it from.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let from_file = std::fs::read_to_string(format!("{}/../../VERSION", manifest_dir))
            .expect("workspace VERSION file");
        assert_eq!(VERSION, from_file.trim());
    }

    #[test]
    fn home_is_abbreviated_only_on_a_component_boundary() {
        let home = Some("/home/me");
        assert_eq!(abbreviate_home("/home/me/proj", home), "~/proj");
        assert_eq!(abbreviate_home("/home/me", home), "~");
        // The trap: a sibling directory whose name merely starts with the
        // home path.
        assert_eq!(
            abbreviate_home("/home/meagain/proj", home),
            "/home/meagain/proj"
        );
        assert_eq!(abbreviate_home("/elsewhere", home), "/elsewhere");
        // No home known: print what came, rather than nothing.
        assert_eq!(abbreviate_home("/home/me/proj", None), "/home/me/proj");
        assert_eq!(abbreviate_home("/home/me/proj", Some("")), "/home/me/proj");
        // Windows separators take the same boundary rule.
        assert_eq!(
            abbreviate_home(r"C:\Users\me\proj", Some(r"C:\Users\me")),
            r"~\proj"
        );
    }

    /// The branch of the repository this test is compiled in. It is a
    /// worktree, so this also exercises the `.git`-is-a-file path — the one
    /// a `.git/HEAD`-only implementation gets wrong.
    #[test]
    fn a_worktrees_branch_is_readable_through_its_gitdir_pointer() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let branch = git_branch_of(root);
        assert!(
            branch.is_some(),
            "this crate's own repository root must resolve a branch: {root}"
        );
        let branch = branch.unwrap_or_default();
        assert_ne!(branch, "HEAD", "`ref: refs/heads/x` must yield `x`");
        assert!(!branch.contains('/'), "only the leaf name: {branch}");
    }

    #[test]
    fn a_folder_that_is_not_a_repository_has_no_branch() {
        assert_eq!(git_branch_of(env!("CARGO_MANIFEST_DIR")), None);
        assert_eq!(git_branch_of("/definitely/not/here"), None);
    }
}
