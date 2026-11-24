use super::{parse_cc_plugin_json_content, parse_cc_plugin_toml_content};
use serde_json::json;
use std::path::Path;

fn panel() -> serde_json::Value {
    json!({
        "tab_id": "overview", "label": "Overview", "icon": "layout", "order": 10,
        "entry": "ui/overview.js", "api_major": 1, "permissions": ["sessions.list"],
        "asset_integrity": "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    })
}

#[test]
fn panel_manifest_json_toml_preserve_the_same_explicit_facts() {
    let json_manifest = json!({"name": "community-example", "version": "1.2.3", "panel": panel()});
    let parsed = parse_cc_plugin_json_content(&json_manifest.to_string(), Path::new(".")).unwrap();
    let toml_manifest = format!(
        "name = \"community-example\"\nversion = \"1.2.3\"\n[panel]\n{}",
        toml::to_string(&panel()).unwrap()
    );
    let other = parse_cc_plugin_toml_content(&toml_manifest, Path::new(".")).unwrap();
    assert_eq!(parsed.panel, other.panel);
    assert_eq!(
        serde_json::to_value(parsed.panel.unwrap()).unwrap(),
        panel()
    );
}

#[test]
fn panel_manifest_legacy_and_generic_runtime_fields_never_infer_ui() {
    for runtime in ["static", "wasm", "mcp"] {
        let input = json!({"name":"legacy", "aleph":{"runtime":runtime, "entry":"ui.js"}});
        let parsed = parse_cc_plugin_json_content(&input.to_string(), Path::new(".")).unwrap();
        assert!(parsed.panel.is_none());
    }
    assert!(
        parse_cc_plugin_toml_content("name = \"legacy\"", Path::new("."))
            .unwrap()
            .panel
            .is_none()
    );
}

#[test]
fn panel_manifest_rejects_missing_unknown_and_host_owned_fields() {
    for missing in [
        "tab_id",
        "label",
        "icon",
        "order",
        "entry",
        "api_major",
        "permissions",
        "asset_integrity",
    ] {
        let mut section = panel();
        section.as_object_mut().unwrap().remove(missing);
        assert!(
            parse_cc_plugin_json_content(
                &json!({"name":"example", "panel":section}).to_string(),
                Path::new(".")
            )
            .is_err(),
            "{missing}"
        );
        let input = format!(
            "name = \"example\"\n[panel]\n{}",
            toml::to_string(&section).unwrap()
        );
        assert!(
            parse_cc_plugin_toml_content(&input, Path::new(".")).is_err(),
            "{missing}"
        );
    }
    for unknown in [
        "trust",
        "owner_generation",
        "status",
        "credentials",
        "public_base",
        "extra",
    ] {
        let mut section = panel();
        section[unknown] = json!("forbidden");
        assert!(
            parse_cc_plugin_json_content(
                &json!({"name":"example", "panel":section}).to_string(),
                Path::new(".")
            )
            .is_err(),
            "{unknown}"
        );
        let input = format!(
            "name = \"example\"\n[panel]\n{}",
            toml::to_string(&section).unwrap()
        );
        assert!(
            parse_cc_plugin_toml_content(&input, Path::new(".")).is_err(),
            "{unknown}"
        );
    }
}

#[test]
fn panel_manifest_requires_plugin_version() {
    let section = panel();
    assert!(parse_cc_plugin_json_content(
        &json!({"name": "example", "panel": section}).to_string(),
        Path::new("."),
    )
    .is_err());

    let input = format!(
        "name = \"example\"\n[panel]\n{}",
        toml::to_string(&panel()).unwrap()
    );
    assert!(parse_cc_plugin_toml_content(&input, Path::new(".")).is_err());
}

#[test]
fn panel_manifest_rejects_invalid_paths_integrity_permissions_and_api() {
    for (field, value) in [
        ("entry", json!("../escape.js")),
        ("entry", json!("/absolute.js")),
        ("entry", json!("https://example.invalid/ui.js")),
        ("entry", json!("C:\\ui.js")),
        ("entry", json!("ui/%2e%2e/escape.js")),
        ("entry", json!("ui//x.js")),
        ("entry", json!("ui/./x.js")),
        ("entry", json!("")),
        ("tab_id", json!("..")),
        ("tab_id", json!("a:b")),
        ("label", json!(" ")),
        ("icon", json!("")),
        ("api_major", json!(2)),
        ("asset_integrity", json!("sha256-short")),
        (
            "asset_integrity",
            json!("sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
        ),
        ("permissions", json!(["filesystem:read"])),
        ("permissions", json!(["gateway.rpc"])),
    ] {
        let mut section = panel();
        section[field] = value.clone();
        let json_result = parse_cc_plugin_json_content(
            &json!({"name":"example", "panel":section.clone()}).to_string(),
            Path::new("."),
        );
        let toml_result = parse_cc_plugin_toml_content(
            &format!(
                "name = \"example\"\n[panel]\n{}",
                toml::to_string(&section).unwrap()
            ),
            Path::new("."),
        );
        assert!(json_result.is_err(), "JSON {field}={value}");
        assert!(toml_result.is_err(), "TOML {field}={value}");
    }
}
