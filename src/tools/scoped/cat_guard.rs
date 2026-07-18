//! Cat-guard: a non-blocking read-steer that nudges the model away from a raw
//! `file_read` / shell `cat` of an *installed skill* file and toward
//! `skill_read`, which expands `${ALEPH_SKILL_DIR}` / runs inline shell and
//! records skill usage that a raw read skips.
//!
//! Defense-in-depth, **not** a security boundary: the shell runs as the same OS
//! user and can always read the file. The guard only *surfaces* the fact and
//! lets the model self-correct on its next turn — it never blocks execution
//! (R7: no deterministic recovery, just a nudge). It mirrors hermes-agent's
//! `file_safety.py` read-steer, adapted to Aleph's `<system-reminder>`
//! tool-result seam and reusing the existing skill-dir classifiers as the
//! single source of truth for what counts as a skill root.
//!
//! Wired at the one tool-dispatch chokepoint ([`super::ScopedToolService::
//! execute_inner`]): the returned steer rides the same `<system-reminder>`
//! wrapping as hook `context:` lines and is applied only on a *successful*
//! read. MCP resources are server URIs with no on-disk root, so a `cat` of one
//! is not expressible as a filesystem path — the guard is deliberately
//! skill-only (the MCP surface is covered by `mcp_list_resources`).

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Whether the matched skill root is a user/project/agent skill or a
/// plugin-shipped one — only the steer wording differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillKind {
    Skill,
    PluginSkill,
}

/// Shell read verbs whose presence marks a command as *reading* a file, so
/// `ls .aleph/skills` / `cd` / `rm` on a skill path do not trip the steer.
const READ_VERBS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "bat", "view", "nl", "sed", "awk",
];

/// Cheap substring pre-filter: only pay for skill-dir enumeration + path
/// normalization when the raw argument plausibly references a skill root. Every
/// real skill dir contains one of these segments (`.aleph/skills`,
/// `.claude/skills`, `<plugin>/skills`, `agents/<id>/skills`).
fn plausibly_skill_path(raw: &str) -> bool {
    raw.contains("skill") || raw.contains(".aleph") || raw.contains(".claude")
}

/// Return a non-blocking skill-read steer for a `file_read` / shell tool call
/// whose target is inside an installed skill, or `None`.
///
/// Only `file_read` (structured `path`) and the two shell tools (`bash` / `code_exec`,
/// free-form command needing tokenization) are considered — the write/edit
/// tools are intentionally excluded so editing a skill is never nudged.
pub(crate) fn skill_read_steer(name: &str, input: &Value) -> Option<String> {
    match name {
        "file_read" => {
            let path = input.get("path")?.as_str()?;
            if !plausibly_skill_path(path) {
                return None;
            }
            let (kind, id) = classify_skill_path(Path::new(path))?;
            Some(build_steer(kind, &id, false))
        }
        "bash" | "code_exec" => {
            // `bash` carries the command in `cmd`, `code_exec` in `code`.
            let cmd = input
                .get("cmd")
                .or_else(|| input.get("code"))
                .and_then(Value::as_str)?;
            if !plausibly_skill_path(cmd) {
                return None;
            }
            let (kind, id) = shell_skill_read(cmd)?;
            Some(build_steer(kind, &id, true))
        }
        _ => None,
    }
}

/// Shell metacharacters that separate one command from the next. Redirections
/// (`<` / `>`) are deliberately absent — they stay *inside* a command so a
/// `cat < skills/foo/x.md` keeps `cat` bound to its operand.
fn is_command_separator(c: char) -> bool {
    matches!(c, '|' | ';' | '&' | '(' | ')' | '\n')
}

/// Collect the operand tokens that follow a read verb *within the same command
/// segment*. Binding the read verb to its own operands is what stops an
/// unrelated skill path elsewhere on the line from tripping the steer: in
/// `ls ~/.aleph/skills/foo && cat /etc/hosts` only `/etc/hosts` (cat's operand)
/// is returned — never the `ls` operand. Pure (no FS / cwd), so it is directly
/// unit-testable; [`shell_skill_read`] layers skill classification on top.
fn read_verb_operands(cmd: &str) -> Vec<&str> {
    let mut operands = Vec::new();
    for segment in cmd.split(is_command_separator) {
        let mut after_read_verb = false;
        for raw in segment.split_whitespace() {
            let tok = raw.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>'));
            if READ_VERBS.contains(&tok) {
                after_read_verb = true;
            } else if after_read_verb {
                operands.push(tok);
            }
        }
    }
    operands
}

/// Find the first read-verb operand in a shell command that classifies as an
/// installed-skill path. Replaces the former uncorrelated `command_reads` +
/// `first_skill_token` global scans, which could pair a `cat` in one segment
/// with a skill path listed by an unrelated command in another (false steer).
fn shell_skill_read(cmd: &str) -> Option<(SkillKind, String)> {
    read_verb_operands(cmd)
        .into_iter()
        .filter(|tok| plausibly_skill_path(tok))
        .find_map(|tok| classify_skill_path(Path::new(tok)))
}

