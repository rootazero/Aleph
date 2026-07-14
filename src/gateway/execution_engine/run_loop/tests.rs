//! Tests carved verbatim from the original `run_loop.rs` `project_context_tests`
//! module. Header re-imports the original top-of-file `use` block plus sibling
//! modules so the dedented bodies keep resolving the same items.
#![allow(unused_imports)]

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::sync_primitives::Arc;

use super::super::{ExecutionError, RunRequest};
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::super::engine::ExecutionEngine;

use super::inner::*;
use super::project_context::*;

use super::*;
use tempfile::tempdir;

/// Mark `dir` as a `.git` boundary so the discovery walk halts there.
/// Tests build their workspaces inside a tempdir and would otherwise
/// walk up to whichever directory holds the test runner — sometimes a
/// user's real `~/.aleph/...` layout — and pick up files that pollute
/// assertions. Calling this on the workspace root keeps the walk
/// confined to the tempdir.
fn anchor(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
}

#[test]
fn workspace_directive_names_the_effective_directory() {
    // The directive must always carry the resolved path verbatim so the
    // model writes there instead of inventing one. Holds for the default
    // `~/.aleph/workspaces/{id}` path and for a project override alike —
    // it is the same helper fed by `effective_workspace` in both modes.
    let default_ws = std::path::Path::new("/home/u/.aleph/workspaces/main");
    let d = workspace_directive(default_ws);
    assert!(d.contains("/home/u/.aleph/workspaces/main"));
    assert!(d.to_lowercase().contains("working directory"));

    let project_ws = std::path::Path::new("/home/u/projects/paris-riot-timeline");
    let p = workspace_directive(project_ws);
    assert!(p.contains("/home/u/projects/paris-riot-timeline"));
}

#[test]
fn returns_nothing_when_no_project_files() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    let blocks = collect_project_context_blocks(dir.path());
    assert!(blocks.is_empty());
}

#[test]
fn reads_agents_md_when_present() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "# Project rules\nNo force push.\n",
    )
    .unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].contains("Active project"));
    assert!(blocks[0].contains("### AGENTS.md"));
    assert!(blocks[0].contains("No force push"));
}

#[test]
fn includes_both_agents_and_claude_md_when_both_present() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    std::fs::write(dir.path().join("AGENTS.md"), "# Aleph rules").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "# CC rules").unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].contains("### AGENTS.md"));
    assert!(blocks[0].contains("### CLAUDE.md"));
}

#[test]
fn ignores_whitespace_only_files() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    std::fs::write(dir.path().join("AGENTS.md"), "   \n\n\t\n").unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert!(blocks.is_empty());
}

#[test]
fn truncates_oversized_files() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    // Larger than the shared per-file char cap (20k) so discovery truncates.
    let big = "x".repeat(40_000);
    std::fs::write(dir.path().join("AGENTS.md"), &big).unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    // Unified truncation marker from `truncate_with_head_tail`.
    assert!(blocks[0].contains("truncated"));
    assert!(blocks[0].len() < big.len());
}

#[test]
fn loads_claude_md_in_subdir() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(dir.path().join(".claude/CLAUDE.md"), "# CC sub").unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert!(blocks[0].contains(".claude/CLAUDE.md"));
    assert!(blocks[0].contains("# CC sub"));
}

#[test]
fn loads_aleph_claude_md_in_subdir() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    std::fs::create_dir_all(dir.path().join(".aleph")).unwrap();
    std::fs::write(dir.path().join(".aleph/CLAUDE.md"), "# Aleph sub").unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert!(blocks[0].contains(".aleph/CLAUDE.md"));
    assert!(blocks[0].contains("# Aleph sub"));
}

/// Walk-up: parent CLAUDE.md is included, and parent appears BEFORE
/// the project root's so the LLM reads parent first → project last
/// (last-wins ordering).
#[test]
fn walks_up_to_ancestor_claude_md_until_git_boundary() {
    let root = tempdir().unwrap();
    anchor(root.path()); // `.git` lives on the outer dir (the repo root)
    std::fs::write(root.path().join("CLAUDE.md"), "# outer").unwrap();
    let inner = root.path().join("packages").join("svc");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("CLAUDE.md"), "# inner").unwrap();

    let blocks = collect_project_context_blocks(&inner);
    assert_eq!(blocks.len(), 1);
    let body = &blocks[0];
    let outer_pos = body
        .find("# outer")
        .expect("outer CLAUDE.md must be injected");
    let inner_pos = body
        .find("# inner")
        .expect("inner CLAUDE.md must be injected");
    assert!(
        outer_pos < inner_pos,
        "ancestor must appear before project root so last-wins ordering holds"
    );
}

/// Walk-up halts at `.git`: a CLAUDE.md sitting above the boundary is
/// NOT injected, even if it physically exists on disk.
#[test]
fn walk_stops_at_git_boundary() {
    let outer = tempdir().unwrap();
    std::fs::write(outer.path().join("CLAUDE.md"), "# above boundary").unwrap();
    let project = outer.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    anchor(&project); // `.git` lives ON the project root → stops walk there
    std::fs::write(project.join("CLAUDE.md"), "# project").unwrap();

    let blocks = collect_project_context_blocks(&project);
    assert!(blocks[0].contains("# project"));
    assert!(
        !blocks[0].contains("# above boundary"),
        "files above the .git boundary must NOT leak into project context"
    );
}

