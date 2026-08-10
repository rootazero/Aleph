//! Agent identity and the signed operation ledger.
//!
//! Answers the two halves of accountability that a string `agent_id` cannot:
//!
//! * **Who acted** — every agent holds an Ed25519 keypair
//!   ([`keystore`]). The public half and a stable fingerprint live in
//!   `security.db`; the private half lives in the existing encrypted vault.
//!   Rotation retires a key rather than replacing it, so history signed under
//!   the old key stays verifiable.
//! * **What happened** — every consequential action an agent takes is appended
//!   to that agent's own hash-chained, signed chain ([`ledger`]), from the one
//!   chokepoint every tool call already passes through
//!   (`tools::scoped::dispatch`). [`verify`] then answers whether the chain is
//!   intact, and [`export`] puts that answer within reach of someone who does
//!   not trust this machine: a self-contained document, checked by the same
//!   walk against a root fingerprint pinned off-box.
//!
//! ## Deliberate non-goals
//!
//! * **This is not an impersonation defence.** `agent_id` still arrives as a
//!   caller-supplied string on `chat.send` and is checked only for existence
//!   ([`crate::gateway::router`]). A caller who has cleared the connection-level
//!   auth can still act as any existing agent, and the ledger will faithfully
//!   record the identity it was given. Closing that requires binding the
//!   claimed agent to the device's authorization, which is a change to the RPC
//!   authorization model, not to this subsystem.
//! * **Not tamper-proof, tamper-evident.** See [`keystore`] for exactly what a
//!   signature here does and does not prove.
//!
//! Subagent calls used to land on the *parent's* chain, because a subagent
//! executes through the parent's `ScopedToolService` and `SessionKey::Subagent`
//! resolves `agent_id()` to its parent. They no longer do: the spawner-side
//! wrapper scopes the acting role through [`actor`], so a delegated role signs
//! its own work. Teams were never affected — a member run owns its turn.
//!
//! Full model: `docs/reference/AGENT_IDENTITY.md`.

pub mod actor;
pub mod export;
pub mod hash;
pub mod keystore;
pub mod ledger;
pub mod record;
pub mod schema;
pub mod verify;

pub use actor::{as_actor, current_actor};
pub use export::{
    export_chain, verify_export, ChainExport, ExportError, ExportPins, ExportReport, HeadPin,
    PinnedHead,
};
pub use keystore::{AgentIdentityRow, AgentKeyRow, AgentKeystore, KeyError};
pub use ledger::{
    flush, global, install, record as record_action, revoke_identity, rotate_identity, AgentLedger,
    LedgerCommandError, Rotation,
};
pub use record::{LedgerAction, LedgerOutcome, LedgerRecord, NewRecord};
pub use schema::{IDENTITY_SCHEMA, LEDGER_ADD_PRINCIPAL_SQL};
pub use verify::{verify_chain, ChainFault, ChainReport};

use crate::sync_primitives::Arc;

/// Build and install the process-wide ledger from the two handles boot already
/// owns. Returns the writer task handle, or `None` when a ledger is already
/// installed.
///
/// Kept here rather than in the binary so the assembly (keystore over the
/// security store + vault) has one definition and the boot site stays a single
/// call (P1).
pub fn install_from(
    store: Arc<crate::gateway::security::store::SecurityStore>,
    vault: Arc<crate::gateway::security::shared_token::SharedTokenManager>,
) -> Option<tokio::task::JoinHandle<()>> {
    let keys = Arc::new(AgentKeystore::new(store, vault));
    install(Arc::new(AgentLedger::new(keys)))
}