/// Classify a candidate path as living inside a known skill root. Reuses the
/// existing skill-dir enumerators (SSOT): [`get_all_skills_dirs`] already lists
/// agent/project/global/plugin roots in precedence order, and
/// [`get_plugin_skills_dirs`] is the subset used to tag a plugin-shipped skill.
///
/// [`get_all_skills_dirs`]: crate::utils::paths::get_all_skills_dirs
/// [`get_plugin_skills_dirs`]: crate::utils::paths::get_plugin_skills_dirs
fn classify_skill_path(candidate: &Path) -> Option<(SkillKind, String)> {
    let abs = resolve_candidate(&candidate.to_string_lossy())?;
    let all: Vec<PathBuf> = crate::utils::paths::get_all_skills_dirs(None)
        .ok()?
        .iter()
        .map(|p| normalize_lexically(p))
        .collect();
    let plugin: Vec<PathBuf> = crate::utils::paths::get_plugin_skills_dirs(None)
        .iter()
        .map(|p| normalize_lexically(p))
        .collect();
    classify_against_roots(&abs, &all, &plugin)
}

/// Pure classification core: given an already-normalized absolute candidate and
/// the (normalized) skill-root lists, return the matched `(kind, skill_id)`.
/// Split out from [`classify_skill_path`] so it is unit-testable without
/// touching the filesystem or the process cwd. Skill layout is
/// `<root>/<skill_id>/SKILL.md`, so the skill id is the first path component
/// after the matched root.
fn classify_against_roots(
    abs: &Path,
    all_roots: &[PathBuf],
    plugin_roots: &[PathBuf],
) -> Option<(SkillKind, String)> {
    for root in all_roots {
        let Ok(rel) = abs.strip_prefix(root) else {
            continue;
        };
        let Some(first) = rel.components().next() else {
            // The candidate is the root itself — no skill id to name.
            continue;
        };
        let id = first.as_os_str().to_string_lossy().to_string();
        if id.is_empty() {
            continue;
        }
        let kind = if plugin_roots.iter().any(|p| p == root) {
            SkillKind::PluginSkill
        } else {
            SkillKind::Skill
        };
        return Some((kind, id));
    }
    None
}

/// Resolve a raw path string into an absolute, lexically-normalized `PathBuf`
/// *without* requiring the file to exist: expand a leading `~`, absolutize a
/// relative path against the current dir, then normalize `.`/`..` lexically.
///
/// Deliberately avoids [`std::fs::canonicalize`] — it requires existence and,
/// on Windows, emits `\\?\` verbatim-prefix paths that would never
/// `strip_prefix`-match a plainly-built skill root. Best-effort by design
/// (symlinks are not resolved): this feeds an advisory, not a security check.
fn resolve_candidate(raw: &str) -> Option<PathBuf> {
    let expanded: PathBuf = if let Some(rest) =
        raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"))
    {
        crate::utils::paths::get_home_dir().ok()?.join(rest)
    } else if raw == "~" {
        crate::utils::paths::get_home_dir().ok()?
    } else {
        PathBuf::from(raw)
    };
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().ok()?.join(expanded)
    };
    Some(normalize_lexically(&abs))
}

/// Lexically normalize `.`/`..`/redundant separators without touching the
/// filesystem (works for not-yet-existing paths; avoids Windows `\\?\`
/// canonicalization). Standard cargo-style `normalize_path`.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().copied() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };
    for component in components {
        match component {
            Component::Prefix(..) => {}
            Component::RootDir => ret.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => ret.push(c),
        }
    }
    ret
}

