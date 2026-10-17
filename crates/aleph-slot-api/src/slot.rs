//! Slot identity and typed Hook@1 trait (Task 6).
//!
//! This module is a NEW pure contract; it is not a migration of the legacy
//! Core `HookContext` / `HookResult` / `HookConfig` / hook executor. The
//! runtime dispatch path, permission gates, lease/grant policy, and Core
//! `HookExecutor` continue to live in `alephcore` and own those distinct
//! legacy types. Production wiring of the new trait belongs to P0.5 / P2 and
//! is out of scope for this module.
//!
//! ## Authority
//!
//! * The protocol `HOOK_CONTRACT_VERSION`, the seven-variant `HookEventTag`,
//!   the strict `HookEnvelope` / `HookResponse` wire, and the protocol-side
//!   `validate_envelope` / `validate_response` validators are the single
//!   source of truth (see `shared/protocol/src/hook.rs`, accepted in Task 5).
//!   The protocol re-exports reach this module via `crate::hooks` and via
//!   `crate::protocol`; no protocol→API back-edge exists in source.
//! * `SlotId` and `ContractVersion` are correlation newtypes; they do not
//!   grant authority, do not parse any privileged ID syntax, and do not
//!   authorize or deny anything. The new types are expressible for any
//!   plugin — being expressible is not a grant.
//! * The single event source is `HookEventTag` (seven variants). The legacy
//!   24-variant `HookEvent` is retained in `crate::hooks` for the existing
//!   hooks.json / JSON-RPC surface and never serves as a wire tag for the
//!   new contract.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hooks::{HookKind, HookPriority};
use crate::protocol::hook::{
    validate_envelope, HookEnvelope, HookEventTag, HookResponse, HOOK_CONTRACT_VERSION,
};

// =============================================================================
// Slot identity and contract version (correlation only, not authority).
// =============================================================================

/// Slot identifier (correlation only, not authority).
///
/// The slot is the structural unit a hook is bound to. A slot id is just a
/// string used to correlate facts; it does not parse any privileged
/// syntax, encode a grant, or imply ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlotId(pub String);

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Contract version a hook implementation is built against.
///
/// The only contract currently in force is the protocol
/// `HOOK_CONTRACT_VERSION`. A descriptor carrying any other version is
/// rejected by [`validate_hook_request`] before `Hook::execute` is
/// called. This crate does not publish a `CURRENT` alias; callers
/// spell `ContractVersion(HOOK_CONTRACT_VERSION)` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContractVersion(pub u32);

impl fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// Hook descriptor
// =============================================================================

/// Typed descriptor for a single hook implementation.
///
/// The `event` field is the seven-variant `HookEventTag` — the same tag
/// shape the strict `HookEnvelope` carries. The legacy 24-variant
/// `HookEvent` is intentionally NOT a valid value for this field; mixing
/// the two is a wire-shape mistake and is rejected by serde before this
/// struct is constructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDescriptor {
    /// Implementation-defined hook id (correlation only).
    pub id: String,
    /// Slot the hook is bound to.
    pub slot: SlotId,
    /// Event tag this hook claims to handle. Must equal the envelope's
    /// `event.event_tag()` for execution to proceed.
    pub event: HookEventTag,
    /// Hook execution kind (interceptor / observer).
    pub kind: HookKind,
    /// Hook priority.
    pub priority: HookPriority,
    /// Contract version this implementation is built against. Must equal
    /// the protocol `HOOK_CONTRACT_VERSION`.
    pub contract_version: ContractVersion,
}

// =============================================================================
// Hook context
// =============================================================================

/// Typed execution context. Wraps the strict protocol `HookEnvelope`.
///
/// The envelope is the only fact source for the execution body — there is
/// no separate runtime metadata, no inherited Core context, no legacy
/// `HookContext` substitution. Validators on this module re-check the
/// envelope facts every time, so a directly-constructed envelope with the
/// wrong `version` field or invalid facts cannot pass `validate_hook_request`.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub envelope: HookEnvelope,
}

// =============================================================================
// Hook error
// =============================================================================

/// Hook error. The `Display` impl is the inner message verbatim; the type
/// carries no implicit category, severity, or authority. Hook authors use
/// it for guard, validation, and execution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookError {
    pub message: String,
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HookError {}

// =============================================================================
// Hook trait
// =============================================================================

/// Typed hook contract.
///
/// Implementors describe themselves via [`HookDescriptor`] and produce
/// [`HookResponse`]s in response to a [`HookContext`]. The trait is
/// `Send + Sync` so a `Box<dyn Hook>` is itself `Send + Sync`, which is
/// the property the runtime dispatch layer requires once P0.5/P2 wires
/// it up.
///
/// The trait has no default implementation for `execute`: every hook is
/// expected to construct its own response (or fail). A missing
/// implementation is a compile-time error, not a runtime no-op.
#[async_trait::async_trait]
pub trait Hook: Send + Sync {
    /// The descriptor for this hook. Used by [`validate_hook_request`]
    /// before `execute` is called.
    fn descriptor(&self) -> &HookDescriptor;

    /// Run the hook against the typed context. The returned response is
    /// untrusted until the protocol's `validate_response` accepts it; a
    /// test-only call boundary in this module performs that check.
    async fn execute(&self, ctx: &HookContext) -> Result<HookResponse, HookError>;
}

// =============================================================================
// Pre-execute guard
// =============================================================================

/// Pre-execute guard. Rejects requests the descriptor cannot legitimately
/// serve BEFORE `Hook::execute` is called.
///
/// The guard checks, in order:
///
/// 1. `descriptor.contract_version` equals the protocol
///    [`HOOK_CONTRACT_VERSION`]. A mismatched version is rejected.
/// 2. `descriptor.event` equals `envelope.event.event_tag()`. A mismatched
///    tag is rejected.
/// 3. The envelope itself is revalidated by the protocol's
///    [`validate_envelope`], so directly-constructed envelopes with the
///    wrong `version` field or invalid facts (empty `session_id`, wrong
///    turn-id matrix, mismatched lifecycle context ids, …) are rejected.
///
/// This guard performs no serialization / re-parsing and duplicates no
/// lifecycle / pair / schema rules — those live in the protocol module.
/// The guard exists so the test-only call boundary and any future
/// dispatcher can call the same pre-execute check by name.
pub fn validate_hook_request(
    descriptor: &HookDescriptor,
    envelope: &HookEnvelope,
) -> Result<(), HookError> {
    if descriptor.contract_version.0 != HOOK_CONTRACT_VERSION {
        return Err(HookError {
            message: format!(
                "hook descriptor contract version {} does not match protocol {}",
                descriptor.contract_version.0, HOOK_CONTRACT_VERSION
            ),
        });
    }
    let envelope_tag = envelope.event.event_tag();
    if descriptor.event != envelope_tag {
        return Err(HookError {
            message: format!(
                "hook descriptor event {:?} does not match envelope event tag {:?}",
                descriptor.event, envelope_tag
            ),
        });
    }
    validate_envelope(envelope).map_err(|err| HookError {
        message: err.to_string(),
    })
}
