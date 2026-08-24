use crate::orchestrator::loader::{load_presets, load_user_flows_from_str};

#[test]
fn the_preset_catalog_is_exactly_the_canonical_agent_flow() {
    let set = load_presets().expect("parse presets");
    let mut ids: Vec<&str> = set.keys().map(String::as_str).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["default-agent"],
        "the catalog collapsed to one flow on purpose — see default_flows.toml. \
         Six per-agent presets used to sit here declaring `session_strategy = child` \
         with no parent, which minted a fresh UUID session on every turn and \
         discarded the caller's session_hint. Adding a per-agent preset back means \
         re-deciding that; per-agent flows belong in ~/.aleph/flows/<id>.toml."
    );
}

#[test]
fn preset_default_agent_targets_main() {
    let set = load_presets().expect("parse presets");
    let default_agent = set.get("default-agent").unwrap();
    assert_eq!(default_agent.agent, "main");
}

/// The preset every unrouted agent falls back to must honour `session_hint`.
///
/// `Reuse` is the only strategy that does: `Fresh` mints a new key by
/// definition, and `Child` without a runtime parent degrades to the same
/// thing while never consulting the hint (`resolver::resolve_session`). Since
/// `fallback_spec_with_agent` clones this spec for *every* agent that has no
/// flow of its own, a non-`Reuse` strategy here silently un-persists every
/// conversation in the product at once.
#[test]
fn the_fallback_preset_honours_the_session_hint() {
    use crate::orchestrator::flow_spec::SessionStrategy;
    use crate::orchestrator::resolver::{resolve_session, SessionResolveInput};

    let set = load_presets().expect("parse presets");
    let spec = set
        .get(crate::orchestrator::resolver::DEFAULT_AGENT_FLOW_ID)
        .expect("the fallback flow must exist — dispatch resolves it by name");

    assert!(
        matches!(spec.session_strategy, SessionStrategy::Reuse),
        "got {:?}",
        spec.session_strategy
    );

    let resolved = resolve_session(SessionResolveInput {
        strategy: spec.session_strategy.clone(),
        session_hint: Some("caller-supplied-key".to_string()),
        parent_session: None,
        fresh_key_fn: || "a-freshly-minted-uuid".to_string(),
    })
    .expect("reuse with a hint resolves");

    assert_eq!(
        resolved.session_key, "caller-supplied-key",
        "the fallback flow discarded the caller's session key — every agent \
         without its own flow just lost cross-turn memory"
    );
    assert!(!resolved.is_new);
}

/// `load_catalog` is the only place presets and user flows are combined.
///
/// Boot and the `gateway.flow.reload` RPC each used to compose the catalog
/// themselves, and they disagreed: reload read `~/.aleph/flows/*.toml`, boot
/// did not. An operator's flow therefore took effect on reload and vanished on
/// the next restart, silently in both directions. Now both call `load_catalog`,
/// which makes an equality assertion between them tautological — so the
/// property worth pinning is that no third composer grows back.
#[test]
fn the_catalog_has_exactly_one_composer() {
    let src = include_str!("../loader.rs");
    let production = crate::utils::source_scan::strip_comment_lines(
        &crate::utils::source_scan::production_prefix(src),
    );
    assert!(
        production.contains("pub async fn load_catalog"),
        "load_catalog is the single composer; it must live in loader.rs"
    );

    // Every caller outside loader.rs must go through it. `load_presets` alone
    // is what boot used to call, and that call is exactly the bug.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for rel in [
        "src/bin/aleph-server/commands/start/orchestrator_init.rs",
        "src/gateway/handlers/flow_admin.rs",
    ] {
        let path = repo_root.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let code = crate::utils::source_scan::strip_comment_lines(
            &crate::utils::source_scan::production_prefix(&text),
        );
        if code.contains("merge_catalogs(") || code.contains("load_user_flows_from_dir(") {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "these hand-roll the catalog instead of calling load_catalog: {offenders:?}"
    );
}

#[test]
fn user_flow_file_with_single_flow_parses() {
    let toml_src = r#"
[[flow]]
id = "user/my-flow"
description = "My custom flow"
agent = "main"
brain = { kind = "preferred", provider = "minimax" }
session_strategy = { kind = "fresh" }
"#;
    let set = load_user_flows_from_str(toml_src).expect("parse");
    assert_eq!(set.len(), 1);
    assert!(set.contains_key("user/my-flow"));
}

/// The documented reason to author a user flow: pin one agent to one model.
///
/// `BrainRef::Strict { provider, model }` is the live half of `FlowSpec` —
/// `harness_bridge::llm::pick_llm` wraps the named provider in
/// `ModelOverrideProvider` so the model is stamped on every request. Boot did
/// not read `~/.aleph/flows` until 2026-08-20, so this capability shipped with
/// no reachable producer.
#[test]
fn a_user_flow_can_pin_an_agent_to_a_provider_and_model() {
    use crate::orchestrator::flow_spec::BrainRef;

    let toml_src = r#"
[[flow]]
id = "coder"
description = "Pin the coder agent to a strong model"
agent = "coder"
brain = { kind = "strict", provider = "anthropic", model = "claude-opus-5" }
session_strategy = { kind = "reuse" }
"#;
    let set = load_user_flows_from_str(toml_src).expect("parse");
    let spec = set.get("coder").expect("flow id must equal the agent id");
    match &spec.brain {
        BrainRef::Strict { provider, model } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model.as_deref(), Some("claude-opus-5"));
        }
        other => panic!("expected Strict, got {other:?}"),
    }
}

#[test]
fn duplicate_flow_id_within_single_file_is_rejected() {
    let toml_src = r#"
[[flow]]
id = "dup"
description = "first"
agent = "main"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }

[[flow]]
id = "dup"
description = "second"
agent = "main"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }
"#;
    let err = load_user_flows_from_str(toml_src).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("duplicate"),
        "got {err}"
    );
}
