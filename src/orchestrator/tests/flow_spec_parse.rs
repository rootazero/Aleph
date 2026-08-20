use crate::orchestrator::flow_spec::{BrainRef, FlowOverrides, FlowSpec, SessionStrategy};

#[test]
fn parses_minimal_flow_spec() {
    let toml_src = r#"
        id = "default-agent"
        description = "Primary chat agent"
        agent = "main"

        [brain]
        kind = "default"

        [session_strategy]
        kind = "reuse"
    "#;
    let flow: FlowSpec = toml::from_str(toml_src).expect("parse");
    assert_eq!(flow.id, "default-agent");
    assert_eq!(flow.agent, "main");
    assert!(matches!(flow.brain, BrainRef::Default));
    assert!(matches!(flow.session_strategy, SessionStrategy::Reuse));
    assert!(flow.overrides.max_iterations.is_none());
}

#[test]
fn parses_strict_brain_and_child_session() {
    let toml_src = r#"
        id = "researcher"
        description = "Read-only web researcher"
        agent = "researcher"

        [brain]
        kind = "strict"
        provider = "minimax"
        model = "text-01"

        [session_strategy]
        kind = "child"

        [overrides]
        max_iterations = 10
    "#;
    let flow: FlowSpec = toml::from_str(toml_src).expect("parse");
    match flow.brain {
        BrainRef::Strict { provider, model } => {
            assert_eq!(provider, "minimax");
            assert_eq!(model.as_deref(), Some("text-01"));
        }
        other => panic!("expected Strict, got {other:?}"),
    }
    match flow.session_strategy {
        SessionStrategy::Child { parent_session_key } => {
            assert!(parent_session_key.is_none(), "parent injected at runtime");
        }
        other => panic!("expected Child, got {other:?}"),
    }
    assert_eq!(flow.overrides.max_iterations, Some(10));
}

#[test]
fn rejects_unknown_fields() {
    let toml_src = r#"
        id = "x"
        description = "x"
        agent = "x"
        unknown_field = "boom"

        [brain]
        kind = "default"

        [session_strategy]
        kind = "fresh"
    "#;
    let err = toml::from_str::<FlowSpec>(toml_src).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("unknown"),
        "got {err}"
    );
}

#[test]
fn roundtrips_preferred_brain() {
    let flow = FlowSpec {
        id: "x".into(),
        description: "x".into(),
        agent: "x".into(),
        brain: BrainRef::Preferred {
            provider: "chatgpt".into(),
        },
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    };
    let s = toml::to_string(&flow).unwrap();
    let back: FlowSpec = toml::from_str(&s).unwrap();
    assert_eq!(back.id, flow.id);
    assert!(matches!(back.brain, BrainRef::Preferred { provider } if provider == "chatgpt"));
}
