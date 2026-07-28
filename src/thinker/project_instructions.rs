//! Project-scoped instruction files (`CLAUDE.md` / `AGENTS.md` / `.claude/rules`)
//! discovered from the active project workspace — Aleph's analog of Claude
//! Code's per-project `CLAUDE.md`.
//!
//! When an agent runs against a user-chosen project folder (the
//! `workspace_override`), this loader walks up from that folder to the git root
//! collecting instruction files. [`discover_project_instructions`] is the single
//! discovery source of truth; both prompt presenters consume it:
//!   * the **orchestrator path** via [`load_project_instructions`] → the harness
//!     bridge merges the [`ExtraPromptFile`] set into `[prompt.extra_files]` so
//!     they render through `ExtraFilesLayer` (with the same trust-boundary
//!     sanitization identity/extra files get); and
//!   * the **legacy gateway path** via `collect_project_context_blocks` in the
//!     execution engine, which formats the same files into a per-turn
//!     `<system-reminder>` block.
//!
//! Keeping one discovery function means both paths see the identical file set
//! (`CLAUDE.md`, `.aleph/CLAUDE.md`, `CLAUDE.local.md`, rules, `@import`, …) —
//! no capability drift between execution modes.
//!
//! Beyond the top-level instruction files this loader also supports (Slice 2a):
//!   * `CLAUDE.local.md` — gitignored local overrides (read after the base file).
//!   * `.claude/rules/*.md` and `.aleph/rules/*.md` — supplementary rule files,
//!     each surfaced as its own entry in lexical order.
//!   * `@import` directives inside any loaded file — inlined recursively (max
//!     depth 5, cycle-guarded). Imports are confined to the git-root subtree:
//!     an `@~/...`, `@/abs/outside`, or any path resolving outside the workspace
//!     is left as a skip marker, so a cloned-but-untrusted repo cannot exfiltrate
//!     `~/.ssh` or `/etc/passwd` into the prompt.
//!
//! The default agent workspace (`~/.aleph/workspaces/{agent_id}`) has no such
//! files, so the loader naturally yields nothing there — project instructions
//! only appear when the user explicitly picks a project folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::thinker::prompt_budget::truncate_with_head_tail;
use crate::thinker::prompt_layer::ExtraPromptFile;

/// Per-file character cap (mirrors identity / extra-files loaders).
const PER_FILE_MAX_CHARS: usize = 20_000;
/// Total character budget across all project instruction files. Lower than the
/// extra-files cap so a giant monorepo-root `CLAUDE.md` can't dominate context.
const TOTAL_MAX_CHARS: usize = 32_000;
/// Maximum `@import` recursion depth (mirrors Claude Code's import depth limit).
const MAX_IMPORT_DEPTH: usize = 5;

/// Instruction file names checked at each directory level, in render order.
/// `.aleph/` is Aleph-native and preferred; `.claude/` is read for Claude Code
/// compatibility. `CLAUDE.local.md` carries gitignored local overrides and is
/// read after its base file so it reads with more weight.
const CANDIDATES: &[&str] = &[
    "CLAUDE.md",
    "CLAUDE.local.md",
    ".claude/CLAUDE.md",
    ".claude/CLAUDE.local.md",
    ".aleph/CLAUDE.md",
    "AGENTS.md",
    ".aleph/AGENTS.md",
];

/// Subdirectories globbed for supplementary `*.md` rule files at each level.
const RULE_DIRS: &[&str] = &[".claude/rules", ".aleph/rules"];

/// Belt-and-suspenders cap on ancestor traversal when the workspace is not in a
/// git repo (so `find_git_root` returns `None` and the walk would otherwise run
/// to the filesystem root). The git-root and `$HOME` stops normally trip first.
const MAX_WALK_DEPTH: usize = 8;

/// One discovered project instruction file: its on-disk path, a short display
/// label (relative to the workspace when possible), and its budget-capped,
/// `@import`-expanded content. This is the structured form both prompt
/// presenters consume — the orchestrator path ([`load_project_instructions`] →
/// `ExtraFilesLayer`) and the legacy gateway path
/// (`collect_project_context_blocks` → per-turn system-reminder).
#[derive(Debug, Clone)]
pub struct ProjectInstructionFile {
    /// Absolute path the content was read from.
    pub path: PathBuf,
    /// Display label: path relative to the workspace, or absolute for ancestors.
    pub label: String,
    /// `@import`-expanded, budget-capped file content.
    pub content: String,
}

