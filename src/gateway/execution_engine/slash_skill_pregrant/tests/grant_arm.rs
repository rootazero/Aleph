use super::*;

// ---------------------------------------------------------------------------
// Rulings 3 and 4, the arm that grants.
// ---------------------------------------------------------------------------

/// An operator's `/skill` from either user-level root pre-grants exactly its
/// own list: the listed tools run without a card, an unlisted mutating tool
/// still cards, and the surface stays whole — a skill never narrows.
#[tokio::test]
async fn a_user_level_skill_pregrants_its_own_list_for_an_operator() {
    for (root, role) in [
        ("aleph", None),
        ("aleph", Some("operator")),
        ("claude", None),
    ] {
        let w = World::new().await;
        let dir = if root == "aleph" {
            w.user.clone()
        } else {
            w.claude_user.clone()
        };
        write_skill(&dir, SKILL, "[bash, file_write]");
        w.scan(&[w.user.clone(), w.claude_user.clone()]).await;
        let t = w.turn(&engine(None), w.skill_request(role).await).await;

        let case = format!("{root} / {role:?}");
        assert_eq!(t.pregrant(), names(&["bash", "file_write"]), "{case}");
        assert!(t.restriction().is_none(), "{case}: a skill narrowed");
        assert!(t.runs("bash").await, "{case}: bash still asks");
        assert!(t.runs("file_write").await, "{case}: file_write still asks");
        assert!(
            !t.runs("file_edit").await,
            "{case}: an unlisted mutating tool ran without a card"
        );
        let mut whole = names(&TOOLS);
        whole.sort();
        assert_eq!(t.surface().await, whole, "{case}");
    }
}

/// A Claude Code skill (P4.12): its `allowed-tools` names Claude Code tools.
/// It registers — its catalog row carries the names registration could
/// validate, in Aleph spelling — and an operator's typed `/skill` pre-grants
/// exactly those: the file's `Read` meets the registration's `file_read`
/// because both sides are mapped by one function; the scoped `Bash(...)` and
/// the tool with no Aleph counterpart grant nothing.
#[tokio::test]
async fn a_claude_code_skill_pregrants_its_mapped_names() {
    let w = World::new().await;
    write_skill(
        &w.claude_user,
        SKILL,
        "Read, Grep, Bash(git status:*), NotebookEdit",
    );
    w.scan(std::slice::from_ref(&w.claude_user)).await;
    let row = w
        .catalog
        .list_all()
        .await
        .into_iter()
        .find(|t| t.name == SKILL)
        .expect("a Claude Code skill registers");
    assert_eq!(
        row.routing_capabilities,
        Some(names(&["file_read", "grep"]))
    );

    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert_eq!(t.pregrant(), names(&["file_read", "grep"]));
    assert!(t.restriction().is_none(), "a skill narrowed");
    assert!(
        !t.runs("bash").await,
        "the scoped Bash(...) granted all of bash"
    );
}

/// The gate over stubs named like the two real tools no catalog row names
/// (`executor::TOOLS_OUTSIDE_DEFINITIONS`) plus `bash`, each held only when
/// the turn's restriction admits it.
fn outside_stubs(registry: &mut LoopToolRegistry, admits: &dyn Fn(&str) -> bool) {
    for name in ["subagent", "tool_search", "bash"] {
        if admits(name) {
            registry.register(Box::new(Stub(name)));
        }
    }
}

