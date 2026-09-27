//! Discord command & component codec + dispatch table (T2.2).
//!
//! Discord's interaction API exposes two distinct shapes:
//!
//! - **Slash commands** (`Interaction::Command`) — the user typed `/foo` and
//!   Discord sent us `data.name == "foo"` plus a typed `options` list. The
//!   channel already forwards slash commands as inbound messages via
//!   `Handler::interaction_create` in `mod.rs`; this module exposes the codec
//!   half (parsing the `custom_id`) and the dispatch half (deciding what to
//!   do with it) so the registration surface has one home.
//! - **Components** (`Interaction::Component`) — buttons and select menus
//!   carry a `custom_id` string the bot picks. The handler currently treats
//!   every `custom_id` as opaque (the approval UI's `cb_<message_id>` shape
//!   is the only thing that gets handled today). This module formalises the
//!   codec so adding a new button kind doesn't require editing the handler's
//!   dispatch — add a `ComponentKind` variant, decode it here, match in the
//!   dispatch table.
//!
//! ## Why a typed codec and not a string registry?
//!
//! - **Bots live longer than the strings that name them.** `ComponentId`
//!   survives a rename because it parses the wire format, not the human
//!   label. Renaming `ComponentKind::ApprovalDeny` from `"deny"` to
//!   `"decline"` only requires updating the wire-format literal; the rest of
//!   the codebase keeps matching on the variant.
//! - **Tests can assert on the variant.** A string registry forces tests to
//!   know the exact wire format — `"approve:abc"` vs `"approve abc"` vs
//!   `"approval:approve:abc"` becomes a string-matching exercise that doesn't
//!   catch the off-by-one errors the codec exists to prevent.
//!
//! ## Wire format
//!
//! ```text
//! <kind>:<payload...>
//! ```
//!
//! `kind` is one of the `ComponentKind::as_str()` values (always lowercase
//! ASCII, no colons). `payload` is the rest of the string verbatim and its
//! shape depends on `kind`. `ComponentId::parse` rejects empty payloads and
//! fields containing colons (so the first colon is always the kind/payload
//! boundary).

use std::fmt;

use crate::security::audit::{global as audit_global, AuditEntry, AuditEventType};

/// Emit an `AuthorityChange` audit row for a slash-command dispatch.
///
/// `action` is one of `"dispatch"`, `"ack"`, etc — the verb in the audit
/// row is the audit producer's responsibility to keep stable across
/// versions. `channel_id` is the channel that hosted the command; we
/// always include it so a post-incident query can scope to a single
/// surface (a guild channel, a DM, etc.).
///
/// `actor_user` is the slash-command author (Discord user id as a string).
/// We pass `None` when the caller is unauthenticated (only happens in
/// tests).
pub(crate) fn audit_command_dispatch(
    actor_user: Option<&str>,
    channel_id: &str,
    command_name: &str,
) {
    let Some(log) = audit_global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let detail = format!(
        "discord.command.dispatch: {command_name} channel={channel_id}"
    );
    tokio::spawn(async move {
        let _ = log.log(AuditEntry::authority_change(actor, detail)).await;
    });
}

/// Emit an `ExecBlocked` audit row when a slash command hits a permission
/// gate (channel allowlist miss, DM-while-disabled, etc.).
///
/// `reason` is the human-readable reason surfaced in the audit row; the
/// caller should include enough detail for an operator to reconstruct the
/// decision (e.g. "guild 1234 not in allowlist"). Severity is `Warn`
/// (matches the rest of the `ExecBlocked` producers).
pub(crate) fn audit_command_blocked(
    actor_user: Option<&str>,
    channel_id: &str,
    reason: &str,
) {
    let Some(log) = audit_global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let detail = format!(
        "discord.command.blocked: {reason} channel={channel_id}"
    );
    tokio::spawn(async move {
        let _ = log
            .log(AuditEntry {
                event_type: AuditEventType::ExecBlocked,
                severity: crate::security::audit::AuditSeverity::Warn,
                source_ip: None,
                session_id: None,
                actor_user: actor,
                detail,
            })
            .await;
    });
}