/// Label a discovered file relative to the workspace when it sits inside it
/// (`CLAUDE.md`, `.claude/rules/a.md`), else the absolute path (ancestor files).
fn relative_label(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|rel| {
            // Always label with `/`. This is a display string for the model's
            // prompt, not a path the OS consumes (`path` carries that), so a
            // separator that changed with the host would make the same project
            // produce a different prompt on Windows.
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| path.display().to_string())
}

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
///
/// This is the single discovery source of truth shared by both prompt
/// presenters; [`load_project_instructions`] wraps it for the orchestrator path.
#[must_use]
pub fn discover_project_instructions(workspace: &Path) -> Vec<ProjectInstructionFile> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let git_root = crate::utils::paths::find_git_root(&workspace);
    // `@import` boundary: the git root, or the workspace itself outside a repo.
    let boundary = git_root.clone().unwrap_or_else(|| workspace.clone());
    let home = dirs::home_dir().map(|home| home.canonicalize().unwrap_or(home));

    // Collect directories from workspace upward to the git root (inclusive).
    // Stop at the git root, `$HOME`, or a depth cap — whichever trips first —
    // so a workspace outside any repo can't walk to the filesystem root.
    let mut dirs = Vec::new();
    let mut current = workspace.clone();
    loop {
        dirs.push(current.clone());
        if git_root.as_deref() == Some(current.as_path()) {
            break;
        }
        if home.as_deref() == Some(current.as_path()) {
            break;
        }
        if dirs.len() >= MAX_WALK_DEPTH {
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
        // Ordered file list at this level: fixed candidates, then globbed rules.
        let mut files: Vec<PathBuf> = CANDIDATES.iter().map(|c| dir.join(c)).collect();
        for rule_dir in RULE_DIRS {
            files.extend(glob_rule_files(&dir.join(rule_dir)));
        }

        for path in files {
            if total >= TOTAL_MAX_CHARS {
                return out;
            }
            // Dedupe by canonical path so a workspace == git_root walk (or
            // symlinked ancestors) never injects the same file twice.
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical.clone()) {
                continue;
            }
            let raw = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if raw.trim().is_empty() {
                continue;
            }

            // Inline `@import` directives relative to this file's own directory,
            // bounded by the workspace subtree. Seed `visited` with this file so
            // a self-import is caught as a cycle.
            let file_dir = path.parent().map_or_else(|| dir.clone(), Path::to_path_buf);
            let mut visited = HashSet::new();
            visited.insert(canonical);
            let expanded = expand_imports(&raw, &file_dir, &boundary, &mut visited, 0);
            if expanded.trim().is_empty() {
                continue;
            }

            let budget = PER_FILE_MAX_CHARS.min(TOTAL_MAX_CHARS - total);
            let capped = if expanded.chars().count() > budget {
                truncate_with_head_tail(&expanded, budget, 0.7, 0.2)
            } else {
                expanded
            };
            total += capped.chars().count();

            out.push(ProjectInstructionFile {
                label: relative_label(&path, &workspace),
                path,
                content: capped,
            });
        }
    }

    out
}

/// Orchestrator-path presenter: discover project instructions and wrap each as
/// an [`ExtraPromptFile`] for `ExtraFilesLayer` injection.
#[must_use]
pub fn load_project_instructions(workspace: &Path) -> Vec<ExtraPromptFile> {
    discover_project_instructions(workspace)
        .into_iter()
        .map(|f| ExtraPromptFile {
            name: format!("Project instructions: {}", f.label),
            content: f.content,
        })
        .collect()
}

/// List `*.md` files directly under `dir` in lexical order. Missing dir → empty.
fn glob_rule_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "md"))
            .collect(),
        Err(_) => return Vec::new(),
    };
    files.sort();
    files
}

