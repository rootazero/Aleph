//! Phase 2 — skill_status must report a non-empty registry when skills exist.

use alephcore::builtin_tools::skill_status::{SkillStatusArgs, SkillStatusTool};
use alephcore::tools::AlephTool;

#[tokio::test]
async fn skill_status_reports_skills_from_temp_dir() {
    // Build a temp skills dir with one SKILL.md, init a SkillSystem on it,
    // and assert skill_status sees total >= 1.
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: A demo skill\n---\nBody.",
    )
    .unwrap();

    let system = alephcore::skill::SkillSystem::new();
    // `init` returns `()`, not a `Result` — it logs and skips unreadable dirs
    // rather than failing. This line said `.unwrap()` and so this whole test
    // binary has not compiled: a broken integration target stops the whole
    // `tests/*` build, so none of the crate's integration tests ran. Neither
    // `cargo check` nor `cargo test --lib` noticed, because neither builds
    // `tests/`.
    system.init(vec![tmp.path().to_path_buf()]).await;

    let tool = SkillStatusTool::new(system);
    let out = tool
        .call(SkillStatusArgs {
            filter: "all".to_string(),
        })
        .await
        .unwrap();
    assert!(out.total >= 1, "expected >=1 skill, got {}", out.total);
}
