use super::*;
use std::path::Path;
use tempfile::TempDir;

const HANDLER: &str = "handler";
const PAYLOAD_A: &str = r#"{"success":true,"result":"A","error":null}"#;
const PAYLOAD_B: &str = r#"{"success":true,"result":"B","error":null}"#;

fn manifest_with_guest(directory: &Path, id: &str, payload: &str) -> PluginManifest {
    let entry = format!("{id}.wasm");
    std::fs::write(directory.join(&entry), guest_module(HANDLER, payload)).unwrap();
    PluginManifest::new(
        id.to_string(),
        format!("Guest fixture {id}"),
        PluginKind::Wasm,
        entry.into(),
    )
    .with_root_dir(directory.to_path_buf())
}

fn call(
    runtime: &WasmRuntime,
    plugin_id: &str,
    export: &str,
) -> Result<WasmToolOutput, ExtensionError> {
    runtime.call_tool(
        plugin_id,
        export,
        WasmToolInput {
            name: "ignored".to_string(),
            arguments: serde_json::json!({}),
        },
    )
}

fn guest_module(handler: &str, payload: &str) -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();

    let mut types = vec![4];
    types.extend_from_slice(&[0x60, 1, 0x7e, 1, 0x7e]); // (i64) -> i64
    types.extend_from_slice(&[0x60, 2, 0x7e, 0x7f, 0]); // (i64, i32) -> ()
    types.extend_from_slice(&[0x60, 2, 0x7e, 0x7e, 0]); // (i64, i64) -> ()
    types.extend_from_slice(&[0x60, 0, 1, 0x7f]); // () -> i32
    append_section(&mut module, 1, types);

    let mut imports = vec![3];
    for (name, type_index) in [("alloc", 0), ("store_u8", 1), ("output_set", 2)] {
        append_name(&mut imports, "extism:host/env");
        append_name(&mut imports, name);
        imports.push(0); // Function import.
        append_uleb(&mut imports, type_index);
    }
    append_section(&mut module, 2, imports);

    append_section(&mut module, 3, vec![1, 3]); // One function, with type index 3.

    let mut exports = vec![1];
    append_name(&mut exports, handler);
    exports.push(0); // Function export.
    append_uleb(&mut exports, 3); // Imported functions occupy indices 0 through 2.
    append_section(&mut module, 7, exports);

    let body = guest_body(payload.as_bytes());
    let mut code = vec![1];
    append_uleb(&mut code, body.len());
    code.extend(body);
    append_section(&mut module, 10, code);
    module
}

fn guest_body(payload: &[u8]) -> Vec<u8> {
    let mut body = vec![1, 1, 0x7e]; // One i64 local holding the host-allocated output pointer.

    body.push(0x42); // i64.const payload length
    append_sleb(&mut body, payload.len() as i64);
    body.extend_from_slice(&[0x10, 0, 0x21, 0]); // call alloc; local.set 0

    for (offset, byte) in payload.iter().copied().enumerate() {
        body.extend_from_slice(&[0x20, 0, 0x42]); // local.get 0; i64.const offset
        append_sleb(&mut body, offset as i64);
        body.push(0x7c); // i64.add
        body.push(0x41); // i32.const byte
        append_sleb(&mut body, i64::from(byte));
        body.extend_from_slice(&[0x10, 1]); // call store_u8
    }

    body.extend_from_slice(&[0x20, 0, 0x42]); // local.get 0; i64.const payload length
    append_sleb(&mut body, payload.len() as i64);
    body.extend_from_slice(&[0x10, 2, 0x41, 0]); // output_set; i32.const 0
    body.push(0x0b); // end
    body
}

fn append_section(module: &mut Vec<u8>, id: u8, contents: Vec<u8>) {
    module.push(id);
    append_uleb(module, contents.len());
    module.extend(contents);
}

fn append_name(output: &mut Vec<u8>, value: &str) {
    append_uleb(output, value.len());
    output.extend_from_slice(value.as_bytes());
}

