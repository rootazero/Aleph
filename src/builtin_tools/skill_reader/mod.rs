//! Skill reading tools for AI agent integration
//!
//! Implements Claude's Progressive Disclosure pattern for skills:
//! - Level 1 (metadata) is always available in system prompt
//! - Level 2 (instructions) loaded via `read_skill` tool call
//! - Level 3 (resources) loaded on-demand via `file_name` parameter
//!
//! This enables the agent to actively request skill instructions,
//! treating them as task directives rather than passive context.

use std::path::Path;

mod list;
mod read;

#[cfg(test)]
mod tests;

pub use list::{ListSkillsArgs, ListSkillsOutput, ListSkillsTool, SkillSummary};
pub use read::{ReadSkillArgs, ReadSkillOutput, ReadSkillTool};

// ============================================================================
// Shared helpers
// ============================================================================

/// List supporting files in a skill dir, including `references/`, `scripts/`,
/// `assets/` subdirectories. Returns slash-joined relative paths. Hidden
/// entries and `SKILL.md` itself are skipped.
pub(super) fn list_skill_files(skill_dir: &Path) -> Vec<String> {
    // Depth cap: skills are shallow (references/, scripts/, assets/), so 16 is
    // generous. Without a bound, a symlink inside the skill dir that points to an
    // ancestor (a cycle) drives `walk` into unbounded recursion → stack overflow.
    const MAX_DEPTH: usize = 16;
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out, depth + 1);
            } else if name != "SKILL.md" {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(skill_dir, skill_dir, &mut files, 0);
    files.sort();
    files
}
