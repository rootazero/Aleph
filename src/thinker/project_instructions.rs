//! Project-scoped instruction files (`CLAUDE.md` / `AGENTS.md`) discovered from
//! the active project workspace — Aleph's analog of Claude Code's per-project
//! `CLAUDE.md`.
//!
//! When an agent runs against a user-chosen project folder (the
//! `workspace_override`), this loader walks up from that folder to the git root
//! collecting instruction files and returns them as [`ExtraPromptFile`] entries.
//! The harness bridge merges them into the `[prompt.extra_files]` set so they
//! render through `ExtraFilesLayer` — reusing the same trust-boundary
//! sanitization (prompt-injection patterns + invisible Unicode) that identity
//! and extra files already get.
//!
//! The default agent workspace (`~/.aleph/workspaces/{agent_id}`) has no such
//! files, so the loader naturally yields nothing there — project instructions
//! only appear when the user explicitly picks a project folder.

use std::collections::HashSet;
use std::path::Path;

use crate::thinker::prompt_budget::truncate_with_head_tail;
use crate::thinker::prompt_layer::ExtraPromptFile;

/// Per-file character cap (mirrors identity / extra-files loaders).
const PER_FILE_MAX_CHARS: usize = 20_000;
/// Total character budget across all project instruction files. Lower than the
/// extra-files cap so a giant monorepo-root `CLAUDE.md` can't dominate context.
const TOTAL_MAX_CHARS: usize = 32_000;

/// Instruction file names checked at each directory level, in render order.
/// `.aleph/` is Aleph-native and preferred; `.claude/` is read for Claude Code
/// compatibility. Top-level `CLAUDE.md` / `AGENTS.md` cover the common case
/// where the file sits at the project root.
const CANDIDATES: &[&str] = &[
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    "AGENTS.md",
    ".aleph/AGENTS.md",
];

/// Discover project instruction files for `workspace`, walking up to the git
/// root (or the filesystem root when not in a repo).
///
/// Entries are ordered outermost-first (git root → workspace) so the most
/// specific instructions — those closest to the chosen folder — appear last and
/// therefore carry the most weight in the model's reading, mirroring Claude
/// Code's "closer to cwd wins" precedence.
///
/// Returns an empty vec when no instruction files exist (e.g. the default agent
/// workspace), which the caller treats as "inject nothing".
#[must_use]
pub fn load_project_instructions(workspace: &Path) -> Vec<ExtraPromptFile> {
    let git_root = crate::utils::paths::find_git_root(workspace);

    // Collect directories from workspace upward to the git root (inclusive).
    let mut dirs = Vec::new();
    let mut current = workspace.to_path_buf();
    loop {
        dirs.push(current.clone());
        if git_root.as_deref() == Some(current.as_path()) {
            break;
        }
        if !current.pop() {
            break;
        }
    }
    // Outermost-first: git root before the workspace folder.
    dirs.reverse();

    let mut out = Vec::new();
    let mut total = 0usize;
    let mut seen = HashSet::new();

    for dir in &dirs {
        for candidate in CANDIDATES {
            if total >= TOTAL_MAX_CHARS {
                return out;
            }
            let path = dir.join(candidate);
            // Dedupe by canonical path so a workspace == git_root walk (or
            // symlinked ancestors) never injects the same file twice.
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if content.trim().is_empty() {
                continue;
            }

            let budget = PER_FILE_MAX_CHARS.min(TOTAL_MAX_CHARS - total);
            let capped = if content.chars().count() > budget {
                truncate_with_head_tail(&content, budget, 0.7, 0.2)
            } else {
                content
            };
            total += capped.chars().count();

            out.push(ExtraPromptFile {
                name: format!("Project instructions: {}", path.display()),
                content: capped,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn empty_workspace_yields_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let files = load_project_instructions(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn loads_top_level_claude_md() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("CLAUDE.md"), "be terse");
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].name.contains("CLAUDE.md"));
        assert_eq!(files[0].content, "be terse");
    }

    #[test]
    fn loads_claude_subdir_and_agents() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join(".claude/CLAUDE.md"), "rules here");
        write(&tmp.path().join("AGENTS.md"), "agents here");
        let files = load_project_instructions(tmp.path());
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains(".claude/CLAUDE.md")));
        assert!(names.iter().any(|n| n.contains("AGENTS.md")));
    }

    #[test]
    fn walks_up_to_git_root_outermost_first() {
        let tmp = tempfile::tempdir().unwrap();
        // git root with a CLAUDE.md, plus a nested subdir with its own.
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        write(&tmp.path().join("CLAUDE.md"), "root rules");
        let sub = tmp.path().join("crates/api");
        write(&sub.join("CLAUDE.md"), "api rules");

        let files = load_project_instructions(&sub);
        assert_eq!(files.len(), 2, "{files:?}");
        // Outermost (git root) first, most-specific (workspace) last.
        assert_eq!(files[0].content, "root rules");
        assert_eq!(files[1].content, "api rules");
    }

    #[test]
    fn skips_blank_files() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("CLAUDE.md"), "   \n\t  ");
        let files = load_project_instructions(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn caps_total_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(PER_FILE_MAX_CHARS * 2);
        write(&tmp.path().join("CLAUDE.md"), &big);
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.chars().count() <= PER_FILE_MAX_CHARS);
    }
}
