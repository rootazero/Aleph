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
use shared_ui_logic::transcript::SemanticColor;

use crate::tui::theme::palette;

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
    let dim = Style::default().fg(palette().color(SemanticColor::Dim));
    let mut spans = vec![
        Span::styled(
            "\u{2135} ".to_string(), // ℵ
            Style::default()
                .fg(palette().color(SemanticColor::Prompt))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("Aleph {version}"),
            Style::default()
                .fg(palette().color(SemanticColor::Fg))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(SEP.to_string(), dim),
        Span::styled(
            model.to_string(),
            Style::default().fg(palette().color(SemanticColor::Accent)),
        ),
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

fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))
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
        let from_file = std::fs::read_to_string("../../VERSION").expect("workspace VERSION file");
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
