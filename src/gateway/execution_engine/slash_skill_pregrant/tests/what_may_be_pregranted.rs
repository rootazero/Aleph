use super::*;

// ---------------------------------------------------------------------------
// Fix round F1 + F3 — what may be pre-granted.
// ---------------------------------------------------------------------------

/// The file is re-read at turn start; registration validated an earlier
/// version of it (`[file_write]`). An edit since — a new tool, a glob that
/// the policy would read back as "every tool" — grants nothing it added.
#[tokio::test]
async fn a_file_widened_after_registration_pregrants_only_the_registered_names() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let request = w.skill_request(None).await;
    write_skill(&w.user, SKILL, r#"[file_write, bash, "?*", "file_*"]"#);
    let t = w.turn(&engine(None), request).await;

    assert_eq!(t.pregrant(), names(&["file_write"]));
    assert!(t.runs("file_write").await, "the registered name is granted");
    assert!(!t.runs("bash").await, "a name added after registration ran");
    assert!(
        !t.runs("file_edit").await,
        "a glob added after registration ran"
    );
    let gate_write = json!({ "action": "update_config", "config_path": "policies.exec_tier" });
    assert!(!t.runs_with("self_config", gate_write).await);
}

/// `skill_manage` — the model's authoring tool, writing into the
/// operator-owned `~/.aleph/skills` the pre-grant trusts — may keep or narrow
/// a skill's `allowed-tools`, never add to it: `edit` and `patch` that widen
/// are refused under every tier (here `full`, which cards nothing), and under
/// `plan` the tool does not run at all. Through the real tool gate and the
/// real tool.
#[tokio::test]
async fn skill_manage_never_adds_to_a_skills_grant() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let svc = w.skill_manage_gate(ExecTier::Full).await;
    let file = w.user.join(SKILL).join("SKILL.md");

    let widen_edit = json!({
        "action": "edit", "skill_id": SKILL, "content": skill_md(SKILL, "[file_write, bash]")
    });
    assert!(
        !runs_with(&svc, "skill_manage", widen_edit).await,
        "an edit added bash to the grant"
    );
    let widen_patch = json!({
        "action": "patch", "skill_id": SKILL,
        "find": "allowed-tools: [file_write]", "replace": "allowed-tools: [file_write, bash]"
    });
    assert!(
        !runs_with(&svc, "skill_manage", widen_patch).await,
        "a patch added bash to the grant"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        skill_md(SKILL, "[file_write]"),
        "a refused write touched the file"
    );
    let keep = json!({
        "action": "patch", "skill_id": SKILL, "find": "Do the thing.", "replace": "Do it well."
    });
    assert!(
        runs_with(&svc, "skill_manage", keep).await,
        "a body edit that keeps the grant was refused"
    );
    let narrow = json!({
        "action": "edit", "skill_id": SKILL, "content": skill_md(SKILL, "[]")
    });
    assert!(
        runs_with(&svc, "skill_manage", narrow).await,
        "a narrowing edit was refused"
    );

    let svc = w.skill_manage_gate(ExecTier::Plan).await;
    let keep = json!({
        "action": "patch", "skill_id": SKILL, "find": "Do it well.", "replace": "Do it."
    });
    assert!(
        !runs_with(&svc, "skill_manage", keep).await,
        "skill_manage wrote under plan"
    );
}

// ---------------------------------------------------------------------------
// Ruling 5 — a model-initiated load never pre-grants.
// ---------------------------------------------------------------------------

/// The model loading a skill itself (`skill_read`) is a turn with no slash
/// mode: nothing is pre-granted however much the skill declares, and a real
/// `skill_read` of it leaves the turn's facts where they were. A command
/// restricted to `skill_read` stays restricted to it.
#[tokio::test]
async fn a_model_initiated_skill_load_grants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;

    let plain = w
        .request(&format!("use the {SKILL} skill"), None, Stamp::Handler, &[])
        .await;
    let t = w.turn(&engine(None), plain).await;
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);

    w.register_command("reader", &["skill_read"]).await;
    let request = w
        .request(&format!("/{PLUGIN}:reader"), None, Stamp::Handler, &[])
        .await;
    let t = w.turn(&engine(None), request).await;
    let before = (t.restriction(), t.pregrant());
    let body =
        crate::builtin_tools::skill_reader::ReadSkillTool::with_auto_discover(Some(&w.project))
            .call_json(json!({ "skill_id": SKILL }))
            .await
            .expect("the model's skill_read loads the skill");
    assert!(body.to_string().contains("Do the thing."), "{body}");
    assert_eq!((t.restriction(), t.pregrant()), before);
    assert_eq!(before.0, Some(BTreeSet::from(["skill_read".to_string()])));
    assert!(before.1.is_empty());
}

