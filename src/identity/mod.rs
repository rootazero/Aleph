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
//!   intact.
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
//! * **Subagent calls are attributed to their parent**, because they execute
//!   through the parent's `ScopedToolService` and therefore under the parent's
//!   `TURN_CONTEXT` — the chokepoint has no sub-role signal to record. This is
//!   the honest reading of the current execution model, not an oversight to be
//!   papered over with a guessed attribution.
//! * **Not tamper-proof, tamper-evident.** See [`keystore`] for exactly what a
//!   signature here does and does not prove.
//!
//! Full model: `docs/reference/AGENT_IDENTITY.md`.

pub mod hash;
pub mod keystore;
pub mod ledger;
pub mod record;
pub mod schema;
pub mod verify;

pub use keystore::{AgentIdentityRow, AgentKeyRow, AgentKeystore, KeyError};
pub use ledger::{global, install, record as record_action, AgentLedger};
pub use record::{LedgerAction, LedgerOutcome, LedgerRecord, NewRecord};
pub use schema::IDENTITY_SCHEMA;
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
