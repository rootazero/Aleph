//! Phase 3 — an installed markdown CLI skill must surface as a loop tool.
//!
//! Mirrors the production path: a skill installed into the process-wide
//! markdown-skill server (via the `skills.install` RPC handler) must become
//! visible to the agent loop through `MarkdownSkillRefreshSource`.
//!
//! Uses a multi-thread runtime because `MarkdownSkillRefreshSource::fetch_tools`
//! bridges async→sync via `tokio::task::block_in_place`, which is only sound
//! on a multi-threaded runtime (the same flavour the `aleph-server` binary
//! uses).

use alephcore::gateway::execution_engine::markdown_skill_refresh::MarkdownSkillRefreshSource;
use alephcore::gateway::handlers::markdown_skills::{
    bump_markdown_skills_revision, markdown_skills_server,
};
use alephcore::tools::markdown_skill::{AlephSkillSpec, MarkdownCliTool};
use alephcore::tools::refresh::ToolRefreshSource;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_markdown_skill_appears_in_refresh_source() {
    let spec = AlephSkillSpec {
        name: "echo_demo".to_string(),
        description: "Echo demo skill".to_string(),
        metadata: Default::default(),
        markdown_content: "## Examples\nDemo".to_string(),
    };
    let tool = MarkdownCliTool::new(spec);
    markdown_skills_server().replace_tool(tool).await;
    bump_markdown_skills_revision();

    // `new()` captures the current revision as its baseline; bump once more so
    // the source observes a change on its first poll regardless of what other
    // tests in this binary have done to the shared counter.
    let src = MarkdownSkillRefreshSource::new();
    bump_markdown_skills_revision();
    assert!(src.poll_changes(), "refresh source should detect the bump");

    let tools = src.fetch_tools();
    assert!(
        tools.iter().any(|t| t.name() == "echo_demo"),
        "installed markdown skill must surface as a loop tool"
    );
}