/// Closed enum of every interaction custom_id shape the channel knows how
/// to dispatch. Adding a new button kind is a one-line addition here; the
/// dispatch table in [`CommandRegistry::dispatch`] picks it up automatically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    /// Approval UI button. `payload` is the message id the approval gate
    /// handed out when it posted the prompt. The router recognises this by
    /// the `cb_` prefix — the codec keeps the surface explicit.
    ApprovalApprove,
    /// Approval UI button — deny path.
    ApprovalDeny,
    /// Generic command callback (future). Reserved for skill/menu callbacks
    /// that don't carry approval semantics; not yet dispatched.
    Callback,
    /// Unknown kind — preserves the raw `payload` for forensic logging. The
    /// dispatch table returns `None` for these so the handler falls back to
    /// the existing approval-sink path on `cb_`-prefixed payloads.
    Unknown(String),
}

impl ComponentKind {
    /// Wire-format literal for this kind. `Callback` has no fixed literal —
    /// it accepts any non-prefixed payload, but the codec still records the
    /// variant for tests.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::ApprovalApprove => "approve",
            Self::ApprovalDeny => "deny",
            Self::Callback => "callback",
            Self::Unknown(_) => "unknown",
        }
    }

    /// Inverse of [`as_str`](Self::as_str). Returns `Unknown(kind)` for any
    /// string the codec doesn't recognise — explicit so the dispatch table
    /// can match on it.
    #[must_use]
    pub fn from_kind(kind: &str) -> Self {
        match kind {
            "approve" => Self::ApprovalApprove,
            "deny" => Self::ApprovalDeny,
            "callback" => Self::Callback,
            other => Self::Unknown(other.to_string()),
        }
    }
}

impl fmt::Display for ComponentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parsed representation of a Discord interaction `custom_id`. Wraps the
/// kind + payload so the dispatch table can match on the kind without
/// re-parsing the wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentId {
    pub kind: ComponentKind,
    pub payload: String,
}

impl ComponentId {
    /// Parse a Discord `custom_id`. Empty strings and strings without a
    /// `:` separator are rejected (`Err`) — the codec treats those as
    /// malformed so a stray `Approve` button click with no payload doesn't
    /// silently fall through to the approval sink.
    pub fn parse(custom_id: &str) -> Result<Self, ComponentParseError> {
        if custom_id.is_empty() {
            return Err(ComponentParseError::Empty);
        }
        let (kind_str, payload) = match custom_id.split_once(':') {
            Some((k, p)) => (k, p),
            None => {
                // No `:` at all — treat the whole string as the kind and
                // empty the payload. This matches Discord's "raw callback"
                // pattern (a button that carries no parameters).
                return Ok(Self {
                    kind: ComponentKind::Unknown(custom_id.to_string()),
                    payload: String::new(),
                });
            }
        };
        if kind_str.is_empty() {
            return Err(ComponentParseError::EmptyKind);
        }
        if payload.is_empty() {
            return Err(ComponentParseError::EmptyPayload);
        }
        Ok(Self {
            kind: ComponentKind::from_kind(kind_str),
            payload: payload.to_string(),
        })
    }

    /// Reconstruct the wire format. Used by tests + the approval UI when it
    /// has to echo a `ComponentId` back into a follow-up click.
    pub fn to_wire(&self) -> String {
        format!("{}:{}", self.kind, self.payload)
    }

