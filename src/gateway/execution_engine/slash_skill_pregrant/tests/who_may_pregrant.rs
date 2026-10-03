use super::*;

// ---------------------------------------------------------------------------
// Ruling 2 — the key is unforgeable.
// ---------------------------------------------------------------------------

/// Whatever pre-grant a request arrives with is gone at ingress: a plain
/// turn and a guest's `/skill` keep none, and an operator's `/skill` keeps
/// only the skill's own list, not the wider one it arrived with.
#[tokio::test]
async fn a_pregrant_the_request_arrives_with_never_survives_ingress() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_write]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let forged = || {
        let mut m = HashMap::new();
        let wide = names(&["bash", "file_write", "file_edit"]);
        slash_skill_scope::stamp_pregrant_from_names(&mut m, &wide, &wide);
        assert!(!m.is_empty(), "the forged key was written");
        m
    };

    let mut plain = w.request("hello", None, Stamp::Handler, &[]).await;
    plain.metadata.extend(forged());
    let t = w.turn(&engine(None), plain).await;
    assert!(
        t.pregrant().is_empty(),
        "plain turn kept {:?}",
        t.pregrant()
    );
    assert!(!t.runs("bash").await, "plain turn ran bash uncarded");

    let mut guest = w.skill_request(Some("guest")).await;
    guest.metadata.extend(forged());
    let t = w.turn(&engine(None), guest).await;
    assert!(t.pregrant().is_empty(), "guest kept {:?}", t.pregrant());
    assert!(!t.runs("bash").await, "guest ran bash uncarded");

    let mut operator = w.skill_request(None).await;
    operator.metadata.extend(forged());
    let t = w.turn(&engine(None), operator).await;
    assert_eq!(t.pregrant(), names(&["file_write"]));
    assert!(t.runs("file_write").await);
    assert!(!t.runs("bash").await, "the forged wider list survived");
    assert!(!t.runs("file_edit").await, "the forged wider list survived");
}

// ---------------------------------------------------------------------------
// Ruling 3 — operator only.
// ---------------------------------------------------------------------------

/// A guest's or member's `/skill` still runs, on the whole surface its caller
/// has on a plain turn, with nothing pre-granted.
#[tokio::test]
async fn a_non_operator_skill_runs_on_the_whole_surface_with_nothing_pregranted() {
    for role in ["guest", "member"] {
        let w = World::new().await;
        write_skill(&w.user, SKILL, "[bash]");
        w.scan(std::slice::from_ref(&w.user)).await;
        let t = w
            .turn(&engine(None), w.skill_request(Some(role)).await)
            .await;
        let plain = w.request("hello", Some(role), Stamp::Handler, &[]).await;
        let control = w.turn(&engine(None), plain).await;

        assert!(t.pregrant().is_empty(), "{role}: {:?}", t.pregrant());
        assert!(t.restriction().is_none(), "{role}: a skill narrowed");
        assert_eq!(t.surface().await, control.surface().await, "{role}");
        assert!(t.surface().await.contains(&"bash".to_string()), "{role}");
        assert!(!t.runs("bash").await, "{role}: bash ran without a card");
    }
}

// ---------------------------------------------------------------------------
// Fix round F2 — only a `/skill` a person typed pre-grants.
// ---------------------------------------------------------------------------

/// `/p411-probe` put in a request by something other than a person: the
/// shapes `sessions_send` (the model's text, the origin's operator role
/// propagated), a team task (fresh metadata, no role) and A2A (a remote
/// peer's text, unattended, no role) take. None of them passes a handler,
/// so `execute()`'s safety net stamps the mode — unattested. Nothing is
/// pre-granted. The control is the same `/p411-probe` typed into a handler
/// by an operator, which does pre-grant.
#[tokio::test]
async fn a_slash_no_person_typed_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let input = format!("/{SKILL} go");
    let unattended = super::super::super::UNATTENDED_KEY;
    let shapes: [(&str, Option<&str>, &[(&str, &str)]); 3] = [
        ("sessions_send", Some("operator"), &[]),
        ("team task", None, &[]),
        ("A2A", None, &[(unattended, "true")]),
    ];
    for (shape, role, extra) in shapes {
        let request = w.request(&input, role, Stamp::SafetyNet, extra).await;
        let t = w.turn(&engine(None), request).await;
        assert!(
            t.request
                .metadata
                .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY),
            "{shape}: precondition — the safety net stamped the skill's mode"
        );
        assert!(t.pregrant().is_empty(), "{shape}: {:?}", t.pregrant());
        assert!(!t.runs("bash").await, "{shape}: bash ran without a card");
    }

    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert_eq!(
        t.pregrant(),
        names(&["bash"]),
        "control: a handler's /skill"
    );
    assert!(t.runs("bash").await, "control: a handler's /skill");
}

