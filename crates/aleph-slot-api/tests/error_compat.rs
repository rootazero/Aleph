//! Golden compatibility tests for `aleph-slot-api` — Task 1 closure.
//!
//! These tests pin behaviour that downstream consumers (alephcore, panel,
//! harness) rely on: `Display` strings, classification predicates, helper
//! call paths, and `From` impl prefixes. They are written **before** the
//! crate exists on purpose — the first run MUST fail with a package-missing
//! error, which we record as an INTEGRATION RED (not a behaviour assertion
//! red) so reviewers can tell the difference between "the crate is absent"
//! and "the crate behaves wrong".
//!
//! Once the crate ships, the same tests must go GREEN with the *exact* values
//! pinned here. Any change to these literals is a deliberate API break and
//! belongs in a separate, explicitly-named proposal — not in this closure.

// =========================================================================
// 1. Display strings — pin verbatim against `src/error.rs` pre-migration.
// =========================================================================

#[test]
fn clipboard_display_pins_access_denied_string() {
    let err = aleph_slot_api::AlephError::clipboard("access denied");
    assert_eq!(err.to_string(), "Clipboard error: access denied");
}

#[test]
fn cancelled_display_pins_phrase() {
    // Brief names this explicitly; the helper used to be a tuple variant but
    // the canonical constructor (`AlephError::cancelled`) returns `Cancelled`.
    let err = aleph_slot_api::AlephError::cancelled();
    assert_eq!(err.to_string(), "Operation cancelled");
}

#[test]
fn invalid_response_display_pins_prefix() {
    let err = aleph_slot_api::AlephError::invalid_response("not json");
    assert_eq!(err.to_string(), "Invalid response: not json");
}

#[test]
fn request_rejected_display_pins_prefix() {
    let err = aleph_slot_api::AlephError::request_rejected("bad param");
    assert_eq!(err.to_string(), "Request rejected: bad param");
}

// =========================================================================
// 2. ErrorClass mapping — exhaustive, no wildcard arm.
// =========================================================================

#[test]
fn cancelled_class_is_recoverable() {
    let err = aleph_slot_api::AlephError::cancelled();
    assert_eq!(err.class(), aleph_slot_api::ErrorClass::Recoverable);
}

#[test]
fn invalid_response_class_is_transient() {
    let err = aleph_slot_api::AlephError::invalid_response("html");
    assert_eq!(err.class(), aleph_slot_api::ErrorClass::Transient);
}

#[test]
fn request_rejected_class_is_fixable() {
    let err = aleph_slot_api::AlephError::request_rejected("400");
    assert_eq!(err.class(), aleph_slot_api::ErrorClass::Fixable);
}

// =========================================================================
// 3. `is_transient` — narrower than `class() == Transient` on purpose.
//    Brief: do NOT widen this to ProviderError or McpTimeout.
// =========================================================================

#[test]
fn provider_error_is_not_transient() {
    let err = aleph_slot_api::AlephError::provider("API returned 500");
    assert!(!err.is_transient());
}

#[test]
fn mcp_timeout_is_not_transient() {
    let err = aleph_slot_api::AlephError::McpTimeout;
    assert!(!err.is_transient());
}

#[test]
fn rate_limit_is_transient() {
    let err = aleph_slot_api::AlephError::rate_limit("429");
    assert!(err.is_transient());
}

#[test]
fn network_error_is_transient() {
    let err = aleph_slot_api::AlephError::network("reset");
    assert!(err.is_transient());
}

// =========================================================================
// 4. From impls — JSON prefix must be `"JSON serialization error: "`
//    (NOT `"Serde error..."`), and `std::io::Error` is plain `to_string`.
// =========================================================================

