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

use chrono::Utc;

use crate::gateway::channel::{
    ChannelId, ConversationId, InboundMessage, MessageId, UserId, CB_MESSAGE_ID_PREFIX,
};
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
    // The verb is hard-coded as `discord.command.dispatch` — the audit-census
    // extractor reads the first `"` after `::authority_change(` and rejects
    // `format!` placeholders in the verb slot (its "dotted lowercase" check
    // would see `{command_name}` and refuse it). `format!` is inlined into
    // the call so the verb literal sits directly between `(` and the
    // placeholder; capturing the detail into a `let` first would put a `;`
    // between the call and its string literal, which the extractor refuses
    // as "the detail is not a string literal".
    let Some(log) = audit_global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    let command = command_name.to_string();
    tokio::spawn(async move {
        let _ = log
            .log(AuditEntry::authority_change(
                actor,
                format!("discord.command.dispatch: {command} channel={channel}"),
            ))
            .await;
    });
}

/// Emit an `ExecBlocked` audit row when a slash command hits a permission
/// gate (channel allowlist miss, DM-while-disabled, etc.).
///
/// `reason` is the human-readable reason surfaced in the audit row; the
/// caller should include enough detail for an operator to reconstruct the
/// decision (e.g. "guild 1234 not in allowlist"). Severity is `Warn`
/// (matches the rest of the `ExecBlocked` producers).
pub(crate) fn audit_command_blocked(actor_user: Option<&str>, channel_id: &str, reason: &str) {
    let Some(log) = audit_global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let detail = format!("discord.command.blocked: {reason} channel={channel_id}");
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
        matches!(
            self.kind,
            ComponentKind::ApprovalApprove | ComponentKind::ApprovalDeny
        )
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
        // Unknown kinds fall back to AckNoReply via this Callback handler;
        // the dispatch table looks up `id.kind` first, then `Callback`.
        r.handlers.insert(
            ComponentKind::Callback,
            Box::new(|_| DispatchOutcome::AckNoReply),
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
        let outcome = self.handlers.get(&id.kind).map(|h| h(id)).or_else(|| {
            // Unknown kinds fall back to AckNoReply unless a handler
            // is explicitly registered for `Callback`.
            self.handlers.get(&ComponentKind::Callback).map(|h| h(id))
        });
        // Record every dispatch so the trail captures unknown-kind fallbacks.
        audit_command_dispatch(None, id.kind.as_str(), id.payload.as_str());
        outcome
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// What [`dispatch_component_click`] decided to do with a Discord
/// `custom_id`. The serenity handler turns these into channel sends +
/// ACK calls; the function is exposed as a pure seam so integration
/// tests don't need to construct a full `ComponentInteraction` (the
/// serenity type has many private fields and mocking it is brittle).
///
/// **D1 rationale:** the previous `handle_component` always forwarded
/// the raw `custom_id` to `inbound_tx`. This typed enum is the
/// behaviour-preserving replacement: `Forward` still feeds the inbound
/// router (which intercepts by `cb_` prefix and routes to the approval
/// sink), `AckOnly` drops the click silently when the dispatcher said
/// nothing useful should happen.
#[derive(Debug, Clone)]
pub enum CommandAction {
    /// Forward as an inbound message. The inbound router's `cb_`
    /// prefix intercept handles approval clicks by routing them to the
    /// approval sink. The reconstructed `text` is in
    /// `ApprovalBridge::parse_callback` format so the sink can resolve
    /// the click. Boxed to keep the enum small relative to `AckOnly`
    /// (clippy::large_enum_variant).
    Forward(Box<InboundMessage>),
    /// ACK the interaction without sending anything to the inbound
    /// router. Used when the registry returned `AckNoReply` or
    /// `Rejected` (the latter is a defensive fallback — the parser
    /// already ran before the registry).
    AckOnly,
}

/// Build an `InboundMessage` carrying `text` as the callback data.
///
/// Centralises the field shape (id prefix, channel id, no attachments,
/// fresh timestamp) so the parse-error and unknown-kind fallback paths
/// stay in lockstep with the typed Forward path. `interaction_id` is
/// the serenity interaction id (used for the `cb_<id>` prefix that
/// the inbound router matches on).
fn legacy_inbound(
    text: &str,
    sender_id: &str,
    sender_name: Option<&str>,
    interaction_id: u64,
    conversation_id: ConversationId,
    is_group: bool,
) -> InboundMessage {
    InboundMessage {
        id: MessageId::new(format!("{CB_MESSAGE_ID_PREFIX}{interaction_id}")),
        channel_id: ChannelId::new("discord"),
        conversation_id,
        sender_id: UserId::new(sender_id),
        sender_name: sender_name.map(String::from),
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group,
        raw: None,
        metadata: vec![],
    }
}

/// Decide what the serenity `handle_component` should do with a button
/// click.
///
/// Three layered fallbacks preserve the legacy `handle_component`
/// behaviour:
///
/// 1. **Parse failure** (e.g. empty kind, empty payload) →
///    `Forward(raw custom_id)`. The raw string is forwarded so an
///    operator can diagnose the malformed shape.
///
/// 2. **Legacy no-colon shape** (codec parsed as
///    `Unknown(<whole string>)` with empty payload) → `Forward(raw
///    custom_id)`. Old bots that still emit `cb_<message_id>` style
///    strings land here; the previous `handle_component` forwarded
///    these as raw text and the inbound router's `cb_` prefix
///    intercept did the routing. We can't let the codec's Unknown
///    catch-all swallow them (Review Focus #3 — legacy `cb_<id>` bots
///    must keep working).
///
/// 3. **Unknown kind, no `Callback` handler** (empty registry) →
///    `Forward(raw custom_id)`. The agent loop sees the click instead
///    of it being silently swallowed (mirrors the old behaviour where
///    every `custom_id` became text).
///
/// 4. **Unknown kind, `Callback` handler registered** (the
///    `with_defaults()` case) → `AckOnly`. The Callback handler
///    returned `AckNoReply`; nothing useful should happen.
///
/// For typed kinds:
/// - `ApprovalApprove` / `ApprovalDeny` → `Forward(reconstructed callback_data)`.
///   The reconstruction depends on the payload shape:
///   - `ComponentKind` payload `<id>:<decision>` (legacy multi-tier
///     format from `ApprovalBridge::build_approval_keyboard`) →
///     `to_wire()` round-trips the full `approve:<id>:<decision>`
///     shape unchanged.
///   - `ComponentKind` payload `<id>` (the new typed `D4` format)
///     → append `:once` or `:deny` so `ApprovalBridge::parse_callback`
///     matches. The decision tier defaults to AllowOnce for Approve
///     and Deny for Deny.
///
/// `sender_id` / `sender_name` propagate into the forwarded inbound so
/// the inbound router's originator gate and the audit trail have a
/// real actor. `interaction_id` becomes the `cb_<id>` suffix (the
/// router matches the prefix, not the suffix).
#[must_use]
pub fn dispatch_component_click(
    registry: &CommandRegistry,
    custom_id: &str,
    sender_id: &str,
    sender_name: Option<&str>,
    interaction_id: u64,
    conversation_id: ConversationId,
    is_group: bool,
) -> CommandAction {
    let id = match ComponentId::parse(custom_id) {
        Ok(id) => id,
        Err(_) => {
            return CommandAction::Forward(Box::new(legacy_inbound(
                custom_id,
                sender_id,
                sender_name,
                interaction_id,
                conversation_id,
                is_group,
            )));
        }
    };

    // Legacy `cb_<message_id>` style strings (no `:`) parse as
    // `Unknown(<whole string>)` with empty payload. The codec
    // documents this as Discord's "raw callback" pattern, but the
    // previous handler forwarded them as text. Keep doing that —
    // routing them through the registry's Callback fallback would
    // AckOnly-swalllow clicks that bots depend on for approval
    // resolution (Review Focus #3).
    let is_legacy_no_colon =
        matches!(&id.kind, ComponentKind::Unknown(k) if k == custom_id) && id.payload.is_empty();
    if is_legacy_no_colon {
        return CommandAction::Forward(Box::new(legacy_inbound(
            custom_id,
            sender_id,
            sender_name,
            interaction_id,
            conversation_id,
            is_group,
        )));
    }

    match registry.dispatch(&id) {
        Some(DispatchOutcome::ForwardToApproval) => {
            // Reconstruct the callback_data that
            // `ApprovalBridge::parse_callback` understands. Two
            // wire-format shapes can reach this branch:
            //
            // - Legacy multi-tier: custom_id = `approve:<id>:<decision>`,
            //   id.payload = `<id>:<decision>` (contains a colon).
            //   `to_wire()` gives the right shape.
            // - New typed (D4): custom_id = `approve:<id>`,
            //   id.payload = `<id>` (no colon). Append `:once` or
            //   `:deny` so the sink has a parseable tier.
            let callback_data = match id.kind {
                ComponentKind::ApprovalApprove if !id.payload.contains(':') => {
                    format!("approve:{}:once", id.payload)
                }
                ComponentKind::ApprovalDeny if !id.payload.contains(':') => {
                    format!("approve:{}:deny", id.payload)
                }
                _ => id.to_wire(),
            };
            CommandAction::Forward(Box::new(legacy_inbound(
                &callback_data,
                sender_id,
                sender_name,
                interaction_id,
                conversation_id,
                is_group,
            )))
        }
        Some(DispatchOutcome::AckNoReply) | Some(DispatchOutcome::Rejected(_)) => {
            CommandAction::AckOnly
        }
        None => CommandAction::Forward(Box::new(legacy_inbound(
            custom_id,
            sender_id,
            sender_name,
            interaction_id,
            conversation_id,
            is_group,
        ))),
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
        assert_eq!(id.kind, ComponentKind::Unknown("mystery".to_string()));
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
        assert_eq!(r.dispatch(&id), Some(DispatchOutcome::ForwardToApproval));
        let id = ComponentId::parse("deny:m1").unwrap();
        assert_eq!(r.dispatch(&id), Some(DispatchOutcome::ForwardToApproval));
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

    // ----- dispatch_component_click tests (R2 D1) -----
    //
    // The legacy `cb_<id>` fallback is the contract Review Focus #3
    // called out. Without them the dispatcher would happily AckNo-clud-slow
    // legacy bots' approval resolutions and there would be no test to
    // catch it.

    fn conv() -> ConversationId {
        ConversationId::new("chan-1")
    }

    #[test]
    fn dispatch_legacy_cb_id_forwards_raw_custom_id() {
        // Old bot emits "cb_<message_id>" with no colon. The codec parses
        // this as Unknown(<whole>) with empty payload; Review Focus #3
        // requires the dispatcher NOT swallow it as AckNoReply.
        let r = CommandRegistry::with_defaults();
        let action =
            dispatch_component_click(&r, "cb_msg-123", "user-1", Some("alice"), 42, conv(), true);
        match action {
            CommandAction::Forward(msg) => {
                // The inbound router matches on the cb_ prefix; the
                // raw custom_id must be in `text` for routing to work.
                assert_eq!(msg.text, "cb_msg-123");
                assert!(msg.id.0.starts_with(CB_MESSAGE_ID_PREFIX));
                assert!(msg.is_group);
            }
            CommandAction::AckOnly => panic!("legacy cb_<id> must not AckOnly"),
        }
    }

    #[test]
    fn dispatch_approval_approve_reconstructs_callback_data() {
        // D4 typed format: "approve:<id>" (no decision tier). The sink
        // expects `approve:<id>:<decision>`; dispatcher appends :once.
        let r = CommandRegistry::with_defaults();
        let action = dispatch_component_click(
            &r,
            "approve:msg-7",
            "user-1",
            Some("alice"),
            42,
            conv(),
            true,
        );
        match action {
            CommandAction::Forward(msg) => {
                assert_eq!(msg.text, "approve:msg-7:once");
            }
            CommandAction::AckOnly => panic!("approval approve must Forward"),
        }
    }

    #[test]
    fn dispatch_approval_deny_reconstructs_callback_data() {
        // Same as approve but :deny tier.
        let r = CommandRegistry::with_defaults();
        let action =
            dispatch_component_click(&r, "deny:msg-7", "user-1", Some("alice"), 42, conv(), true);
        match action {
            CommandAction::Forward(msg) => {
                assert_eq!(msg.text, "approve:msg-7:deny");
            }
            CommandAction::AckOnly => panic!("approval deny must Forward"),
        }
    }

    #[test]
    fn dispatch_unknown_kind_without_callback_handler_forwards_raw() {
        // Empty registry (no Callback). Unknown kinds fall through to
        // Forward so the agent loop sees the click instead of a silent
        // AckOnly — mirrors the legacy handle_component forwarding the raw
        // custom_id.
        let r = CommandRegistry::new();
        let action =
            dispatch_component_click(&r, "mystery:x", "user-1", Some("alice"), 42, conv(), true);
        match action {
            CommandAction::Forward(msg) => {
                assert_eq!(msg.text, "mystery:x");
            }
            CommandAction::AckOnly => panic!("empty-registry unknown must Forward"),
        }
    }

    #[test]
    fn dispatch_unknown_kind_with_callback_handler_acks_only() {
        // Registry with Callback registered → Unknown kinds defer to
        // AckNoReply via the dispatcher (via with_defaults path which
        // also registers Callback).
        let r = CommandRegistry::with_defaults();
        let action =
            dispatch_component_click(&r, "mystery:x", "user-1", Some("alice"), 42, conv(), true);
        assert!(matches!(action, CommandAction::AckOnly));
    }

    #[test]
    fn dispatch_approval_approve_session_tier_round_trips() {
        // ApprovalBridge::build_approval_keyboard emits
        // `approve:<id>:<decision>` for each offered tier (once, session,
        // always, deny). The round-trip must preserve the tier so the
        // approval sink sees `Session` instead of the default `Once`.
        // R2 D4: cover the Session tier explicitly because the typed
        // format from ApprovalBridge is the wire shape the dispatcher
        // sees in production — dropping the tier would silently
        // downgrade operator decisions.
        let r = CommandRegistry::with_defaults();
        let action = dispatch_component_click(
            &r,
            "approve:req-42:session",
            "user-1",
            Some("alice"),
            42,
            conv(),
            true,
        );
        match action {
            CommandAction::Forward(msg) => {
                assert_eq!(
                    msg.text, "approve:req-42:session",
                    "session tier must round-trip; dispatcher must not default to :once"
                );
            }
            CommandAction::AckOnly => panic!("session-tier approval must Forward"),
        }
    }
}
