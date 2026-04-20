use crate::orchestrator::errors::FlowError;

#[test]
fn display_unknown_flow() {
    let e = FlowError::UnknownFlow("nope".into());
    assert_eq!(e.to_string(), "unknown flow id: nope");
}

#[test]
fn display_recursion_limit() {
    let e = FlowError::RecursionLimit { max: 4 };
    assert_eq!(e.to_string(), "flow recursion limit (4) exceeded");
}

#[test]
fn display_session_conflict() {
    let e = FlowError::SessionConflict("sess-abc".into());
    assert_eq!(e.to_string(), "session sess-abc already dispatching");
}

#[test]
fn display_provider_unavailable() {
    let e = FlowError::ProviderUnavailable("minimax".into());
    assert_eq!(e.to_string(), "provider unavailable: minimax");
}