    /// Convenience: is this an approval-style callback?
    #[must_use]
    pub fn is_approval(&self) -> bool {
        matches!(self.kind, ComponentKind::ApprovalApprove | ComponentKind::ApprovalDeny)
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

/// Why a `custom_id` was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentParseError {
    /// The custom_id was empty.
    Empty,
    /// The custom_id had a `:` but the kind segment before it was empty
    /// (`":payload"`).
    EmptyKind,
    /// The custom_id had a `:` but the payload after it was empty
    /// (`"approve:"`).
    EmptyPayload,
}

impl fmt::Display for ComponentParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            Self::Empty => "custom_id is empty",
            Self::EmptyKind => "custom_id has empty kind (':payload')",
            Self::EmptyPayload => "custom_id has empty payload ('approve:')",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ComponentParseError {}

/// What a registered handler decided to do with a parsed `ComponentId`.
/// Kept tiny so the dispatch table is easy to read at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Hand off to the approval sink. `payload` is the message id the
    /// approval gate posted; the inbound message gets the `cb_` prefix and
    /// the existing router picks it up.
    ForwardToApproval,
    /// No handler registered; the interaction should be ACKed without any
    /// follow-up message (the click is recorded in the audit trail but the
    /// user sees no response). The dispatch table returns this for
    /// `Callback` / `Unknown` until a future round adds handlers.
    AckNoReply,
    /// The codec rejected the input. The dispatch table doesn't see this —
    /// `ComponentId::parse` already failed. Exists so future callers can
    /// route parse errors through the same `Result` shape.
    Rejected(ComponentParseError),
}

/// A registered handler for a single `ComponentKind`. The handler is a
/// closure the registry stores; the actual business logic lives at the
/// call site (commands.rs is the codec + dispatch table, not the place
/// that posts the response).
pub type ComponentHandler = Box<dyn Fn(&ComponentId) -> DispatchOutcome + Send + Sync>;

/// Dispatch table keyed by `ComponentKind`. `Callback` is a single entry
/// that matches all `Unknown` kinds too (callers can register the same
/// handler for both by registering once and dispatching on the kind
/// themselves), and `Unknown` has no slot — the codec preserves the raw
/// kind string in `ComponentKind::Unknown(String)` so the handler can
/// pattern-match.
pub struct CommandRegistry {
    handlers: std::collections::HashMap<ComponentKind, ComponentHandler>,
}

