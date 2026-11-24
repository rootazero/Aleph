use super::{parse_aleph_plugin_toml_content, PanelUiManifest};
use std::path::Path;

#[test]
fn panel_manifest_schema_requires_only_declaration_facts() {
    let schema = serde_json::to_value(schemars::schema_for!(PanelUiManifest)).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"].as_array().unwrap().len(), 8);
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 8);
    assert!(properties.contains_key("asset_integrity"));
    assert!(!properties.contains_key("trust"));
}

#[test]
fn panel_manifest_native_toml_retains_explicit_section() {
    let input = r#"
[plugin]
id = "example"
name = "Example"
kind = "static"
version = "1.2.3"
[panel]
tab_id = "overview"
label = "Overview"
icon = "layout"
order = 10
entry = "ui/overview.js"
api_major = 1
permissions = ["sessions.list"]
asset_integrity = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
"#;
    let parsed = parse_aleph_plugin_toml_content(input, Path::new(".")).unwrap();
    assert_eq!(parsed.panel.unwrap().entry, "ui/overview.js");
    assert!(parse_aleph_plugin_toml_content(
        &input.replace("ui/overview.js", "../escape.js"),
        Path::new(".")
    )
    .is_err());
}