fn append_uleb(output: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn append_sleb(output: &mut Vec<u8>, mut value: i64) {
    loop {
        let mut byte = (value as u8) & 0x7f;
        value >>= 7;
        let sign_bit_set = byte & 0x40 != 0;
        let done = (value == 0 && !sign_bit_set) || (value == -1 && sign_bit_set);
        if !done {
            byte |= 0x80;
        }
        output.push(byte);
        if done {
            break;
        }
    }
}

#[test]
fn guest_fixture_distinct_instances_return_a_and_b() {
    let directory_a = TempDir::new().unwrap();
    let directory_b = TempDir::new().unwrap();
    let manifest_a = manifest_with_guest(directory_a.path(), "guest-a", PAYLOAD_A);
    let manifest_b = manifest_with_guest(directory_b.path(), "guest-b", PAYLOAD_B);
    let mut runtime = WasmRuntime::new();

    runtime.load_plugin(&manifest_a).unwrap();
    runtime.load_plugin(&manifest_b).unwrap();

    for (id, expected) in [
        ("guest-a", "A"),
        ("guest-b", "B"),
        ("guest-a", "A"),
        ("guest-b", "B"),
    ] {
        let output = call(&runtime, id, HANDLER).unwrap();
        assert!(output.success);
        assert_eq!(
            output.result,
            Some(serde_json::Value::String(expected.to_string()))
        );
        assert_eq!(output.error, None);
    }
}

#[test]
fn guest_fixture_same_id_reload_is_actual_new_guest() {
    let directory_a = TempDir::new().unwrap();
    let directory_b = TempDir::new().unwrap();
    let manifest_a = manifest_with_guest(directory_a.path(), "guest-reload", PAYLOAD_A);
    let manifest_b = manifest_with_guest(directory_b.path(), "guest-reload", PAYLOAD_B);
    let mut runtime = WasmRuntime::new();

    runtime.load_plugin(&manifest_a).unwrap();
    let first = call(&runtime, "guest-reload", HANDLER).unwrap();
    assert!(first.success);
    assert_eq!(
        first.result,
        Some(serde_json::Value::String("A".to_string()))
    );
    assert!(runtime.unload_plugin("guest-reload"));
    runtime.load_plugin(&manifest_b).unwrap();

    let second = call(&runtime, "guest-reload", HANDLER).unwrap();
    assert!(second.success);
    assert_eq!(
        second.result,
        Some(serde_json::Value::String("B".to_string()))
    );
    assert_eq!(second.error, None);
}

#[test]
fn guest_fixture_unknown_export_refuses() {
    let directory = TempDir::new().unwrap();
    let manifest = manifest_with_guest(directory.path(), "guest-export", PAYLOAD_A);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest).unwrap();

    let error = call(&runtime, "guest-export", "not_handler").unwrap_err();
    match error {
        ExtensionError::Runtime(message) => assert!(message.starts_with("WASM call failed:")),
        other => panic!("expected missing-export runtime error, got {other:?}"),
    }
}

#[test]
fn bound_fixture_current_instance_returns_a() {
    let directory = TempDir::new().unwrap();
    let manifest = manifest_with_guest(directory.path(), "guest-bound", PAYLOAD_A);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest).unwrap();

    let binding = runtime.bind("guest-bound", HANDLER).unwrap();
    let output = runtime
        .call_bound(
            &binding,
            WasmToolInput {
                name: "not_an_export".to_string(),
                arguments: serde_json::json!({}),
            },
        )
        .unwrap();
    assert!(output.success);
    assert_eq!(
        output.result,
        Some(serde_json::Value::String("A".to_string()))
    );
}

#[test]
fn bound_fixture_unloaded_refuses() {
    let directory = TempDir::new().unwrap();
    let manifest = manifest_with_guest(directory.path(), "guest-unload", PAYLOAD_A);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest).unwrap();
    let binding = runtime.bind("guest-unload", HANDLER).unwrap();

    assert!(runtime.unload_plugin("guest-unload"));
    assert!(matches!(
        runtime.call_bound(
            &binding,
            WasmToolInput {
                name: "ignored".to_string(),
                arguments: serde_json::json!({}),
            }
        ),
        Err(ExtensionError::PluginNotFound(id)) if id == "guest-unload"
    ));
}

