//! Capability ownership base types — the minimal vocabulary
//! [`crate::capability::descriptor`] needs to compile.
//!
//! These are pure data types with **no behaviour**. The ownership *tree* and
//! its fencing/revocation semantics are deliberately deferred to Task 2; this
//! file only establishes the types so a descriptor can name its owner, its
//! generation, its lifetime and its visibility. Keeping them behaviour-free is
//! load-bearing: `descriptor.rs` must compile against these without pulling in
//! an ownership engine that does not exist yet.

use crate::acp::manager::SessionKey;

/// Monotonic owner generation counter.
///
/// Bumped whenever ownership is re-issued, so a stale lease can be told apart
/// from a live one: a lease whose recorded generation is below the current one
/// is invalid. `u64` matches the registry's own monotonic revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerGeneration(pub u64);

/// How long a capability's owner binding lives, from the longest to the
/// shortest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifetimeScope {
    /// Bound to the process — lives for the whole runtime.
    Runtime,
    /// Bound to one session.
    Session,
    /// Bound to one run.
    Run,
    /// Bound to one task.
    Task,
    /// Owned externally, outside this process's lifetime model.
    External,
}

/// A run identifier.
///
/// Defined here as a plain newtype because the codebase has no single
/// canonical run-id type yet (the only `RunId` in-tree is a deserialization
/// params struct in `gateway::handlers::agent`). Align this to an existing
/// type the moment one is introduced — see the module-level note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

/// A task identifier.
///
/// Same reasoning as [`RunId`]: no canonical task-id type exists yet (the
/// domain-layer `TaskId` lives behind `#[cfg(test)]`), so this is a plain
/// newtype until one is introduced.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);

/// Who owns a capability binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerRef {
    /// Owned by the runtime itself (process-global, unowned-by-any-scope).
    Runtime,
    /// Owned by a specific session.
    Session(SessionKey),
    /// Owned by a specific run.
    Run(RunId),
    /// Owned by a specific task.
    Task(TaskId),
}

/// The visibility envelope a capability may be described under.
///
/// Every field is `Option`; `None` means "unrestricted" on that dimension, so
/// `VisibilityScope::default()` (all `None`) is "visible everywhere". The
/// "allowed kinds/ids" dimensions are spelled as `String` on purpose: this
/// module must not depend on [`crate::capability::descriptor::CapabilityKind`]
/// (that would make `descriptor` ↔ `ownership` a cycle), and the strings are
/// compared against the kind/name spellings at describe time, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibilityScope {
    /// The principal / agent allowed. `None` = unrestricted.
    pub principal: Option<String>,
    /// The workspace or channel allowed. `None` = unrestricted.
    pub workspace: Option<String>,
    /// The session allowed. `None` = unrestricted.
    pub session: Option<SessionKey>,
    /// Allowed capability kinds, by name. `None` = unrestricted.
    pub allowed_kinds: Option<Vec<String>>,
    /// Allowed capability ids, by name. `None` = unrestricted.
    pub allowed_ids: Option<Vec<String>>,
    /// Permission / approval context required to see this. `None` = unrestricted.
    pub approval: Option<String>,
}

/// Opaque fence token, issued by the owner layer.
///
/// The field is deliberately private and `Copy`: a fence is an unforgeable
/// marker, and every `claim` mints a fresh one so a stale lease can never
/// present an old fence as live. Only the owner layer constructs these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FencingToken(u64);

impl FencingToken {
    /// Mint a fresh token from the owner layer's monotonic nonce. This is the
    /// *only* constructor; the nonce's monotonicity is the owner layer's
    /// contract, not this type's.
    #[must_use]
    pub fn new(nonce: u64) -> Self {
        Self(nonce)
    }
}

/// A recorded effect claim: one request's ownership assertion over a fence.
///
/// Pure data — the claim's enforcement (who may revoke, what a stale fence
/// means) lives in Task 2's ownership tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaim {
    pub request_id: String,
    pub fence: FencingToken,
    pub owner: OwnerRef,
}