#[test]
fn loads_claude_rules_glob() {
    let dir = tempdir().unwrap();
    anchor(dir.path());
    let rules = dir.path().join(".claude").join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join("a.md"), "rule alpha").unwrap();
    std::fs::write(rules.join("b.md"), "rule beta").unwrap();
    std::fs::write(rules.join("ignored.txt"), "not a rule").unwrap();
    let blocks = collect_project_context_blocks(dir.path());
    assert!(blocks[0].contains("rule alpha"));
    assert!(blocks[0].contains("rule beta"));
    assert!(!blocks[0].contains("not a rule"));
    // a.md should appear before b.md (sort order).
    assert!(blocks[0].find("rule alpha").unwrap() < blocks[0].find("rule beta").unwrap());
}

/// Aggregate-size cap: a deep tree with many ancestors and big files
/// stays within the shared discovery budget plus block overhead.
#[test]
fn enforces_total_context_cap() {
    let outer = tempdir().unwrap();
    anchor(outer.path());
    // 7 ancestor dirs each with a 32 KB CLAUDE.md — total raw input
    // would be ~224 KB, well above the 128 KB cap.
    let mut cur = outer.path().to_path_buf();
    for i in 0..7 {
        cur = cur.join(format!("lvl{i}"));
        std::fs::create_dir_all(&cur).unwrap();
        std::fs::write(cur.join("CLAUDE.md"), "x".repeat(40_000)).unwrap();
    }
    let blocks = collect_project_context_blocks(&cur);
    // Shared discovery budget is 32k chars; allow header + block overhead.
    let allowed = 64 * 1024;
    assert!(
        blocks[0].len() <= allowed,
        "context body {} exceeds allowed budget {}",
        blocks[0].len(),
        allowed
    );
}

/// Write a minimal valid `<dir>/SKILL.md` with the given name/description.
fn write_skill(dir: &std::path::Path, name: &str, description: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
    )
    .unwrap();
}

#[test]
fn project_skill_block_lists_project_skills() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    anchor(project);
    write_skill(
        &project.join(".aleph").join("skills").join("refine-text"),
        "Refine Text",
        "Polish prose without changing meaning",
    );
    write_skill(
        &project.join(".claude").join("skills").join("translate"),
        "Translate",
        "Translate text to another language",
    );

    let block = collect_project_skill_block(project).expect("project skills present");
    assert!(block.contains("`refine-text` — Refine Text:"));
    assert!(block.contains("`translate` — Translate:"));
    assert!(block.contains("skill_read"));
}

#[test]
fn project_skill_block_none_when_no_project_skills() {
    let tmp = tempdir().unwrap();
    anchor(tmp.path());
    assert!(collect_project_skill_block(tmp.path()).is_none());
}

#[test]
fn project_skill_block_skips_dirs_without_manifest() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    anchor(project);
    // A subdir with no SKILL.md must not appear.
    std::fs::create_dir_all(project.join(".aleph").join("skills").join("empty-dir")).unwrap();
    write_skill(
        &project.join(".aleph").join("skills").join("real"),
        "Real Skill",
        "A genuine skill",
    );
    let block = collect_project_skill_block(project).expect("one real skill");
    assert!(block.contains("`real` — Real Skill:"));
    assert!(!block.contains("empty-dir"));
}

// ── model_supports_vision (attachment injection gate) ────────────────────────

#[test]
fn vision_model_takes_the_image_natively() {
    assert!(model_supports_vision("claude-fable-5", None));
    assert!(model_supports_vision("grok-4-fast", None));
}

#[test]
fn text_only_model_is_degraded_not_sent_an_image() {
    // The regression this gate exists for: these catalogue entries declare
    // `supports_vision: false`, and used to receive ContentBlock::Image anyway.
    assert!(!model_supports_vision("deepseek-chat", None));
    assert!(!model_supports_vision("minimax-m2.5", None));
}

#[test]
fn serving_hint_answers_when_the_configured_id_is_unknown() {
    // Agent model unset / custom alias → the live provider chain names the
    // model it would actually serve, and that verdict is used.
    assert!(!model_supports_vision("", Some("deepseek-chat")));
    assert!(model_supports_vision("", Some("claude-fable-5")));
    // A catalogued configured id wins; the hint is not consulted.
    assert!(model_supports_vision("claude-fable-5", Some("deepseek-chat")));
}

#[test]
fn unknown_capabilities_fail_open() {
    // Neither id is in the catalogue (custom endpoint, local proxy): keep
    // sending the image. A loud provider error beats silently blinding a model
    // that could have seen it — see `model_supports_vision`'s doc comment.
    assert!(model_supports_vision("my-local-proxy/v1", None));
    assert!(model_supports_vision("", Some("some-unlisted-model")));
    assert!(model_supports_vision("", None));
}