#[test]
fn bound_fixture_replacement_refuses_old_and_new_returns_b() {
    let directory_a = TempDir::new().unwrap();
    let directory_b = TempDir::new().unwrap();
    let manifest_a = manifest_with_guest(directory_a.path(), "guest-replace", PAYLOAD_A);
    let manifest_b = manifest_with_guest(directory_b.path(), "guest-replace", PAYLOAD_B);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest_a).unwrap();
    let old = runtime.bind("guest-replace", HANDLER).unwrap();
    let old_clone = old.clone();
    assert_eq!(
        call(&runtime, "guest-replace", HANDLER).unwrap().result,
        Some(serde_json::Value::String("A".to_string()))
    );

    assert!(runtime.unload_plugin("guest-replace"));
    runtime.load_plugin(&manifest_b).unwrap();
    for binding in [&old, &old_clone] {
        assert!(matches!(
            runtime.call_bound(
                binding,
                WasmToolInput {
                    name: "ignored".to_string(),
                    arguments: serde_json::json!({}),
                }
            ),
            Err(ExtensionError::Runtime(message))
                if message == "WASM binding no longer mounted: guest-replace"
        ));
    }
    let new = runtime.bind("guest-replace", HANDLER).unwrap();
    assert_eq!(
        runtime
            .call_bound(
                &new,
                WasmToolInput {
                    name: "ignored".to_string(),
                    arguments: serde_json::json!({}),
                },
            )
            .unwrap()
            .result,
        Some(serde_json::Value::String("B".to_string()))
    );
}

#[test]
fn bound_fixture_direct_reload_refuses_old() {
    let directory_a = TempDir::new().unwrap();
    let directory_b = TempDir::new().unwrap();
    let manifest_a = manifest_with_guest(directory_a.path(), "guest-direct", PAYLOAD_A);
    let manifest_b = manifest_with_guest(directory_b.path(), "guest-direct", PAYLOAD_B);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest_a).unwrap();
    let old = runtime.bind("guest-direct", HANDLER).unwrap();
    runtime.load_plugin(&manifest_b).unwrap();

    assert!(matches!(
        runtime.call_bound(
            &old,
            WasmToolInput {
                name: "ignored".to_string(),
                arguments: serde_json::json!({}),
            },
        ),
        Err(ExtensionError::Runtime(message))
            if message == "WASM binding no longer mounted: guest-direct"
    ));
    let new = runtime.bind("guest-direct", HANDLER).unwrap();
    assert_eq!(
        runtime
            .call_bound(
                &new,
                WasmToolInput {
                    name: "ignored".to_string(),
                    arguments: serde_json::json!({}),
                },
            )
            .unwrap()
            .result,
        Some(serde_json::Value::String("B".to_string()))
    );
}

#[test]
fn bound_fixture_unknown_export_cannot_bind() {
    let directory = TempDir::new().unwrap();
    let manifest = manifest_with_guest(directory.path(), "guest-bind-export", PAYLOAD_A);
    let mut runtime = WasmRuntime::new();
    runtime.load_plugin(&manifest).unwrap();

    assert!(matches!(
        runtime.bind("guest-bind-export", "missing"),
        Err(ExtensionError::Runtime(message))
            if message == "WASM export not found: missing"
    ));
    assert!(matches!(
        runtime.bind("missing-plugin", HANDLER),
        Err(ExtensionError::PluginNotFound(id)) if id == "missing-plugin"
    ));
    let binding = runtime.bind("guest-bind-export", HANDLER).unwrap();
    assert_eq!(
        runtime
            .call_bound(
                &binding,
                WasmToolInput {
                    name: "ignored".to_string(),
                    arguments: serde_json::json!({}),
                },
            )
            .unwrap()
            .result,
        Some(serde_json::Value::String("A".to_string()))
    );
}

#[test]
fn guest_fixture_signed_leb_boundaries() {
    for (value, expected) in [
        (0, vec![0x00]),
        (63, vec![0x3f]),
        (64, vec![0xc0, 0x00]),
        (127, vec![0xff, 0x00]),
        (128, vec![0x80, 0x01]),
        (-1, vec![0x7f]),
        (-64, vec![0x40]),
        (-65, vec![0xbf, 0x7f]),
    ] {
        let mut encoded = Vec::new();
        append_sleb(&mut encoded, value);
        assert_eq!(encoded, expected, "signed LEB128 for {value}");
    }
}
