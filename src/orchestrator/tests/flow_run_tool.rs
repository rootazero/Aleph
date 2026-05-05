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
