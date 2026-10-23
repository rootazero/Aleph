use super::*;
use std::path::Path;
use tempfile::TempDir;

pub(crate) const HANDLER: &str = "handler";
pub(crate) const PAYLOAD_A: &str = r#"{"success":true,"result":"A","error":null}"#;
pub(crate) const PAYLOAD_B: &str = r#"{"success":true,"result":"B","error":null}"#;

pub(crate) fn manifest_with_guest(directory: &Path, id: &str, payload: &str) -> PluginManifest {
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

/// A valid module that imports WASI fd_write. Loading it is a runtime test of
/// the linker authority rather than an assertion about manifest metadata.
pub(crate) fn write_wasi_guest(path: &Path) {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    append_section(
        &mut module,
        1,
        vec![
            2, 0x60, 4, 0x7f, 0x7f, 0x7f, 0x7f, 1, 0x7f, 0x60, 0, 1, 0x7f,
        ],
    );
    let mut imports = vec![1];
    append_name(&mut imports, "wasi_snapshot_preview1");
    append_name(&mut imports, "fd_write");
    imports.extend_from_slice(&[0, 0]);
    append_section(&mut module, 2, imports);
    append_section(&mut module, 3, vec![1, 1]);
    append_section(&mut module, 5, vec![1, 0, 1]);
    let mut exports = vec![2];
    append_name(&mut exports, "memory");
    exports.extend_from_slice(&[2, 0]);
    append_name(&mut exports, HANDLER);
    exports.extend_from_slice(&[0, 1]);
    append_section(&mut module, 7, exports);
    append_section(&mut module, 10, vec![1, 4, 0, 0x41, 0, 0x0b]);
    std::fs::write(path, module).unwrap();
}

/// Exercise Extism's own log_error import, independently of Aleph's custom log.
pub(crate) fn write_builtin_log_guest(path: &Path, message: &str) {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    append_section(
        &mut module,
        1,
        vec![
            4, 0x60, 1, 0x7e, 1, 0x7e, 0x60, 2, 0x7e, 0x7f, 0, 0x60, 1, 0x7e, 0, 0x60, 0, 1, 0x7f,
        ],
    );
    let mut imports = vec![3];
    for (name, signature) in [("alloc", 0), ("store_u8", 1), ("log_error", 2)] {
        append_name(&mut imports, "extism:host/env");
        append_name(&mut imports, name);
        imports.extend_from_slice(&[0, signature]);
    }
    append_section(&mut module, 2, imports);
    append_section(&mut module, 3, vec![1, 3]);
    let mut exports = vec![1];
    append_name(&mut exports, HANDLER);
    exports.extend_from_slice(&[0, 3]);
    append_section(&mut module, 7, exports);
    let mut body = vec![1, 1, 0x7e, 0x42];
    append_sleb(&mut body, message.len() as i64);
    body.extend_from_slice(&[0x10, 0, 0x21, 0]);
    for (index, byte) in message.bytes().enumerate() {
        body.extend_from_slice(&[0x20, 0, 0x42]);
        append_sleb(&mut body, index as i64);
        body.extend_from_slice(&[0x7c, 0x41]);
        append_sleb(&mut body, i64::from(byte));
        body.extend_from_slice(&[0x10, 1]);
    }
    body.extend_from_slice(&[0x20, 0, 0x10, 2, 0x41, 0, 0x0b]);
    let mut code = vec![1];
    append_uleb(&mut code, body.len());
    code.extend(body);
    append_section(&mut module, 10, code);
    std::fs::write(path, module).unwrap();
}

/// The same real guest, trapping unless every byte of the JSON ABI input matches.
pub(crate) fn manifest_with_checked_input_guest(
    directory: &Path,
    id: &str,
    payload: &str,
    expected_input: &str,
) -> PluginManifest {
    let manifest = manifest_with_guest(directory, id, payload);
    std::fs::write(
        manifest.entry_path().unwrap(),
        guest_module_checked(HANDLER, payload, Some(expected_input.as_bytes())),
    )
    .unwrap();
    manifest
}

/// A real JSON-ABI identity transform: copy snapshot.payload, measurements and
/// fingerprint from each request, accepting fresh request ids and content. This
/// fixture supports object/array payloads and canonical host JSON field order.
/// It is guest bytecode, not a host callback or a precomputed response.
pub(crate) fn manifest_with_context_identity_guest(directory: &Path, id: &str) -> PluginManifest {
    let manifest = manifest_with_guest(directory, id, PAYLOAD_A);
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    let mut types = vec![6];
    for signature in [
        &[0x60, 1, 0x7e, 1, 0x7e][..],
        &[0x60, 2, 0x7e, 0x7f, 0][..],
        &[0x60, 2, 0x7e, 0x7e, 0][..],
        &[0x60, 0, 1, 0x7f][..],
        &[0x60, 0, 1, 0x7e][..],
        &[0x60, 1, 0x7e, 1, 0x7f][..],
    ] {
        types.extend_from_slice(signature);
    }
    append_section(&mut module, 1, types);
    let mut imports = vec![5];
    for (name, signature) in [
        ("alloc", 0),
        ("store_u8", 1),
        ("output_set", 2),
        ("input_load_u8", 5),
        ("input_length", 4),
    ] {
        append_name(&mut imports, "extism:host/env");
        append_name(&mut imports, name);
        imports.push(0);
        append_uleb(&mut imports, signature);
    }
    append_section(&mut module, 2, imports);
    append_section(&mut module, 3, vec![1, 3]);
    let mut exports = vec![1];
    append_name(&mut exports, HANDLER);
    exports.extend_from_slice(&[0, 5]);
    append_section(&mut module, 7, exports);

    // Locals: out pointer, out length, scan, begin, end, input length (i64),
    // current byte, nesting depth, inside string, escaped (i32).
    let mut body = vec![2, 6, 0x7e, 4, 0x7f];
    body.extend_from_slice(&[0x10, 4, 0x22, 5, 0x42]);
    append_sleb(&mut body, 1024);
    body.extend_from_slice(&[0x7c, 0x10, 0, 0x21, 0]);
    identity_literal(
        &mut body,
        br#"{"success":true,"result":{"input_fingerprint":"#,
    );
    identity_seek(&mut body, b"\"input_fingerprint\":");
    // Copy a quoted canonical SHA256 token (71 bytes plus quotes).
    identity_copy_fixed(&mut body, 73);
    identity_seek(&mut body, b"\"snapshot\":{");
    identity_literal(&mut body, br#", "output_bytes":"#);
    identity_seek(&mut body, b"\"byte_count\":");
    identity_number(&mut body);
    identity_literal(&mut body, br#", "output":"#);
    identity_seek(&mut body, b"\"payload\":");
    identity_json(&mut body);
    identity_literal(&mut body, br#", "output_tokens":"#);
    identity_seek(&mut body, b"\"token_count\":");
    identity_number(&mut body);
    identity_literal(
        &mut body,
        br#", "strategy":"fixture.identity", "quality":"condensed"}, "error":null}"#,
    );
    body.extend_from_slice(&[0x20, 0, 0x20, 1, 0x10, 2, 0x41, 0, 0x0b]);
    let mut code = vec![1];
    append_uleb(&mut code, body.len());
    code.extend(body);
    append_section(&mut module, 10, code);
    std::fs::write(manifest.entry_path().unwrap(), module).unwrap();
    manifest
}

fn identity_increment(body: &mut Vec<u8>, local: u8) {
    body.extend_from_slice(&[0x20, local, 0x42, 1, 0x7c, 0x21, local]);
}

fn identity_store(body: &mut Vec<u8>) {
    body.extend_from_slice(&[0x20, 0, 0x20, 1, 0x7c, 0x20, 6, 0x10, 1]);
    identity_increment(body, 1);
}

fn identity_literal(body: &mut Vec<u8>, literal: &[u8]) {
    for byte in literal {
        body.push(0x41);
        append_sleb(body, i64::from(*byte));
        body.extend_from_slice(&[0x21, 6]);
        identity_store(body);
    }
}

fn identity_seek(body: &mut Vec<u8>, needle: &[u8]) {
    body.extend_from_slice(&[0x02, 0x40, 0x03, 0x40]); // block; loop
    body.extend_from_slice(&[0x20, 2, 0x42]);
    append_sleb(body, needle.len() as i64);
    body.extend_from_slice(&[0x7c, 0x20, 5, 0x56, 0x04, 0x40, 0, 0x0b]);
    for (offset, byte) in needle.iter().enumerate() {
        body.extend_from_slice(&[0x20, 2, 0x42]);
        append_sleb(body, offset as i64);
        body.extend_from_slice(&[0x7c, 0x10, 3, 0x41]);
        append_sleb(body, i64::from(*byte));
        body.push(0x46); // i32.eq
        if offset > 0 {
            body.push(0x71);
        } // i32.and
    }
    body.extend_from_slice(&[0x04, 0x40, 0x20, 2, 0x42]);
    append_sleb(body, needle.len() as i64);
    body.extend_from_slice(&[0x7c, 0x21, 2, 0x0c, 2, 0x0b]);
    identity_increment(body, 2);
    body.extend_from_slice(&[0x0c, 0, 0x0b, 0x0b]);
}

fn identity_copy_fixed(body: &mut Vec<u8>, bytes: usize) {
    for _ in 0..bytes {
        body.extend_from_slice(&[0x20, 2, 0x10, 3, 0x21, 6]);
        identity_store(body);
        identity_increment(body, 2);
    }
}

fn identity_number(body: &mut Vec<u8>) {
    body.extend_from_slice(&[0x02, 0x40, 0x03, 0x40, 0x20, 2, 0x10, 3, 0x22, 6]);
    body.extend_from_slice(&[0x41, 48, 0x49, 0x20, 6, 0x41, 57, 0x4b, 0x72, 0x0d, 1]);
    identity_store(body);
    identity_increment(body, 2);
    body.extend_from_slice(&[0x0c, 0, 0x0b, 0x0b]);
}

fn identity_byte_is(body: &mut Vec<u8>, byte: u8) {
    body.extend_from_slice(&[0x20, 6, 0x41]);
    append_sleb(body, i64::from(byte));
    body.push(0x46);
}

fn identity_json(body: &mut Vec<u8>) {
    for local in [7, 8, 9] {
        body.extend_from_slice(&[0x41, 0, 0x21, local]);
    }
    // Only object/array snapshots are exercised by this fixture.
    body.extend_from_slice(&[0x20, 2, 0x10, 3, 0x21, 6]);
    identity_byte_is(body, b'{');
    identity_byte_is(body, b'[');
    body.extend_from_slice(&[0x72, 0x45, 0x04, 0x40, 0, 0x0b]);
    body.extend_from_slice(&[0x02, 0x40, 0x03, 0x40]);
    body.extend_from_slice(&[0x20, 2, 0x20, 5, 0x5a, 0x04, 0x40, 0, 0x0b]); // ge_u -> trap
    body.extend_from_slice(&[0x20, 2, 0x10, 3, 0x21, 6]);
    identity_store(body);
    identity_increment(body, 2);
    body.extend_from_slice(&[0x20, 8, 0x04, 0x40]); // inside a string
    body.extend_from_slice(&[0x20, 9, 0x04, 0x40, 0x41, 0, 0x21, 9, 0x05]);
    identity_byte_is(body, b'\\');
    body.extend_from_slice(&[0x04, 0x40, 0x41, 1, 0x21, 9, 0x05]);
    identity_byte_is(body, b'"');
    body.extend_from_slice(&[0x04, 0x40, 0x41, 0, 0x21, 8, 0x0b, 0x0b, 0x0b]);
    body.push(0x05); // outside a string
    identity_byte_is(body, b'"');
    body.extend_from_slice(&[0x04, 0x40, 0x41, 1, 0x21, 8, 0x0b]);
    identity_byte_is(body, b'{');
    identity_byte_is(body, b'[');
    body.extend_from_slice(&[0x72, 0x04, 0x40, 0x20, 7, 0x41, 1, 0x6a, 0x21, 7, 0x0b]);
    identity_byte_is(body, b'}');
    identity_byte_is(body, b']');
    body.extend_from_slice(&[
        0x72, 0x04, 0x40, 0x20, 7, 0x41, 1, 0x6b, 0x21, 7, 0x0b, 0x0b,
    ]);
    body.extend_from_slice(&[0x20, 7, 0x45, 0x0d, 1, 0x0c, 0, 0x0b, 0x0b]);
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
    guest_module_checked(handler, payload, None)
}

pub(crate) fn write_memory_probe_guest(path: &Path, bytes: i64) {
    let mut probe = vec![0x42];
    append_sleb(&mut probe, bytes);
    // Allocation must succeed before the guest can produce its normal reply.
    probe.extend_from_slice(&[0x10, 0, 0x50, 0x04, 0x40, 0x00, 0x0b]);
    std::fs::write(
        path,
        guest_module_with_probe(HANDLER, PAYLOAD_A, None, &probe),
    )
    .unwrap();
}

pub(crate) fn write_fuel_probe_guest(path: &Path) {
    // A valid infinite loop; either fuel or the existing deadline must stop it.
    let probe = [0x03, 0x40, 0x0c, 0, 0x0b];
    std::fs::write(
        path,
        guest_module_with_probe(HANDLER, PAYLOAD_A, None, &probe),
    )
    .unwrap();
}

pub(crate) fn call_guest_raw(runtime: &WasmRuntime, plugin_id: &str) -> Result<Vec<u8>, String> {
    let instance = runtime.plugins.get(plugin_id).ok_or("missing guest")?;
    let mut plugin = instance.plugin.lock().unwrap();
    plugin
        .call::<_, Vec<u8>>(HANDLER, "")
        .map_err(|error| format!("{error:#}"))
}

fn guest_module_checked(handler: &str, payload: &str, expected_input: Option<&[u8]>) -> Vec<u8> {
    guest_module_with_probe(handler, payload, expected_input, &[])
}

/// Real guest calls a test-only host import before producing its JSON output.
/// This lets lifecycle tests stop during a guest invocation, not a host mock.
pub(crate) fn write_barrier_guest(path: &Path, payload: &str) {
    std::fs::write(
        path,
        guest_module_with_import_probe(HANDLER, payload, None, &[0x10, 5], true),
    )
    .unwrap();
}

fn guest_module_with_probe(
    handler: &str,
    payload: &str,
    expected_input: Option<&[u8]>,
    probe: &[u8],
) -> Vec<u8> {
    guest_module_with_import_probe(handler, payload, expected_input, probe, false)
}

fn guest_module_with_import_probe(
    handler: &str,
    payload: &str,
    expected_input: Option<&[u8]>,
    probe: &[u8],
    barrier: bool,
) -> Vec<u8> {
    let mut module = b"\0asm\x01\0\0\0".to_vec();

    let mut types = vec![if barrier { 7 } else { 6 }];
    types.extend_from_slice(&[0x60, 1, 0x7e, 1, 0x7e]); // (i64) -> i64
    types.extend_from_slice(&[0x60, 2, 0x7e, 0x7f, 0]); // (i64, i32) -> ()
    types.extend_from_slice(&[0x60, 2, 0x7e, 0x7e, 0]); // (i64, i64) -> ()
    types.extend_from_slice(&[0x60, 0, 1, 0x7f]); // () -> i32
    types.extend_from_slice(&[0x60, 0, 1, 0x7e]); // () -> i64
    types.extend_from_slice(&[0x60, 1, 0x7e, 1, 0x7f]); // (i64) -> i32
    if barrier {
        types.extend_from_slice(&[0x60, 0, 0]);
    } // () -> ()
    append_section(&mut module, 1, types);

    let mut imports = vec![if barrier { 6 } else { 5 }];
    for (name, type_index) in [
        ("alloc", 0),
        ("store_u8", 1),
        ("output_set", 2),
        ("input_load_u8", 5),
        ("input_length", 4),
    ] {
        append_name(&mut imports, "extism:host/env");
        append_name(&mut imports, name);
        imports.push(0); // Function import.
        append_uleb(&mut imports, type_index);
    }
    if barrier {
        append_name(&mut imports, "extism:host/user");
        append_name(&mut imports, "b3_test_barrier");
        imports.extend_from_slice(&[0, 6]);
    }
    append_section(&mut module, 2, imports);

    append_section(&mut module, 3, vec![1, 3]); // One function, with type index 3.

    let mut exports = vec![1];
    append_name(&mut exports, handler);
    exports.push(0); // Function export.
    append_uleb(&mut exports, if barrier { 6 } else { 5 }); // Export follows the imports.
    append_section(&mut module, 7, exports);

    let mut body = guest_body(payload.as_bytes());
    body.splice(3..3, probe.iter().copied());
    if let Some(expected) = expected_input {
        let mut checks = vec![0x10, 4, 0x42]; // input_length; i64.const expected length
        append_sleb(&mut checks, expected.len() as i64);
        checks.extend_from_slice(&[0x52, 0x04, 0x40, 0x00, 0x0b]); // i64.ne; if; trap; end
        for (offset, byte) in expected.iter().copied().enumerate() {
            checks.push(0x42); // i64.const offset
            append_sleb(&mut checks, offset as i64);
            checks.extend_from_slice(&[0x10, 3, 0x41]); // input_load_u8; i32.const expected byte
            append_sleb(&mut checks, i64::from(byte));
            checks.extend_from_slice(&[0x47, 0x04, 0x40, 0x00, 0x0b]); // i32.ne; if; trap; end
        }
        body.splice(3..3, checks); // After local declarations, before producing output.
    }
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
