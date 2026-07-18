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
    let base = format_capability_summary(program, caps);
    match crate::sandbox::context::current_justification().and_then(sanitize_justification) {
        Some(why) => format!("{base}\n— justification: {why}"),
        None => base,
    }
}

/// The capability portion of the request — deterministic, and free of the
/// model-controlled `justification`. This is what the denial ledger / grant
/// fingerprint MUST key on: folding the justification into the key would let the
/// model mint a fresh fingerprint for the same escalation just by rewording its
/// reason, defeating the blind-retry guard. Pass a `normalized()` capability set
/// so `fs_read`/`fs_write` path order does not perturb the key.
pub(crate) fn format_capability_summary(program: &str, caps: &SandboxCapabilities) -> String {
    let mut parts = Vec::new();
    if !caps.fs_read.is_empty() {
        parts.push(format!("fs_read={:?}", caps.fs_read));
    }
    if !caps.fs_write.is_empty() {
        parts.push(format!("fs_write={:?}", caps.fs_write));
    }
    match &caps.network {
        crate::sandbox::capabilities::NetworkPolicy::None => {}
        crate::sandbox::capabilities::NetworkPolicy::AllowHosts { hosts } => {
            // Sort the hosts so the fingerprint is order-independent — the
            // network sibling of `normalized()`'s fs_read/fs_write sort, which
            // does NOT touch the host list. Without this, a reordered AllowHosts
            // set renders a different summary and mints a fresh denial-ledger
            // fingerprint, letting the model re-prompt the user for an elevation
            // already denied this session (blind-retry / fatigue-loop bypass).
            let mut sorted = hosts.clone();
            sorted.sort();
            parts.push(format!(
                "network={:?}",
                crate::sandbox::capabilities::NetworkPolicy::AllowHosts { hosts: sorted }
            ));
        }
        other => parts.push(format!("network={other:?}")),
    }
    if caps.spawn_subprocess {
        parts.push("spawn=true".into());
    }
    format!(
        "{program} requests elevated capabilities: {}",
        parts.join(", ")
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::NetworkPolicy;
    use crate::sandbox::context::EXEC_JUSTIFICATION;

    fn net_caps() -> SandboxCapabilities {
        SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        }
    }

    #[tokio::test]
    async fn summary_excludes_justification_that_request_includes() {
        let caps = net_caps();
        EXEC_JUSTIFICATION
            .scope("fetch the changelog".to_string(), async {
                let request = format_capability_request("curl", &caps);
                let summary = format_capability_summary("curl", &caps);
                // Human-facing request surfaces WHY, for the approver.
                assert!(
                    request.contains("fetch the changelog"),
                    "request must surface the justification: {request}"
                );
                // The fingerprint input must NOT — else rewording the reason
                // mints a fresh key and defeats the denial ledger.
                assert!(
                    !summary.contains("fetch the changelog"),
                    "summary must be justification-free: {summary}"
                );
            })
            .await;
    }
}
