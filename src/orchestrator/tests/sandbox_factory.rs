use crate::sync_primitives::Arc;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::sandbox_factory::build_sandbox_factory;
use crate::sandbox::{NoopSandbox, Sandbox};

#[test]
fn factory_delegates_to_the_workspace_builder_with_the_session_key() {
    let seen: Arc<crate::sync_primitives::Mutex<Vec<String>>> =
        Arc::new(crate::sync_primitives::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let factory = build_sandbox_factory(Arc::new(move |session_key: &str| {
        recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_key.to_string());
        Ok(Arc::new(NoopSandbox) as Arc<dyn Sandbox>)
    }));

    factory("sess-abc").expect("workspace builder succeeded");

    assert_eq!(
        &*seen.lock().unwrap_or_else(|e| e.into_inner()),
        &["sess-abc".to_string()],
        "the factory's only job is to hand the session key to the builder"
    );
}

#[test]
fn a_workspace_builder_failure_surfaces_as_sandbox_provision_failed() {
    let factory =
        build_sandbox_factory(Arc::new(|_session_key: &str| Err("disk full".to_string())));

    // `Arc<dyn Sandbox>` is not `Debug`, so `expect_err` is unavailable here.
    match factory("sess-abc") {
        Ok(_) => panic!("a failing workspace builder must not yield a sandbox"),
        Err(FlowError::SandboxProvisionFailed(m)) => assert_eq!(m, "disk full"),
        Err(other) => panic!("got {other:?}"),
    }
}

/// The `SandboxKind` axis is gone on purpose, and the reason is a *security*
/// reason rather than a tidiness one — so it gets a guard rather than a
/// comment. `SandboxFactory` takes a session key and nothing else; if a future
/// change reintroduces a per-flow sandbox selector, this file stops compiling
/// and whoever does it has to re-read `sandbox_factory.rs`'s module doc first.
///
/// Source-level, because at runtime "the flow picked a restrictive sandbox"
/// and "the flow picked the boot sandbox" were indistinguishable — the picked
/// sandbox only ever reached `.summary()` in the prompt, never tool execution.
#[test]
fn the_factory_has_no_per_flow_selector() {
    let src = include_str!("../sandbox_factory.rs");
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("SandboxKind"),
        "a per-flow sandbox selector is back in sandbox_factory.rs. Enforcement \
         lives in src/tools/scoped/ and nowhere else (CLAUDE.md); a selector \
         here can only mislead the prompt or become a second enforcement point."
    );
}
