use super::*;

// ---------------------------------------------------------------------------
// Fix round F4 — children.
// ---------------------------------------------------------------------------

/// A subagent this turn spawns runs under `for_children()` — the policy
/// without the pre-grant — so it is carded for the tool its parent turn ran
/// uncarded.
#[tokio::test]
async fn a_child_is_carded_for_what_its_parent_turn_pregranted() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert!(
        t.runs("bash").await,
        "precondition: the parent turn pre-granted bash"
    );

    let child = gate(
        &t.request,
        &t.permissions,
        t.permissions
            .explicit
            .as_ref()
            .and_then(TurnToolPolicy::for_children),
        stubs,
    );
    assert!(
        !runs_with(&child, "bash", json!({})).await,
        "a child ran its parent's pre-granted bash without a card"
    );
}

// ---------------------------------------------------------------------------
// Rulings 6 and 7 — the lift's scope, one derivation.
// ---------------------------------------------------------------------------

/// A skill declaring the four tools the rulings below speak about.
async fn declaring_world() -> World {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash, file_write, file_edit, self_config]");
    w.scan(std::slice::from_ref(&w.user)).await;
    w
}

/// The pre-grant lifts the tier's `Ask` and nothing else: an install's
/// explicit deny and glob win over it; a name nothing binds is lifted.
#[tokio::test]
async fn an_explicit_entry_outranks_the_pregrant() {
    let w = declaring_world().await;
    let t = w
        .turn(
            &engine(Some(
                r#"{"default":"allow","overrides":{"bash":"deny","file_e*":"ask"}}"#,
            )),
            w.skill_request(None).await,
        )
        .await;
    assert!(
        !t.runs("bash").await,
        "an explicit deny lost to the pre-grant"
    );
    assert!(!t.runs("file_edit").await, "a glob lost to the pre-grant");
    assert!(t.runs("file_write").await, "nothing bound file_write");
}

/// A policy whose `default` is `deny` takes no pre-grant: an exact `allow`
/// would outrank the operator's default, which is not the tier.
#[tokio::test]
async fn a_deny_default_takes_no_pregrant() {
    let w = declaring_world().await;
    let t = w
        .turn(
            &engine(Some(r#"{"default":"deny","overrides":{}}"#)),
            w.skill_request(None).await,
        )
        .await;
    assert!(
        !t.runs("file_write").await,
        "a deny default lost to the pre-grant"
    );
}

/// The floors hold under a live pre-grant: the gate-removal floor still
/// cards a `self_config` write the skill listed, and `plan` still refuses.
#[tokio::test]
async fn the_floors_hold_under_a_pregrant() {
    let w = declaring_world().await;
    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert!(t.runs("file_write").await, "control: the pre-grant is live");
    let gate_write = json!({ "action": "update_config", "config_path": "policies.exec_tier" });
    assert!(
        !t.runs_with("self_config", gate_write).await,
        "the gate-removal floor stood down for a skill's list"
    );

    let t = w
        .turn(
            &engine(None),
            at(ExecTier::Plan, w.skill_request(None).await),
        )
        .await;
    assert_eq!(t.permissions.tier, ExecTier::Plan);
    assert!(
        !t.runs("file_write").await,
        "plan's floor lost to the pre-grant"
    );
}

/// A pre-grant lifts the NAME-level `Ask` only. Its entry is not a decision
/// a person wrote, so the tool gate must not read it as one: the
/// argument-level cards stay. A pre-granted `file_ops` lists without a card
/// and still cards a destructive `delete` — under `auto`, and under `ask`,
/// where the name-level `Ask` the pre-grant lifted was what covered it. The
/// control is the same call under an operator's own exact `allow`, which is a
/// person's decision and does stand the card down.
#[tokio::test]
async fn a_pregranted_tool_keeps_its_argument_cards() {
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[file_ops]");
    w.scan(std::slice::from_ref(&w.user)).await;
    let list = json!({ "operation": "list", "path": "." });
    let delete = json!({ "operation": "delete", "path": "gone.txt" });

    for tier in [ExecTier::Auto, ExecTier::Ask] {
        let t = w
            .turn(&engine(None), at(tier, w.skill_request(None).await))
            .await;
        assert_eq!(t.pregrant(), names(&["file_ops"]), "{tier:?}");
        assert!(
            t.runs_with("file_ops", list.clone()).await,
            "{tier:?}: the name-level lift is gone"
        );
        assert!(
            !t.runs_with("file_ops", delete.clone()).await,
            "{tier:?}: a pre-granted destructive file_ops ran without its argument card"
        );
    }

    let t = w
        .turn(
            &engine(Some(
                r#"{"default":"allow","overrides":{"file_ops":"allow"}}"#,
            )),
            at(ExecTier::Auto, w.skill_request(None).await),
        )
        .await;
    assert!(
        t.runs_with("file_ops", delete).await,
        "control: an operator's own exact allow stands the argument card down"
    );
}

/// The one merge: the value a plugin command's inline shell reads
/// (`builtin_permission`) and the tool gate agree on a pre-granted `bash`,
/// and an explicit deny still reads `Deny` on both — the inline face
/// withholds on `Deny` only, so a pre-grant never changes it.
#[tokio::test]
async fn the_inline_face_and_the_tool_gate_read_one_pregrant() {
    use crate::extension::PermissionAction;
    let w = World::new().await;
    write_skill(&w.user, SKILL, "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;

    let t = w.turn(&engine(None), w.skill_request(None).await).await;
    assert_eq!(
        t.permissions.builtin_permission("bash"),
        PermissionAction::Allow
    );
    assert!(t.runs("bash").await);

    let t = w
        .turn(
            &engine(Some(r#"{"default":"allow","overrides":{"bash":"deny"}}"#)),
            w.skill_request(None).await,
        )
        .await;
    assert_eq!(
        t.permissions.builtin_permission("bash"),
        PermissionAction::Deny
    );
    assert!(!t.runs("bash").await);
}