/// `Task` / `Agent` map to `subagent`, `ToolSearch` to `tool_search`: real
/// tools, though no catalog row names them when skills register. An
/// operator's typed `/skill` pre-grants both, and `subagent` — which the
/// `ask` tier cards by that name (it is not idempotent) — runs uncarded. The
/// control skill that does not declare it leaves `subagent` carded.
#[tokio::test]
async fn a_claude_code_skill_pregrants_subagent_and_it_runs_uncarded() {
    for (allowed, granted) in [("Task, ToolSearch", true), ("[grep]", false)] {
        let w = World::new().await;
        write_skill(&w.user, SKILL, allowed);
        w.scan(std::slice::from_ref(&w.user)).await;
        let t = w.turn(&engine(None), w.skill_request(None).await).await;
        let gate = gate(
            &t.request,
            &t.permissions,
            t.permissions.explicit.clone(),
            outside_stubs,
        );
        if granted {
            assert_eq!(t.pregrant(), names(&["subagent", "tool_search"]));
        }
        assert_eq!(
            runs_with(&gate, "subagent", json!({})).await,
            granted,
            "{allowed}: subagent ran uncarded = {}",
            !granted
        );
        assert!(!runs_with(&gate, "bash", json!({})).await, "{allowed}");
    }
}

/// A plugin command naming `subagent` (Claude Code's `Task`) registers and
/// restricts its turn to exactly that name.
#[tokio::test]
async fn a_command_restricted_to_subagent_lists_only_subagent() {
    let w = World::new().await;
    w.register_command("delegate", &["subagent"]).await;
    let request = w
        .request(&format!("/{PLUGIN}:delegate go"), None, Stamp::Handler, &[])
        .await;
    let t = w.turn(&engine(None), request).await;
    assert_eq!(
        t.restriction(),
        Some(BTreeSet::from(["subagent".to_string()]))
    );
    assert!(t.pregrant().is_empty());
    let gate = gate(
        &t.request,
        &t.permissions,
        t.permissions.explicit.clone(),
        outside_stubs,
    );
    let mut listed: Vec<String> = gate.list().await.into_iter().map(|d| d.name).collect();
    listed.sort();
    assert_eq!(listed, names(&["subagent"]));
}

/// A global plugin's skill pre-grants from either `Global` plugin parent —
/// Aleph's `<config>/plugins` or Claude Code's `~/.claude/plugins` cache.
/// The same skill does not under a project-scoped plugin, nor under a
/// `Global` key whose dir lies outside both parents: a directory the model's
/// file tools can write never pre-grants (fix round 2, N1).
#[tokio::test]
async fn a_plugin_skill_pregrants_only_from_a_global_plugin() {
    for case in ["aleph", "claude cache", "global outside", "project"] {
        let w = World::new().await;
        let plugin_skills = match case {
            "aleph" => w.tmp.path().join("aleph/plugins").join(PLUGIN),
            "claude cache" => w
                .tmp
                .path()
                .join("home/.claude/plugins/cache/p411-mkt")
                .join(PLUGIN)
                .join("1.0.0"),
            "global outside" => w.tmp.path().join("plug"),
            _ => w.project.join(".aleph/plugins").join(PLUGIN),
        }
        .join("skills");
        write_skill(&plugin_skills, SKILL, "[bash]");
        let scope_key = if case == "project" {
            ScopeKey::project(&w.project)
        } else {
            ScopeKey::Global
        };
        publish_plugin_skill_dirs(vec![PublishedPluginSkillDir {
            dir: plugin_skills.clone(),
            plugin_id: PLUGIN.into(),
            scope_key,
            plugin_root: plugin_skills.parent().unwrap().to_path_buf(),
        }]);
        w.scan(std::slice::from_ref(&w.user)).await;
        let source = w
            .skills
            .get_skill(&SKILL.into())
            .await
            .map(|m| m.source().clone());
        let t = w.turn(&engine(None), w.skill_request(None).await).await;
        let bash_runs = t.runs("bash").await;
        publish_plugin_skill_dirs(Vec::new());

        assert_eq!(
            source,
            Some(SkillSource::Plugin(PluginId::new(PLUGIN))),
            "{case}"
        );
        if matches!(case, "aleph" | "claude cache") {
            assert_eq!(t.pregrant(), names(&["bash"]), "{case}");
            assert!(
                bash_runs,
                "{case}: a global plugin's skill did not pre-grant"
            );
        } else {
            assert!(t.pregrant().is_empty(), "{case}: {:?}", t.pregrant());
            assert!(!bash_runs, "{case}: the skill pre-granted");
        }
    }
}