impl CommandRegistry {
    /// Build an empty registry. `with_defaults()` wires the two approval
    /// kinds so a freshly-constructed registry already routes the existing
    /// approval buttons.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: std::collections::HashMap::new(),
        }
    }

    /// Build a registry with the two approval handlers wired to
    /// `ForwardToApproval`. Tests that don't care about approval behavior
    /// use `new()`; the production bootstrap calls `with_defaults()`.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.handlers.insert(
            ComponentKind::ApprovalApprove,
            Box::new(|_| DispatchOutcome::ForwardToApproval),
        );
        r.handlers.insert(
            ComponentKind::ApprovalDeny,
            Box::new(|_| DispatchOutcome::ForwardToApproval),
        );
        r
    }

    /// Register a handler for a single `ComponentKind`. Replaces any
    /// previous registration.
    pub fn register(&mut self, kind: ComponentKind, handler: ComponentHandler) {
        self.handlers.insert(kind, handler);
    }

    /// Dispatch one parsed `ComponentId` through the table. Returns `None`
    /// when no handler is registered (the call site then ACKs the click
    /// without replying — Discord requires every interaction to be
    /// answered within 3s or it shows an error spinner).
    #[must_use]
    pub fn dispatch(&self, id: &ComponentId) -> Option<DispatchOutcome> {
        let outcome = self
            .handlers
            .get(&id.kind)
            .map(|h| h(id))
            .or_else(|| {
                // Unknown kinds fall back to AckNoReply unless a handler
                // is explicitly registered for `Callback`.
                self.handlers
                    .get(&ComponentKind::Callback)
                    .map(|h| h(id))
            });
        // Record every dispatch so the trail captures unknown-kind fallbacks.
        audit_command_dispatch(
            None,
            id.kind.as_str(),
            id.payload.as_str(),
        );
        outcome
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approval_approve() {
        let id = ComponentId::parse("approve:msg-abc-123").unwrap();
        assert_eq!(id.kind, ComponentKind::ApprovalApprove);
        assert_eq!(id.payload, "msg-abc-123");
        assert!(id.is_approval());
    }

    #[test]
    fn parse_approval_deny() {
        let id = ComponentId::parse("deny:msg-abc-123").unwrap();
        assert_eq!(id.kind, ComponentKind::ApprovalDeny);
        assert_eq!(id.payload, "msg-abc-123");
        assert!(id.is_approval());
    }

    #[test]
    fn parse_unknown_kind_is_preserved() {
        let id = ComponentId::parse("mystery:x").unwrap();
        assert_eq!(
            id.kind,
            ComponentKind::Unknown("mystery".to_string())
        );
        assert_eq!(id.payload, "x");
    }

    #[test]
    fn parse_rejects_empty_string() {
        assert_eq!(
            ComponentId::parse("").unwrap_err(),
            ComponentParseError::Empty
        );
    }

    #[test]
    fn parse_rejects_empty_payload() {
        assert_eq!(
            ComponentId::parse("approve:").unwrap_err(),
            ComponentParseError::EmptyPayload
        );
    }

    #[test]
    fn parse_rejects_empty_kind() {
        assert_eq!(
            ComponentId::parse(":payload").unwrap_err(),
            ComponentParseError::EmptyKind
        );
    }

    #[test]
    fn parse_no_colon_yields_unknown_with_empty_payload() {
        // Bare strings are forwarded as `Unknown(kind)` with an empty
        // payload — matches Discord's "click with no parameters" pattern.
        let id = ComponentId::parse("bare").unwrap();
        assert_eq!(id.kind, ComponentKind::Unknown("bare".to_string()));
        assert_eq!(id.payload, "");
    }

    #[test]
    fn roundtrip_via_to_wire() {
        let id = ComponentId::parse("approve:msg-xyz").unwrap();
        assert_eq!(id.to_wire(), "approve:msg-xyz");
        // Re-parsing the wire form yields the same id.
        let again = ComponentId::parse(&id.to_wire()).unwrap();
        assert_eq!(id, again);
    }

    #[test]
    fn registry_defaults_route_approval() {
        let r = CommandRegistry::with_defaults();
        let id = ComponentId::parse("approve:m1").unwrap();
        assert_eq!(
            r.dispatch(&id),
            Some(DispatchOutcome::ForwardToApproval)
        );
        let id = ComponentId::parse("deny:m1").unwrap();
        assert_eq!(
            r.dispatch(&id),
            Some(DispatchOutcome::ForwardToApproval)
        );
    }

    #[test]
    fn registry_unknown_kind_falls_back_to_ack() {
        let r = CommandRegistry::with_defaults();
        let id = ComponentId::parse("mystery:x").unwrap();
        assert_eq!(r.dispatch(&id), Some(DispatchOutcome::AckNoReply));
    }

    #[test]
    fn registry_unknown_kind_callback_handler_used() {
        // Register a callback handler — Unknown kinds should defer to it.
        let mut r = CommandRegistry::new();
        r.register(
            ComponentKind::Callback,
            Box::new(|_| DispatchOutcome::AckNoReply),
        );
        let id = ComponentId::parse("future-thing:x").unwrap();
        assert_eq!(r.dispatch(&id), Some(DispatchOutcome::AckNoReply));
    }

    #[test]
    fn registry_unknown_kind_without_callback_returns_none() {
        // Empty registry: Unknown kinds return None so the call site
        // decides (today: ACK without reply).
        let r = CommandRegistry::new();
        let id = ComponentId::parse("future-thing:x").unwrap();
        assert_eq!(r.dispatch(&id), None);
    }

    #[test]
    fn parse_payload_preserves_inner_colons() {
        // Only the FIRST colon is the kind/payload boundary; later colons
        // are part of the payload (e.g. message ids that happen to be
        // channel:msg tuples).
        let id = ComponentId::parse("approve:chan:msg-1").unwrap();
        assert_eq!(id.kind, ComponentKind::ApprovalApprove);
        assert_eq!(id.payload, "chan:msg-1");
    }
}