/// Recursively inline `@import` directives in `content`.
///
/// `file_dir` is the directory of the file being expanded (relative imports
/// resolve against it); `boundary` is the workspace subtree root that imports
/// may not escape. Returns the original content unchanged when there is nothing
/// to expand, preserving byte-for-byte fidelity for the common (no-import) case.
fn expand_imports(
    content: &str,
    file_dir: &Path,
    boundary: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    if depth >= MAX_IMPORT_DEPTH || !content.contains('@') {
        return content.to_string();
    }

    let had_trailing_nl = content.ends_with('\n');
    let mut in_fence = false;
    let mut lines_out: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        // Fenced code blocks are passed through verbatim — `@tokens` shown in a
        // code example must not be treated as imports (Claude Code parity).
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            lines_out.push(line.to_string());
            continue;
        }
        if in_fence || !line.contains('@') {
            lines_out.push(line.to_string());
            continue;
        }
        lines_out.push(expand_line(line, file_dir, boundary, visited, depth));
    }

    let mut joined = lines_out.join("\n");
    if had_trailing_nl {
        joined.push('\n');
    }
    joined
}

/// Expand any `@path` import tokens within a single (non-fenced) line.
fn expand_line(
    line: &str,
    file_dir: &Path,
    boundary: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut result = String::new();
    let mut backticks = 0usize;
    let mut prev_alnum = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            backticks += 1;
            result.push(c);
            prev_alnum = false;
            i += 1;
            continue;
        }
        // Trigger only when `@` opens a token (not mid-word like an email
        // `user@host`) and we are outside an inline code span.
        if c == '@' && backticks.is_multiple_of(2) && !prev_alnum {
            let mut j = i + 1;
            while j < chars.len() && !chars[j].is_whitespace() && chars[j] != '`' {
                j += 1;
            }
            let token: String = chars[i + 1..j].iter().collect();
            if looks_like_path(&token) {
                result.push_str(&resolve_import_token(
                    &token, file_dir, boundary, visited, depth,
                ));
                prev_alnum = false;
                i = j;
                continue;
            }
        }
        result.push(c);
        prev_alnum = c.is_alphanumeric();
        i += 1;
    }

    result
}

/// A token is import-worthy only if it actually looks like a path (has a
/// separator, an extension dot, or a home prefix) — this excludes `@mention`.
fn looks_like_path(token: &str) -> bool {
    !token.is_empty() && (token.contains('/') || token.contains('.') || token.starts_with('~'))
}

