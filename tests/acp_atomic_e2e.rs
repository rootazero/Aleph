//! Spec C Task 24 (part 3): `acp_sessions.json` writes go through
//! `atomic_io::write_atomic` (Task 9), so concurrent CLI/server
//! processes never observe a half-written JSON file.
//!
//! We exercise the same write path by hand and assert the result
//! parses as valid JSON. (Task 24 part 2 already covers the
//! concurrent serialisation invariant for vault; the same temp+rename
//! primitive backs both.)

#[test]
fn acp_sessions_atomic_write_yields_valid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("acp_sessions.json");

    alephcore::utils::atomic_io::write_atomic(
        &path,
        br#"[{"session_id":"abc","cwd":"/tmp"}]"#,
    )
    .expect("atomic write");

    let bytes = std::fs::read_to_string(&path).expect("read back");
    let parsed: serde_json::Value = serde_json::from_str(&bytes).expect("valid json");
    assert!(parsed.is_array(), "expected json array, got: {parsed}");
    assert_eq!(
        parsed[0]["session_id"].as_str(),
        Some("abc"),
        "expected session_id=abc, got: {parsed}"
    );
}
