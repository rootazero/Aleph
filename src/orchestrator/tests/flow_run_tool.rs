use crate::orchestrator::flow_run_tool::FlowRunTool;

#[test]
fn flow_run_descriptor_has_expected_shape() {
    let d = FlowRunTool::descriptor();
    assert_eq!(d.name, "flow_run");
    assert!(
        d.description.to_lowercase().contains("sub-flow"),
        "description: {}",
        d.description
    );
    // Schema is a JSON object with required flow_id + input.
    let required = d.schema["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "flow_id"));
    assert!(required.iter().any(|v| v == "input"));
}

#[test]
fn max_flow_depth_is_four() {
    // Pin the invariant: MAX_FLOW_DEPTH = 4 per design §7.
    use crate::orchestrator::resolver::MAX_FLOW_DEPTH;
    assert_eq!(MAX_FLOW_DEPTH, 4);
}
