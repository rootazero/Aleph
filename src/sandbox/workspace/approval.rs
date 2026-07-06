//! Capability approval request formatting for the workspace sandbox.

use crate::sandbox::capabilities::SandboxCapabilities;

/// Format a user-facing capability elevation request string. Used as the
/// `reason` argument to `ApprovalGate::request_approval_for_tool`.
///
/// The capability portion is unchanged (WHAT is requested). When the model
/// supplied a `justification` for this exec call (carried via the
/// [`EXEC_JUSTIFICATION`](crate::sandbox::context::EXEC_JUSTIFICATION)
/// task-local), we append a `— justification: …` clause so the human approver
/// also sees WHY — codex `justification` / hermes reason parity. Absent a
/// justification the string is byte-identical to its pre-feature form.
pub(crate) fn format_capability_request(program: &str, caps: &SandboxCapabilities) -> String {
    let mut parts = Vec::new();
    if !caps.fs_read.is_empty() {
        parts.push(format!("fs_read={:?}", caps.fs_read));
    }
    if !caps.fs_write.is_empty() {
        parts.push(format!("fs_write={:?}", caps.fs_write));
    }
    if caps.network != crate::sandbox::capabilities::NetworkPolicy::None {
        parts.push(format!("network={:?}", caps.network));
    }
    if caps.spawn_subprocess {
        parts.push("spawn=true".into());
    }
    let base = format!(
        "{program} requests elevated capabilities: {}",
        parts.join(", ")
    );
    match crate::sandbox::context::current_justification().and_then(sanitize_justification) {
        Some(why) => format!("{base}\n— justification: {why}"),
        None => base,
    }
}

/// Longest model justification we surface in an approval prompt. The reason is
/// a hint for the human, not a contract — a runaway string is clamped.
pub(crate) const JUSTIFICATION_MAX_CHARS: usize = 240;

/// Clean a model-supplied justification for display in the approval prompt.
/// The text is untrusted model output, so collapse control characters /
/// newlines to single spaces (keep the prompt one logical line) and clamp the
/// length. Returns `None` when nothing meaningful remains, so the caller falls
/// back to the byte-identical capabilities-only string.
pub(crate) fn sanitize_justification(raw: String) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    // UTF-8-safe clamp (P7): take whole chars, never split a codepoint.
    let clamped: String = collapsed.chars().take(JUSTIFICATION_MAX_CHARS).collect();
    Some(clamped)
}
