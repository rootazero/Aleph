//! P5: Memory filter tests.
//!
//! Covers session-scoped memory retrieval and cross-session isolation.

use alephcore::memory::store::types::MemoryFilter;

/// P5-01: No session_ids field → no filter clause generated.
#[test]
fn p5_01_no_session_ids_no_filter_clause() {
    let f = MemoryFilter {
        session_ids: None,
        ..Default::default()
    };
    assert!(f.to_lance_filter().is_none());
}

/// P5-02: Empty session_ids vec → no filter clause generated.
#[test]
fn p5_02_empty_session_ids_no_filter_clause() {
    let f = MemoryFilter {
        session_ids: Some(vec![]),
        ..Default::default()
    };
    assert!(f.to_lance_filter().is_none());
}

/// P5-03: Single session ID generates an IN clause containing that ID.
#[test]
fn p5_03_single_session_id_generates_in_clause() {
    let f = MemoryFilter {
        session_ids: Some(vec!["agent:main:main".to_string()]),
        ..Default::default()
    };
    let sql = f.to_lance_filter().unwrap();
    assert!(sql.contains("session_id IN"), "expected IN clause, got: {sql}");
    assert!(sql.contains("agent:main:main"), "expected session id in SQL, got: {sql}");
}

/// P5-04: Multiple session IDs all appear in the generated IN clause.
#[test]
fn p5_04_multiple_session_ids_generate_in_clause() {
    let f = MemoryFilter {
        session_ids: Some(vec![
            "agent:main:main".to_string(),
            "agent:main:main:s1".to_string(),
            "agent:main:main:s2".to_string(),
        ]),
        ..Default::default()
    };
    let sql = f.to_lance_filter().unwrap();
    assert!(sql.contains("session_id IN"), "expected IN clause, got: {sql}");
    assert!(sql.contains("agent:main:main"), "missing base id in: {sql}");
    assert!(sql.contains(":s1"), "missing :s1 in: {sql}");
    assert!(sql.contains(":s2"), "missing :s2 in: {sql}");
}

/// P5-05: Session ID with single quote is SQL-escaped (O'Brien → O''Brien).
#[test]
fn p5_05_session_ids_with_special_chars_are_escaped() {
    let f = MemoryFilter {
        session_ids: Some(vec!["O'Brien".to_string()]),
        ..Default::default()
    };
    let sql = f.to_lance_filter().unwrap();
    assert!(sql.contains("O''Brien"), "single quote not escaped in: {sql}");
    // Must not contain un-escaped single-interior quote
    assert!(!sql.contains("O'Brien'"), "raw quote should be escaped in: {sql}");
}
