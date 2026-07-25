//! Aleph cluster (single-center asymmetric node federation).
//!
//! Full-stack center-side infrastructure: reverse RPC transport primitives
//! ([`ReverseRpcChannel`] / [`PendingInvokes`], server→connected-client id-bearing
//! request/response distinguished by structure not id), node registry
//! ([`NodeRegistry`] multi-level addressing + tag fan-out + disconnect
//! fail-fast), node-side command dispatch ([`CommandTable`] allowlist
//! authoritative), file command jail ([`FileReadCommand`] /
//! [`FileWriteCommand`]), and node approval loopback to center
//! ([`CenterApprovalRequester`] fail-closed). Center-side LLM tools `node_list` /
//! `node_invoke` / `node_invoke_many` / `node_file` in `crate::builtin_tools`
//! drive nodes through these primitives. Trust model is LAN-trust: nodes hold no
//! tokens, connection identity is declared by the connect frame's parameter shape
//! (`commands` + `tags`) (see [`maybe_register_node`]), and registration itself
//! happens within `connect` (see [`admit_node`]). For the full engineering
//! picture see `docs/reference/CLUSTER.md`.
//!
//! Redline: this module contains no LLM reasoning (R7), does not enter
//! `src/harness/` (R10).

mod enrollment;
mod node_approval;
mod node_file_cmd;
mod node_runtime;
mod registry;
mod reverse_rpc;

pub use enrollment::{
    admit_node, deregister_node, enroll_node_device, DeregisterError, DeregisterOutcome,
    NodeAdmission,
};
pub use node_approval::{ApprovalSlot, CenterApprovalRequester, NODE_APPROVAL_TIMEOUT_MS};
pub(crate) use node_file_cmd::sha256_hex;
pub use node_file_cmd::{FileReadCommand, FileWriteCommand, MAX_FILE_BYTES};
pub use node_runtime::{CommandTable, NodeCommand};
pub(crate) use registry::normalize_node_key;
pub use registry::{
    maybe_register_node, CommandDescriptor, Environment, NodeMatch, NodeRegistry, NodeSession,
    ResolveError,
};
pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