/// Build the non-blocking steer message for a classified skill hit.
fn build_steer(kind: SkillKind, id: &str, via_shell: bool) -> String {
    let shipped = match kind {
        SkillKind::Skill => "installed skill",
        SkillKind::PluginSkill => "plugin-shipped skill",
    };
    let how = if via_shell {
        "A shell command read a file"
    } else {
        "file_read targeted a file"
    };
    format!(
        "{how} inside {shipped} `{id}`. Prefer `skill_read(skill_id=\"{id}\")` for its \
         instructions, or `skill_read(skill_id=\"{id}\", file_name=\"<relative file>\")` for a \
         supporting file — skill_read expands `${{ALEPH_SKILL_DIR}}`, runs inline shell, and \
         records skill usage that a raw read skips. Use `skill_list` to see available skills. \
         (Advisory only — the read still ran; this is defense-in-depth, not a block.)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plausibly_skill_path_prefilters() {
        assert!(plausibly_skill_path("~/.aleph/skills/foo/SKILL.md"));
        assert!(plausibly_skill_path(".claude/skills/bar/x.md"));
        assert!(plausibly_skill_path("some/plugin/skills/y"));
        assert!(!plausibly_skill_path("/etc/hosts"));
        assert!(!plausibly_skill_path("src/main.rs"));
    }

    #[test]
    fn read_verb_operands_binds_verb_to_its_own_operands() {
        assert_eq!(
            read_verb_operands("cat .aleph/skills/foo/SKILL.md"),
            vec![".aleph/skills/foo/SKILL.md"]
        );
        // Flags between the verb and the path are captured too (a later
        // `plausibly_skill_path` filter drops the non-path ones).
        assert_eq!(
            read_verb_operands("sed -n '1,20p' .aleph/skills/foo/x.md"),
            vec!["-n", "1,20p", ".aleph/skills/foo/x.md"]
        );
        // A read verb after `&&` binds to the *second* command's operand only.
        assert_eq!(
            read_verb_operands("wc -l x && cat skills/foo/SKILL.md"),
            vec!["skills/foo/SKILL.md"]
        );
        // Pipe: head's operand captured; grep (not a read verb) contributes none.
        assert_eq!(
            read_verb_operands("head -5 skills/foo/README.md | grep x"),
            vec!["-5", "skills/foo/README.md"]
        );
        // No read verb anywhere → no operands (ls/cd/rm on a skill path are fine).
        assert!(read_verb_operands("ls .aleph/skills").is_empty());
        assert!(read_verb_operands("cd .aleph/skills && pwd").is_empty());
        assert!(read_verb_operands("rm -rf .aleph/skills/tmp").is_empty());
    }

    #[test]
    fn read_verb_operands_ignores_skill_path_of_unrelated_command() {
        // Regression for the two-scan false positive: `ls` lists a skill dir,
        // then an unrelated `cat` reads a non-skill file. The old design tripped
        // (a read verb existed *and* a skill token existed, uncorrelated). Now
        // `cat` binds to ITS operand only, so the skill path never appears.
        assert_eq!(
            read_verb_operands("ls ~/.aleph/skills/foo && cat /etc/hosts"),
            vec!["/etc/hosts"]
        );
    }

    #[test]
    fn compound_command_does_not_steer_on_unrelated_skill_listing() {
        // End-to-end: `/etc/hosts` is filtered out before any FS classification,
        // so this is deterministic regardless of the machine's real skill dirs.
        assert!(skill_read_steer(
            "bash",
            &json!({"cmd": "ls ~/.aleph/skills/foo && cat /etc/hosts"})
        )
        .is_none());
    }

    #[test]
    fn classify_against_roots_extracts_id_and_kind() {
        let root = PathBuf::from("/home/u/.aleph/skills");
        let plugin_root = PathBuf::from("/home/u/.aleph/plugins/p/skills");
        let all = vec![root, plugin_root.clone()];
        let plugins = vec![plugin_root];

        let hit = classify_against_roots(
            &PathBuf::from("/home/u/.aleph/skills/git-helper/SKILL.md"),
            &all,
            &plugins,
        );
        assert_eq!(hit, Some((SkillKind::Skill, "git-helper".to_string())));

        // A supporting file several levels deep still resolves to the skill id,
        // and a plugin root is tagged PluginSkill.
        let phit = classify_against_roots(
            &PathBuf::from("/home/u/.aleph/plugins/p/skills/deploy/scripts/run.sh"),
            &all,
            &plugins,
        );
        assert_eq!(phit, Some((SkillKind::PluginSkill, "deploy".to_string())));
    }

    #[test]
    fn classify_against_roots_ignores_non_skill_and_root_itself() {
        let root = PathBuf::from("/home/u/.aleph/skills");
        let all = vec![root.clone()];
        // Outside any skill root.
        assert!(
            classify_against_roots(&PathBuf::from("/home/u/project/src/main.rs"), &all, &[])
                .is_none()
        );
        // The root itself has no skill-id component after it.
        assert!(classify_against_roots(&root, &all, &[]).is_none());
    }

    #[test]
    fn normalize_lexically_resolves_dot_segments() {
        assert_eq!(
            normalize_lexically(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn steer_only_for_read_tools() {
        // Wrong tool → no steer even with a skill-ish arg (editing is not nudged).
        assert!(
            skill_read_steer("file_write", &json!({"path": "~/.aleph/skills/x/SKILL.md"}))
                .is_none()
        );
        // file_read with a non-skill path → no steer (pre-filter short-circuits).
        assert!(skill_read_steer("file_read", &json!({"path": "/etc/hosts"})).is_none());
        // A shell command with no read verb → no steer.
        assert!(
            skill_read_steer("bash", &json!({"cmd": "ls ~/.aleph/skills"})).is_none()
        );
    }

    #[test]
    fn build_steer_names_the_skill_and_tool() {
        let msg = build_steer(SkillKind::Skill, "git-helper", false);
        assert!(msg.contains("git-helper"));
        assert!(msg.contains("skill_read(skill_id=\"git-helper\")"));
        assert!(msg.contains("${ALEPH_SKILL_DIR}"));
        assert!(msg.contains("Advisory only"));

        let pmsg = build_steer(SkillKind::PluginSkill, "deploy", true);
        assert!(pmsg.contains("plugin-shipped skill"));
        assert!(pmsg.starts_with("A shell command read a file"));
    }
}
