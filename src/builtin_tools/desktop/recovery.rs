//! Failure→recovery-hint mapping for desktop tool errors.
//!
//! orca ships per-error-code "what to do next" guidance and it measurably
//! stops models from retrying a failed call unchanged. This is the A2
//! adoption clause in engineering form: compress the error AND the way out
//! into the tool result so the model can self-heal on the next turn.
//!
//! Matching operates on machine-generated error text (DesktopError displays,
//! bridge RPC messages) — not on natural language — so simple substring
//! routing is appropriate here (P8 does not apply).

/// Append a recovery hint to a failure message when a known category matches.
pub(super) fn with_hint(message: String) -> String {
    let lower = message.to_lowercase();
    let hint = if lower.contains("no element matches locator") {
        Some(
            "Run desktop_ax_snapshot to see current elements, then retry with \
             a role plus element_title, or add x/y as a nearest-center hint. \
             Do not retry unchanged.",
        )
    } else if lower.contains("window not found")
        || lower.contains("no window")
        || lower.contains("window operation failed")
    {
        Some(
            "Window ids go stale — re-run window_list and use a fresh id. \
             Do not retry unchanged.",
        )
    } else if lower.contains("permission denied") || lower.contains("not trusted") {
        Some(
            "A system permission is missing. Run desktop_check_permissions and \
             surface the guide steps to the user; do not retry until granted.",
        )
    } else if lower.contains("bridgedisabled")
        || lower.contains("bridge backoff")
        || lower.contains("bridgebackoff")
        || lower.contains("bridge disabled")
        || lower.contains("backing off")
    {
        Some(
            "The desktop helper is restarting. Wait a few seconds and retry \
             once; if it persists, fall back to screenshot + click.",
        )
    } else if lower.contains("notimplemented") || lower.contains("not implemented")
        || lower.contains("ax capability not available")
    {
        Some(
            "This platform has no accessibility write path — use screenshot \
             plus click / type_text instead.",
        )
    } else if lower.contains("blank frame") {
        Some(
            "The capture chain is dead, not the screen: it is locked or asleep, \
             the screen-recording grant was revoked, or the desktop helper is \
             wedged. Run desktop_check_permissions (and the doctor) and have the \
             user re-check screen recording; never treat a blank frame as the \
             screen's state.",
        )
    } else if lower.contains("read-only") || lower.contains("value_mismatch") {
        Some(
            "The element rejected the direct write — click it first, then use \
             type_text (select-all + type to replace existing content).",
        )
    } else {
        None
    };
    match hint {
        Some(h) => format!("{message} Hint: {h}"),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_hint_for_known_categories() {
        let out = with_hint("set_value failed: no element matches locator".into());
        assert!(out.contains("Hint:"));
        assert!(out.contains("desktop_ax_snapshot"));
    }

    #[test]
    fn leaves_unknown_messages_unchanged() {
        let msg = "some novel failure".to_string();
        assert_eq!(with_hint(msg.clone()), msg);
    }

    #[test]
    fn permission_denied_points_at_guide() {
        let out = with_hint("ax.query_tree RPC: permission denied: accessibility".into());
        assert!(out.contains("desktop_check_permissions"));
    }

    #[test]
    fn value_mismatch_points_at_type_text() {
        let out = with_hint(
            "Value written but read-back did not match (value_mismatch). \
             Re-observe before proceeding."
                .into(),
        );
        assert!(out.contains("Hint:"));
        assert!(out.contains("type_text"));
    }

    #[test]
    fn window_operation_failed_points_at_stale_ids() {
        let out = with_hint("window operation failed: window 123 not accessible".into());
        assert!(out.contains("Hint:"));
        assert!(out.contains("window_list"));
    }
}
