use super::*;

// ---------------------------------------------------------------------------
// Ruling 1 — the kind comes from the registration.
// ---------------------------------------------------------------------------

/// A plugin COMMAND reaches `execute.rs` as the same `type: "skill"` mode a
/// skill does. Its kind comes from the registration, never from whether it
/// would be admitted: this command's plugin is in no registry at all (the
/// shape a disabled, hidden or orphaned plugin's command takes at the split,
/// which reads no admission state), and a user-level SKILL with the
/// command's bare name exists. The command keeps its RESTRICTION and is
/// never pre-granted — typed by an operator into a handler, as here.
#[tokio::test]
async fn a_plugin_command_restricts_and_never_pregrants_whatever_its_admission() {
    let w = World::new().await;
    write_skill(&w.user, "greet", "[bash]");
    w.scan(std::slice::from_ref(&w.user)).await;
    w.register_command("greet", &["bash"]).await;
    let request = w
        .request(&format!("/{PLUGIN}:greet hi"), None, Stamp::Handler, &[])
        .await;
    assert!(
        slash_skill_scope::is_typed(&request.metadata),
        "precondition"
    );
    let t = w.turn(&engine(None), request).await;

    assert!(
        t.pregrant().is_empty(),
        "a command pre-granted {:?}",
        t.pregrant()
    );
    assert_eq!(
        t.restriction(),
        Some(BTreeSet::from(["bash".to_string()])),
        "a command's allowed-tools must still restrict"
    );
    assert_eq!(t.surface().await, names(&["bash"]));
    assert!(
        !t.runs("bash").await,
        "restricted to bash, and bash still asks"
    );
    assert!(!t.runs("file_write").await, "narrowed away");
}
