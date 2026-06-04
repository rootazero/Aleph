//! Classification of MCP tool-call failures into actionable kinds.
//!
//! When a tool call against an external MCP server fails, the underlying error
//! is an opaque string buried in a transport or JSON-RPC error — "401", "broken
//! pipe", "session expired", "connection reset" all surface the same way. The
//! reference implementation (hermes) distinguishes these so it can pick the
//! right recovery: re-authenticate on an auth failure, reconnect on a server-GC
//! session expiry, retry on a transient blip.
//!
//! This module ports that distinction as a typed enum — the failure mode is no
//! longer a stringly-typed guess at every call site. The classification feeds
//! structured diagnostics today (a `tracing` field plus an actionable hint
//! appended to user-visible errors) and gives a future reconnect/retry path a
//! single, tested decision point instead of scattered substring checks.

/// The recoverable category of an MCP failure, inferred from its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpErrorKind {
    /// Credentials are missing, invalid, or expired (HTTP 401/403, token errors).
    /// Recovery: re-authenticate the server.
    AuthExpired,
    /// The server discarded a previously-valid session (idle GC, restart, broken
    /// pipe). The credentials are still good. Recovery: reconnect the transport.
    SessionExpired,
    /// A temporary network condition (timeout, reset, refused, 5xx). Recovery:
    /// retry after backoff.
    Transient,
    /// No recognizable signal — surface as-is.
    Unknown,
}

impl McpErrorKind {
    /// Stable lowercase tag for structured logging and tests.
    pub fn as_str(&self) -> &'static str {
        match self {
            McpErrorKind::AuthExpired => "auth_expired",
            McpErrorKind::SessionExpired => "session_expired",
            McpErrorKind::Transient => "transient",
            McpErrorKind::Unknown => "unknown",
        }
    }

    /// A short, actionable hint for an operator or the agent. Only the
    /// recoverable kinds return guidance; `None` means "no useful hint, do not
    /// add noise to the error".
    pub fn guidance(&self) -> Option<&'static str> {
        match self {
            McpErrorKind::AuthExpired => {
                Some("the MCP server rejected credentials — re-authenticate it")
            }
            McpErrorKind::SessionExpired => {
                Some("the MCP session expired — it will reconnect on the next health probe")
            }
            McpErrorKind::Transient => {
                Some("a transient transport error — retry the call")
            }
            McpErrorKind::Unknown => None,
        }
    }

    /// `" (hint: …)"` suffix for appending to a user-visible error message, or
    /// an empty string when there is no actionable hint.
    pub fn guidance_suffix(&self) -> String {
        match self.guidance() {
            Some(hint) => format!(" (hint: {})", hint),
            None => String::new(),
        }
    }
}

/// Auth-failure markers. Checked first: an expired *token* must not be misread
/// as an expired *session*.
const AUTH_MARKERS: &[&str] = &[
    "401",
    "403",
    "unauthorized",
    "forbidden",
    "invalid token",
    "token expired",
    "expired token",
    "invalid_grant",
    "needs_reauth",
    "authentication failed",
    "access denied",
];

/// Server-side session-loss markers (credentials still valid).
const SESSION_MARKERS: &[&str] = &[
    "session expired",
    "session has expired",
    "invalid or expired session",
    "session not found",
    "closed resource",
    "broken pipe",
    "connection closed",
];

/// Transient network markers.
const TRANSIENT_MARKERS: &[&str] = &[
    "timeout",
    "timed out",
    "connection reset",
    "connection refused",
    "temporarily",
    "try again",
    "502",
    "503",
    "504",
];

/// Classify an MCP error message into a [`McpErrorKind`].
///
/// Pure and case-insensitive. Precedence is auth → session → transient → unknown
/// so the most specific, action-changing categories win.
pub fn classify_mcp_error(message: &str) -> McpErrorKind {
    let haystack = message.to_lowercase();
    let any = |markers: &[&str]| markers.iter().any(|m| haystack.contains(m));

    if any(AUTH_MARKERS) {
        McpErrorKind::AuthExpired
    } else if any(SESSION_MARKERS) {
        McpErrorKind::SessionExpired
    } else if any(TRANSIENT_MARKERS) {
        McpErrorKind::Transient
    } else {
        McpErrorKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_auth_failures() {
        assert_eq!(classify_mcp_error("HTTP 401 Unauthorized"), McpErrorKind::AuthExpired);
        assert_eq!(classify_mcp_error("token expired"), McpErrorKind::AuthExpired);
        assert_eq!(classify_mcp_error("invalid_grant"), McpErrorKind::AuthExpired);
    }

    #[test]
    fn classifies_session_expiry() {
        assert_eq!(
            classify_mcp_error("Server reports: session expired"),
            McpErrorKind::SessionExpired
        );
        assert_eq!(classify_mcp_error("broken pipe"), McpErrorKind::SessionExpired);
    }

    #[test]
    fn auth_wins_over_session_for_token_expiry() {
        // "token expired" must classify as auth, not session, despite "expired".
        assert_eq!(classify_mcp_error("the token expired"), McpErrorKind::AuthExpired);
    }

    #[test]
    fn classifies_transient() {
        assert_eq!(classify_mcp_error("request timed out"), McpErrorKind::Transient);
        assert_eq!(classify_mcp_error("connection reset by peer"), McpErrorKind::Transient);
        assert_eq!(classify_mcp_error("502 Bad Gateway"), McpErrorKind::Transient);
    }

    #[test]
    fn unknown_has_no_guidance() {
        let kind = classify_mcp_error("tool returned: file not found");
        assert_eq!(kind, McpErrorKind::Unknown);
        assert_eq!(kind.guidance(), None);
        assert_eq!(kind.guidance_suffix(), "");
    }

    #[test]
    fn recoverable_kinds_have_guidance_suffix() {
        assert!(McpErrorKind::AuthExpired.guidance_suffix().contains("re-authenticate"));
        assert!(McpErrorKind::SessionExpired.guidance_suffix().starts_with(" (hint:"));
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(classify_mcp_error("UNAUTHORIZED"), McpErrorKind::AuthExpired);
    }
}
