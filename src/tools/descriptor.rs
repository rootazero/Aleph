//! Re-export shim — descriptor types live in `aleph-slot-api`.
//!
//! As of P0 Task 2, the pure descriptor contract — `ToolKind`,
//! `ReplayPolicy`, `ImplementationContract`, `ReplayContractFingerprint`,
//! `ToolCallIdentity`, the descriptor 3-variant `ToolSource`, the
//! `ReplayPolicyLookup` / `ToolDescriptorLookup` traits,
//! `DescriptorError`, `ToolCapabilityDescriptor`, and the
//! `canonicalize_json` algorithm helper — moved to
//! `aleph_slot_api::descriptor`. This file re-exports them so every
//! existing `crate::tools::descriptor::*` path keeps compiling. The
//! `compute_replay_contract_fingerprint` /
//! `canonical_replay_contract_preimage` / `push_len_prefixed` helpers
//! stay private to the API crate exactly as they were private to Core
//! before the move — no Core code ever named them.
//!
//! ## Identity guarantee
//!
//! `crate::tools::descriptor::ToolSource` resolves to the same
//! `TypeId` as `aleph_slot_api::descriptor::ToolSource` and
//! `aleph_slot_api::tool::ToolSource` (which is itself a re-export
//! of the descriptor type). The 3-variant descriptor `ToolSource`
//! is intentionally distinct from the 6-variant metadata
//! `ToolSource` (`crate::tool_metadata::ToolSource` /
//! `aleph_slot_api::metadata::ToolSource`). The two are kept apart
//! on purpose: the descriptor owns canonical capability identity,
//! the metadata owns the projection / panel presentation.
//!
//! ## Single definition of `canonicalize_json`
//!
//! `aleph_slot_api::descriptor::canonicalize_json` is the only
//! definition in the crate graph. `src/capability/backend.rs` and
//! any other Core consumer reaches it through this re-export, so
//! the "two definitions" trap (one for the descriptor, one for the
//! capability backend) is closed. Per the approved P0 Task 2 ruling
//! the helper is `#[doc(hidden)] pub` in the API crate and a
//! **crate-private** re-export here — it is not part of Core's
//! public surface.

pub use aleph_slot_api::descriptor::{
    DescriptorError, ImplementationContract, ReplayContractFingerprint, ReplayPolicy,
    ReplayPolicyLookup, ToolCallIdentity, ToolCapabilityDescriptor, ToolDescriptorLookup, ToolKind,
    ToolSource, MAX_NAME_LEN, REPLAY_CONTRACT_DIGEST_LEN, REPLAY_CONTRACT_FINGERPRINT_VERSION,
    SCHEMA_VERSION,
};

pub(crate) use aleph_slot_api::descriptor::canonicalize_json;