/// Resolve an import token to inlined content, or to a skip marker when the
/// target is missing, escapes the workspace subtree, or forms a cycle.
fn resolve_import_token(
    token: &str,
    file_dir: &Path,
    boundary: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    // `~`-relative paths always resolve to $HOME, outside the workspace subtree.
    if token.starts_with('~') {
        return format!("<!-- import skipped (outside workspace): {token} -->");
    }

    let candidate = if Path::new(token).is_absolute() {
        PathBuf::from(token)
    } else {
        file_dir.join(token)
    };
    let canonical = match candidate.canonicalize() {
        Ok(c) => c,
        Err(_) => return format!("<!-- import not found: {token} -->"),
    };

    let boundary_c = boundary
        .canonicalize()
        .unwrap_or_else(|_| boundary.to_path_buf());
    if !canonical.starts_with(&boundary_c) {
        return format!("<!-- import skipped (outside workspace): {token} -->");
    }
    // Cycle / already-imported guard: visited is seeded with the root file and
    // accumulates across the whole expansion of one top-level file.
    if !visited.insert(canonical.clone()) {
        return format!("<!-- import skipped (cycle): {token} -->");
    }
    let content = match std::fs::read_to_string(&canonical) {
        Ok(c) => c,
        Err(_) => return format!("<!-- import not found: {token} -->"),
    };
    let child_dir = canonical
        .parent()
        .map_or_else(|| file_dir.to_path_buf(), Path::to_path_buf);
    expand_imports(&content, &child_dir, boundary, visited, depth + 1)
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

    /// Isolated workspace: a tempdir with a `.git` marker so discovery treats it
    /// as the git root and stops the ancestor walk there. Without this, on
    /// platforms whose system temp dir lives under `$HOME` (e.g. Windows
    /// `%LOCALAPPDATA%\Temp`), the walk would climb to the real `~/.claude/`
    /// instruction files and pollute exact-count/content assertions.
    fn isolated_ws() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        tmp
    }

    #[test]
    fn empty_workspace_yields_nothing() {
        let tmp = isolated_ws();
        let files = load_project_instructions(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn loads_top_level_claude_md() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "be terse");
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].name.contains("CLAUDE.md"));
        assert_eq!(files[0].content, "be terse");
    }

    #[test]
    fn loads_claude_subdir_and_agents() {
        let tmp = isolated_ws();
        write(&tmp.path().join(".claude/CLAUDE.md"), "rules here");
        write(&tmp.path().join("AGENTS.md"), "agents here");
        let files = load_project_instructions(tmp.path());
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains(".claude/CLAUDE.md")));
        assert!(names.iter().any(|n| n.contains("AGENTS.md")));
    }

    #[test]
    fn walks_up_to_git_root_outermost_first() {
        let tmp = isolated_ws();
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

    #[cfg(unix)]
    #[test]
    fn symlinked_workspace_walks_canonical_repo_ancestors_only() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(&repo.join("CLAUDE.md"), "repo rules");
        let workspace = repo.join("crates/api");
        write(&workspace.join("CLAUDE.md"), "workspace rules");

        let link_parent = tmp.path().join("outside");
        write(&link_parent.join("CLAUDE.md"), "outside rules");
        let link = link_parent.join("workspace");
        std::os::unix::fs::symlink(&workspace, &link).unwrap();

        let files = discover_project_instructions(&link);
        let contents: Vec<&str> = files.iter().map(|file| file.content.as_str()).collect();
        let repo_pos = contents
            .iter()
            .position(|content| *content == "repo rules")
            .unwrap();
        let workspace_pos = contents
            .iter()
            .position(|content| *content == "workspace rules")
            .unwrap();
        assert!(repo_pos < workspace_pos);
        assert!(!contents.contains(&"outside rules"));
    }

    #[test]
    fn skips_blank_files() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "   \n\t  ");
        let files = load_project_instructions(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn caps_total_budget() {
        let tmp = isolated_ws();
        let big = "x".repeat(PER_FILE_MAX_CHARS * 2);
        write(&tmp.path().join("CLAUDE.md"), &big);
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.chars().count() <= PER_FILE_MAX_CHARS);
    }

    // --- Slice 2a: instruction expansion ---

    #[test]
    fn loads_claude_local_after_base() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "base");
        write(&tmp.path().join("CLAUDE.local.md"), "local override");
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].content, "base");
        assert_eq!(files[1].content, "local override");
        assert!(files[1].name.contains("CLAUDE.local.md"));
    }

    #[test]
    fn globs_rule_files_in_lexical_order() {
        let tmp = isolated_ws();
        write(&tmp.path().join(".claude/rules/b-style.md"), "style rule");
        write(&tmp.path().join(".claude/rules/a-arch.md"), "arch rule");
        write(&tmp.path().join(".claude/rules/notes.txt"), "ignored");
        let files = load_project_instructions(tmp.path());
        let contents: Vec<&str> = files.iter().map(|f| f.content.as_str()).collect();
        assert_eq!(contents, vec!["arch rule", "style rule"]);
    }

    #[test]
    fn inlines_relative_import() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "head\n@./extra.md\ntail");
        write(&tmp.path().join("extra.md"), "imported body");
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(
            files[0].content.contains("imported body"),
            "{}",
            files[0].content
        );
        assert!(files[0].content.contains("head"));
        assert!(files[0].content.contains("tail"));
        assert!(!files[0].content.contains("@./extra.md"));
    }

    #[test]
    fn inlines_inline_import_in_prose() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "see @./extra.md now");
        write(&tmp.path().join("extra.md"), "BODY");
        let files = load_project_instructions(tmp.path());
        assert!(
            files[0].content.contains("see BODY now"),
            "{}",
            files[0].content
        );
    }

    #[test]
    fn inlines_imports_recursively() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "@./a.md");
        write(&tmp.path().join("a.md"), "A\n@./b.md");
        write(&tmp.path().join("b.md"), "B-deep");
        let files = load_project_instructions(tmp.path());
        assert!(files[0].content.contains("A"));
        assert!(files[0].content.contains("B-deep"), "{}", files[0].content);
    }

    #[test]
    fn import_cycle_is_marked_not_infinite() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "@./a.md");
        write(&tmp.path().join("a.md"), "A\n@./b.md");
        write(&tmp.path().join("b.md"), "B\n@./a.md");
        let files = load_project_instructions(tmp.path());
        assert!(files[0].content.contains("A"));
        assert!(files[0].content.contains("B"));
        assert!(
            files[0].content.contains("import skipped (cycle)"),
            "{}",
            files[0].content
        );
    }

    #[test]
    fn import_outside_workspace_is_skipped() {
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("secret.md"), "TOP SECRET");
        let abs = outside.path().join("secret.md");

        let tmp = isolated_ws();
        // boundary is the git root (== workspace); the absolute path escapes it.
        write(
            &tmp.path().join("CLAUDE.md"),
            &format!("@{}", abs.display()),
        );
        let files = load_project_instructions(tmp.path());
        assert!(
            !files[0].content.contains("TOP SECRET"),
            "{}",
            files[0].content
        );
        assert!(files[0]
            .content
            .contains("import skipped (outside workspace)"));
    }

    #[test]
    fn home_relative_import_is_skipped() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "@~/.ssh/id_rsa");
        let files = load_project_instructions(tmp.path());
        assert!(files[0]
            .content
            .contains("import skipped (outside workspace)"));
    }

    #[test]
    fn missing_import_is_marked() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "@./nope.md");
        let files = load_project_instructions(tmp.path());
        assert!(
            files[0].content.contains("import not found"),
            "{}",
            files[0].content
        );
    }

    #[test]
    fn import_in_code_fence_not_expanded() {
        let tmp = isolated_ws();
        write(
            &tmp.path().join("CLAUDE.md"),
            "intro\n```text\n@./extra.md\n```\ndone",
        );
        write(&tmp.path().join("extra.md"), "imported body");
        let files = load_project_instructions(tmp.path());
        assert!(
            files[0].content.contains("@./extra.md"),
            "{}",
            files[0].content
        );
        assert!(!files[0].content.contains("imported body"));
    }

    #[test]
    fn inline_backtick_import_not_expanded() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "use `@./extra.md` literally");
        write(&tmp.path().join("extra.md"), "imported body");
        let files = load_project_instructions(tmp.path());
        assert!(
            files[0].content.contains("`@./extra.md`"),
            "{}",
            files[0].content
        );
        assert!(!files[0].content.contains("imported body"));
    }

    #[test]
    fn email_is_not_treated_as_import() {
        let tmp = isolated_ws();
        write(
            &tmp.path().join("CLAUDE.md"),
            "contact me@example.com for help",
        );
        let files = load_project_instructions(tmp.path());
        // Untouched: no import marker, original text preserved.
        assert_eq!(files[0].content, "contact me@example.com for help");
    }

    // --- Path unification: .aleph/CLAUDE.md candidate + structured discovery ---

    #[test]
    fn loads_aleph_claude_md() {
        let tmp = isolated_ws();
        write(&tmp.path().join(".aleph/CLAUDE.md"), "aleph project rules");
        let files = load_project_instructions(tmp.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].name.contains(".aleph/CLAUDE.md"));
        assert_eq!(files[0].content, "aleph project rules");
    }

    #[test]
    fn discover_yields_relative_labels_for_in_workspace_files() {
        let tmp = isolated_ws();
        write(&tmp.path().join("CLAUDE.md"), "root");
        write(&tmp.path().join(".claude/CLAUDE.md"), "sub");
        let files = discover_project_instructions(tmp.path());
        let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.contains(&"CLAUDE.md"), "{labels:?}");
        assert!(labels.contains(&".claude/CLAUDE.md"), "{labels:?}");
    }

    #[test]
    fn discover_labels_ancestor_files_absolutely() {
        let tmp = isolated_ws();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        write(&tmp.path().join("CLAUDE.md"), "root rules");
        let sub = tmp.path().join("crates/api");
        fs::create_dir_all(&sub).unwrap();
        write(&sub.join("CLAUDE.md"), "api rules");

        let files = discover_project_instructions(&sub);
        // Ancestor (git root) file labelled by absolute path; workspace file relative.
        assert_eq!(files[0].content, "root rules");
        assert!(files[0].label.contains("CLAUDE.md") && Path::new(&files[0].label).is_absolute());
        assert_eq!(files[1].label, "CLAUDE.md");
    }
}