// ---------------------------------------------------------------------------
// Fix round 2, N1 — a directory that may pre-grant is not the model's to write.
// ---------------------------------------------------------------------------

/// The model's file tools refuse every file a pre-grant is read from — the
/// user roots and a `Global` plugin's skills, new or existing — so a model
/// cannot add a name that registration then validates at the next boot.
/// Control: the same tools write in the project.
#[tokio::test]
async fn the_file_tools_never_write_where_a_skill_pregrants_from() {
    use crate::builtin_tools::file_ops::{FileEditTool, FileWriteTool};
    use crate::tools::AlephTool;

    let w = World::new().await;
    let cache_skills = w
        .tmp
        .path()
        .join("home/.claude/plugins/cache/p411-mkt")
        .join(PLUGIN)
        .join("1.0.0/skills");
    let aleph_plugin_skills = w
        .tmp
        .path()
        .join("aleph/plugins")
        .join(PLUGIN)
        .join("skills");
    let roots = [
        w.user.clone(),
        w.claude_user.clone(),
        cache_skills,
        aleph_plugin_skills,
    ];
    let write = FileWriteTool::new();
    let edit = FileEditTool::new();
    let widened = skill_md(SKILL, "[grep, bash]");
    for root in &roots {
        write_skill(root, SKILL, "[grep]");
        let existing = root.join(SKILL).join("SKILL.md");
        let fresh = root.join("p411-fresh").join("SKILL.md");
        for file in [&existing, &fresh] {
            let wrote = write
                .call_json(json!({ "file_path": file, "content": widened }))
                .await;
            assert!(
                wrote.is_err(),
                "file_write wrote {}: {wrote:?}",
                file.display()
            );
        }
        let edited = edit
            .call_json(json!({
                "file_path": existing,
                "old_string": "[grep]",
                "new_string": "[grep, bash]",
            }))
            .await;
        assert!(
            edited.is_err(),
            "file_edit edited {}: {edited:?}",
            existing.display()
        );
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            skill_md(SKILL, "[grep]"),
            "{}",
            existing.display()
        );
        assert!(!fresh.exists(), "{}", fresh.display());
    }

    let control = w.project.join("notes.md");
    write
        .call_json(json!({ "file_path": control, "content": "ok" }))
        .await
        .expect("control: file_write works in the project");
}

/// Every directory a `/<skill>` may pre-grant from is in the file tools'
/// denylist — the four this world has, named by hand, so the list cannot
/// shrink unnoticed.
#[tokio::test]
async fn every_pregrant_root_is_denied_to_the_file_tools() {
    use crate::builtin_tools::file_ops::{get_denied_paths, path_is_denied};

    let w = World::new().await;
    let roots = crate::utils::paths::pregrant_roots();
    let expected = [
        w.user.clone(),
        w.claude_user.clone(),
        w.tmp.path().join("aleph").join("plugins"),
        w.tmp.path().join("home").join(".claude").join("plugins"),
    ];
    for root in &expected {
        assert!(
            roots.iter().any(|r| r == root),
            "{} is not a pre-grant root: {roots:?}",
            root.display()
        );
    }
    let denied = get_denied_paths();
    for root in &roots {
        let inside = root.join("p411-x");
        std::fs::create_dir_all(&inside).unwrap();
        let canonical = inside.canonicalize().unwrap();
        assert!(
            path_is_denied(&canonical, &denied),
            "{} may pre-grant but the file tools may write it",
            root.display()
        );
    }
}

/// The denylist binds the model's file tools, not skill loading: `skill_read`
/// still loads a skill from `~/.claude/skills`.
#[tokio::test]
async fn skill_read_still_loads_from_the_claude_user_root() {
    let w = World::new().await;
    write_skill(&w.claude_user, SKILL, "[grep]");
    let body =
        crate::builtin_tools::skill_reader::ReadSkillTool::with_auto_discover(Some(&w.project))
            .call_json(json!({ "skill_id": SKILL }))
            .await
            .expect("skill_read loads a ~/.claude/skills skill");
    assert!(body.to_string().contains("Do the thing."), "{body}");
}