#[test]
fn from_serde_json_keeps_literal_english_prefix() {
    // Force a real `serde_json::Error`. `from_str` needs an explicit target
    // type because the literal `"not json"` is itself valid for any type
    // whose deserializer short-circuits on bad syntax — and rustc refuses to
    // guess.
    let jerr: serde_json::Error =
        serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
    let err: aleph_slot_api::AlephError = jerr.into();
    let display = err.to_string();
    // The prefix is the load-bearing bit; the suffix contains rustc's own
    // diagnostic and would change if serde_json renumbered its codes.
    assert!(
        display.starts_with("I/O error: JSON serialization error: "),
        "unexpected from-serde Display: {display}"
    );
}

#[test]
fn from_io_error_uses_plain_to_string() {
    let ioerr = std::io::Error::other("boom");
    let err: aleph_slot_api::AlephError = ioerr.into();
    assert_eq!(err.to_string(), "I/O error: boom");
}

// =========================================================================
// 5. AlephException — three From impls, all collapse to unit variant.
// =========================================================================

#[test]
fn exception_from_aleph_error_collapses() {
    let e = aleph_slot_api::AlephError::cancelled();
    let ex: aleph_slot_api::AlephException = e.into();
    // `AlephException` does NOT derive PartialEq (UniFFI 0.25 keeps the
    // type minimal), so use `matches!` instead of `assert_eq!`.
    assert!(matches!(ex, aleph_slot_api::AlephException::Error));
}

#[test]
fn exception_from_string_collapses() {
    let ex: aleph_slot_api::AlephException = String::from("anything").into();
    assert!(matches!(ex, aleph_slot_api::AlephException::Error));
}

#[test]
fn exception_from_str_collapses() {
    let ex: aleph_slot_api::AlephException = "anything".into();
    assert!(matches!(ex, aleph_slot_api::AlephException::Error));
}

// =========================================================================
// 6. UTF-8 truncate_text — soft cap, char boundaries, "..." appended.
// =========================================================================

#[test]
fn truncate_text_chinese_at_limit_two_appends_marker() {
    // "中" U+4E2D (3 bytes), "文" U+6587 (3 bytes), "🙂" U+1F642 (4 bytes).
    // limit=2 → first 2 chars ("中文") + "..." → "中文...".
    let out = aleph_slot_api::text::truncate_text("中文🙂", 2);
    assert_eq!(out, "中文...");
}

#[test]
fn truncate_text_limit_zero_yields_empty_string() {
    // Brief explicitly names this; do NOT let it become "...".
    let out = aleph_slot_api::text::truncate_text("anything", 0);
    assert_eq!(out, "");
}

#[test]
fn truncate_text_under_limit_returns_input_unchanged() {
    let out = aleph_slot_api::text::truncate_text("hi", 100);
    assert_eq!(out, "hi");
}

#[test]
fn truncate_text_chars_count_not_bytes() {
    // 4 chars / 12 bytes — under a char-based `max = 8`, no cut.
    let text = "你好世界";
    let out = aleph_slot_api::text::truncate_text(text, 8);
    assert_eq!(out, text);
    assert!(!out.ends_with("..."));
}

// =========================================================================
// 7. MissingInput uses 100 CHARACTERS, not 100 bytes.
// =========================================================================

#[test]
fn missing_input_truncates_at_one_hundred_characters_not_bytes() {
    // Build a string whose CHAR count is 100 but whose BYTE count is 300
    // (3 bytes per CJK char). Truncating at 100 chars must NOT cut anything,
    // because the message is exactly 100 chars long. Brief explicitly says
    // "不能改成 100 bytes".
    let message: String = "中".repeat(100); // 100 chars, 300 bytes
    let err = aleph_slot_api::AlephError::MissingInput {
        task_id: "task-1".to_string(),
        task_name: "测试".to_string(),
        message,
    };
    let rendered = err.user_friendly_message();
    // The full 100-char message should be embedded (no ellipsis), and the
    // Chinese task_name should survive verbatim.
    assert!(rendered.contains("测试"), "task_name dropped: {rendered}");
    assert!(
        rendered.contains(&"中".repeat(100)),
        "message was byte-truncated to 100 bytes (300 chars → 100 bytes would be 33 chars): {rendered}"
    );
}
