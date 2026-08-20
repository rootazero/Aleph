use crate::sync_primitives::Arc;

use crate::orchestrator::flow_registry::{FlowRegistry, FlowSet};
use crate::orchestrator::flow_spec::{BrainRef, FlowOverrides, FlowSpec, SessionStrategy};

fn mk_spec(id: &str, agent: &str) -> FlowSpec {
    FlowSpec {
        id: id.into(),
        description: "test".into(),
        agent: agent.into(),
        brain: BrainRef::Default,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    }
}

#[test]
fn resolve_returns_spec_by_id() {
    let mut map = FlowSet::new();
    map.insert("a".into(), Arc::new(mk_spec("a", "main")));
    let reg = FlowRegistry::new(map);
    let got = reg.resolve("a").expect("present");
    assert_eq!(got.id, "a");
    assert_eq!(got.agent, "main");
}

#[test]
fn resolve_unknown_returns_none() {
    let reg = FlowRegistry::new(FlowSet::new());
    assert!(reg.resolve("nope").is_none());
}

#[test]
fn replace_swaps_atomically() {
    let mut map = FlowSet::new();
    map.insert("a".into(), Arc::new(mk_spec("a", "main")));
    let reg = FlowRegistry::new(map);

    // Hold a snapshot from before the swap.
    let snap_before = reg.resolve("a").unwrap();

    let mut new_map = FlowSet::new();
    new_map.insert("a".into(), Arc::new(mk_spec("a", "coder"))); // same id, different agent
    new_map.insert("b".into(), Arc::new(mk_spec("b", "explore")));
    reg.replace(new_map);

    // In-flight handle still sees the old agent — Arc snapshot semantics.
    assert_eq!(snap_before.agent, "main");

    // New resolves see the new catalog.
    assert_eq!(reg.resolve("a").unwrap().agent, "coder");
    assert_eq!(reg.resolve("b").unwrap().agent, "explore");
}

#[test]
fn list_ids_is_sorted() {
    let mut map = FlowSet::new();
    map.insert("zeta".into(), Arc::new(mk_spec("zeta", "main")));
    map.insert("alpha".into(), Arc::new(mk_spec("alpha", "main")));
    map.insert("mid".into(), Arc::new(mk_spec("mid", "main")));
    let reg = FlowRegistry::new(map);
    assert_eq!(reg.list_ids(), vec!["alpha", "mid", "zeta"]);
}