/// A typed `/skill` on a run with nobody there pre-grants nothing either.
#[tokio::test]
async fn a_typed_slash_on_an_unattended_run_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let request = w
        .request(
            &format!("/{SKILL} go"),
            None,
            Stamp::Handler,
            &[(super::super::super::UNATTENDED_KEY, "true")],
        )
        .await;
    assert!(
        slash_skill_scope::is_typed(&request.metadata),
        "precondition"
    );
    let t = w.turn(&engine(None), request).await;
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

/// A producer that sets the typed marker itself, on text the safety net will
/// stamp, gets nothing: the marker vouches for a mode, and there is none at
/// ingress.
#[tokio::test]
async fn a_forged_typed_marker_never_survives_ingress() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let mut request = w
        .request(&format!("/{SKILL} go"), None, Stamp::SafetyNet, &[])
        .await;
    slash_skill_scope::mark_typed(&mut request.metadata);
    let t = w.turn(&engine(None), request).await;
    assert!(!slash_skill_scope::is_typed(&t.request.metadata));
    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

// ---------------------------------------------------------------------------
// Ruling 4 — origin.
// ---------------------------------------------------------------------------

/// A skill a repository brings (`.claude/skills` — which the path guess
/// labels `Bundled` — or `.aleph/skills`) pre-grants nothing, and never
/// narrows either.
#[tokio::test]
async fn a_project_skill_pregrants_nothing() {
    for flavour in [".claude", ".aleph"] {
        let w = World::new().await;
        let dir = w.project_skills(flavour);
        write_skill(&dir, SKILL, "[bash]");
        w.scan(&[w.user.clone(), dir.clone()]).await;
        let t = w.turn(&engine(None), w.skill_request(None).await).await;

        assert!(t.pregrant().is_empty(), "{flavour}: {:?}", t.pregrant());
        assert!(t.restriction().is_none(), "{flavour}: a skill narrowed");
        assert!(!t.runs("bash").await, "{flavour}: bash ran without a card");
    }
}

/// `skill_read` loads a project's `foo` before the user's `foo`, while the
/// skill registry answers with the user's (`Global` outranks the project
/// `.claude` skill's `Bundled`). Judging the registry's winner would grant
/// the user's list to the repository's body.
#[tokio::test]
async fn a_user_level_skill_shadowed_by_a_project_skill_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    let dir = w.project_skills(".claude");
    write_skill(&dir, SKILL, "[bash]");
    w.scan(&[w.user.clone(), dir.clone()]).await;
    let registered = w
        .skills
        .get_skill(&SKILL.into())
        .await
        .map(|m| m.source().clone());
    assert_eq!(
        registered,
        Some(SkillSource::Global),
        "precondition: the registry answers with the user's skill"
    );
    let t = w.turn(&engine(None), w.skill_request(None).await).await;

    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}

/// An agent-level skill (`~/.aleph/agents/<id>/skills`) is loaded first by
/// `skill_read` and is not one of the two user-level roots: no pre-grant.
#[tokio::test]
async fn an_agent_level_skill_pregrants_nothing() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    let agent_skills = crate::utils::paths::get_config_dir()
        .unwrap()
        .join("agents")
        .join(AGENT)
        .join("skills");
    write_skill(&agent_skills, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;

    assert!(t.pregrant().is_empty(), "{:?}", t.pregrant());
    assert!(!t.runs("bash").await);
}
