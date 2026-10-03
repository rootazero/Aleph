use super::*;

// ---------------------------------------------------------------------------
// Fire sites.
// ---------------------------------------------------------------------------

/// Who may spell each key and call each function of the wire, across every
/// production file under `src/`: the pre-grant is written only by the split
/// and read only by the resolution; the typed marker is written only by the
/// two human-facing stamps; the ingress strip runs only in `execute()`. So
/// neither `skill_read`, a resume, a queue replay nor any other producer can
/// put either on a turn.
#[test]
fn the_wire_has_one_writer_and_one_reader_per_fact() {
    use crate::utils::source_scan::{code_keeping_literals, production_text, rust_sources_under};
    const WIRE: &str = "src/gateway/execution_engine/slash_skill_scope.rs";
    const SPLIT: &str = "src/gateway/execution_engine/slash_skill_pregrant/mod.rs";
    const RESOLVE: &str = "src/gateway/execution_engine/turn_permissions.rs";
    const EXECUTE: &str = "src/gateway/execution_engine/execute.rs";
    const STAMP: &str = "src/gateway/execution_engine/slash_command.rs";
    const ROUTER: &str = "src/gateway/inbound_router/executor.rs";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 500,
        "the walk found only {} files",
        sources.len()
    );
    let pregrant_key = format!("\"{}\"", slash_skill_scope::SLASH_SKILL_PREGRANT_TOOLS_KEY);
    let typed_key = format!("\"{}\"", slash_skill_scope::SLASH_MODE_TYPED_KEY);
    let owners: [(&str, &[&str]); 8] = [
        (pregrant_key.as_str(), &[WIRE]),
        (typed_key.as_str(), &[WIRE]),
        ("stamp_pregrant_from_names(", &[WIRE, SPLIT]),
        ("forget_pregrant(", &[WIRE]),
        ("pregrant_from_metadata(", &[WIRE, RESOLVE]),
        ("mark_typed(", &[WIRE, STAMP, ROUTER]),
        ("is_typed(", &[WIRE, SPLIT]),
        ("forget_at_ingress(", &[WIRE, EXECUTE]),
    ];
    let mut offenders = Vec::new();
    let mut seen = [false; 8];
    for (rel, text) in &sources {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let code = code_keeping_literals(&production_text(&path, text));
        for (i, (needle, allowed)) in owners.iter().enumerate() {
            seen[i] |= code.contains(needle);
            if code.contains(needle) && !allowed.contains(&rel.as_str()) {
                offenders.push(format!("{rel}: {needle}"));
            }
        }
    }
    // Self-defence: a needle nobody spells would pass for the wrong reason.
    assert_eq!(seen, [true; 8], "a needle matched nothing: {owners:?}");
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// `execute.rs`:
/// - strips at ingress as its very first metadata operation, before
///   `stamp_btw` and before `admit_run` (whose steer-fold resolves this
///   turn's permissions);
/// - stamps its safety net unattested, never with the human-facing stamp;
/// - runs the split exactly once, at `execute()`'s top level — for every
///   request, not only for one that carries a slash mode — and before the
///   fast path; the restriction is stamped by no other route.
///
/// The router marks the mode it inserts for a channel message, in the same
/// block. The resolution folds the pre-grant into the one merge, after the
/// channel layer and before the all-default check. The run loop builds the
/// children's view from `for_children()` and its own from the whole policy.
/// The steering rescue strips the skill facts from the metadata it re-drives.
#[test]
fn the_fire_sites_are_where_the_design_puts_them() {
    use crate::utils::source_scan::{production_prefix, strip_comment_lines};
    let code = |src: &str| strip_comment_lines(&production_prefix(src));
    let once = |code: &str, needle: &str| -> usize {
        assert_eq!(
            code.matches(needle).count(),
            1,
            "`{needle}` must occur exactly once"
        );
        code.find(needle).expect("counted once above")
    };

    let execute = code(include_str!("../../execute.rs"));
    let ingress = once(&execute, "slash_skill_scope::forget_at_ingress(");
    let btw = once(&execute, "stamp_btw(&request.input");
    let admit = once(&execute, ".admit_run(");
    assert!(
        ingress < btw && btw < admit,
        "the ingress strip must be execute()'s first metadata step, before admit_run"
    );
    once(&execute, ".stamp_slash_mode_unattested(");
    assert!(
        !execute.contains(".stamp_slash_mode("),
        "execute()'s safety net must not use the human-facing stamp"
    );
    let split = once(&execute, "slash_skill_pregrant::split(");
    let fast_path = once(&execute, ".execute_slash_command_fast_path(");
    assert!(split < fast_path, "the split must run before the fast path");
    assert!(
        !execute.contains("stamp_from_mode("),
        "execute.rs restricts only through the split"
    );
    let line = execute
        .lines()
        .find(|l| l.contains("slash_skill_pregrant::split("))
        .expect("found above");
    assert!(
        line.starts_with("        super::") && !line.starts_with("         "),
        "the split must sit at `execute()`'s top level, not inside a branch: {line:?}"
    );

    let router = code(include_str!("../../../inbound_router/executor.rs"));
    let inserted = once(
        &router,
        "metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);",
    );
    let marked = once(&router, "slash_skill_scope::mark_typed(&mut metadata)");
    let block_end = router
        .get(inserted..)
        .and_then(|rest| rest.find('}'))
        .map(|at| at + inserted)
        .expect("the insert's block closes");
    assert!(
        inserted < marked && marked < block_end,
        "the router must mark exactly the mode it inserts, in the same block"
    );

    let resolve = code(include_str!("../../turn_permissions.rs"));
    let channel = once(
        &resolve,
        "merged = ToolPermissionsConfig::merge(&merged, &channel_perms)",
    );
    let fold = once(&resolve, "apply_pregrant(&mut merged, &pregrant)");
    let all_default = once(&resolve, "let is_all_default");
    assert!(
        channel < fold && fold < all_default,
        "the pre-grant folds after the channel layer and before the all-default check"
    );

    let inner = code(include_str!("../../run_loop/inner.rs"));
    let children = once(&inner, "let parent_view_for_children");
    let child_policy = once(&inner, "explicit.for_children()");
    let own = once(
        &inner,
        "let tool_service = super::super::build_request_tool_service(",
    );
    assert!(
        children < child_policy && child_policy < own,
        "the children's view must be built from `for_children()`"
    );
    assert_eq!(
        inner.matches("turn_permissions.explicit.clone()").count(),
        1,
        "the run's own service takes the whole policy, once"
    );

    let steering = code(include_str!("../../steering.rs"));
    let rescue = once(&steering, "fn build_steering_rescue_request(");
    let after = |needle: &str| {
        steering
            .get(rescue..)
            .and_then(|rest| rest.find(needle))
            .map(|at| at + rescue)
            .unwrap_or_else(|| panic!("`{needle}` after the rescue"))
    };
    let cloned = after("request.metadata.clone()");
    let stripped = after("slash_skill_scope::strip(&mut metadata)");
    let built = after("Some(RunRequest {");
    assert!(
        cloned < stripped && stripped < built,
        "the rescue must strip the skill facts before it re-drives"
    );
}
