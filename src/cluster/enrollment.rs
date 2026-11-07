//! Node lifecycle (center side): admission on `connect`, operator
//! pre-enrollment, and deregistration — the three writers of the `role=node`
//! device record, kept in one file so they cannot drift apart.
//!
//! Under LAN-trust, nodes don't hold tokens, but the center still keeps a
//! `role=node` device record in `security_store` for each node — this is the
//! **offline fleet view** (the offline half of `environments.list`) and the
//! bookkeeping foundation for `cluster.deregister`. This module is the **single
//! write/resolve source of truth** for that record, shared by two entry points:
//!
//! * `connect` seam (`gateway/server/connection/mod.rs`) — node self-registration
//!   (first boot, no id).
//! * `cluster.enroll` RPC (`gateway/handlers/cluster.rs`) — operator
//!   pre-enrollment from the Panel.
//!
//! Both share the same [`mint_node_device`], so a pre-enrolled row and the
//! node's first dial-in are **merged by name** into the same `node_id`, rather
//! than each minting a separate UUID that leaves a permanent ghost offline row.
//!
//! **The presented id is authoritative.** A node that presents an ACTIVE
//! `device_id` is admitted under THAT id, even when the `device_name` it
//! declares differs from the stored name: the id is the bearer secret the node
//! persisted on disk, the name is a display affordance a peer may re-spell, and
//! renaming a machine must never change its identity. Under LAN trust (nodes
//! hold no tokens) the id IS the credential — this is the policy, not an
//! oversight.
//!
//! Redline: deterministic table lookup + database writes, no LLM reasoning (R7);
//! does not enter `src/harness/` (R10).

use tracing::warn;

use crate::cluster::normalize_node_key;
use crate::cluster::registry::{bound_device_name, truncate_on_char_boundary};
use crate::gateway::security::store::{DeviceRow, DeviceUpsertData, SecurityStore};
use crate::sync_primitives::Mutex;

/// Process-wide serialization of the **name → row** resolution (the
/// `match_by_name` read followed by the insert).
///
/// Without it, two concurrent `cluster.enroll` calls for the same name (or an
/// enroll racing the node's own first boot) can both read "no such name" and
/// both insert — which is exactly the duplicate cascade this module exists to
/// prevent: the second row makes the by-name match ambiguous, so the node's
/// first boot stops adopting either and mints a third, while the offline
/// deregister fallback (which needs a unique name) can no longer address any
/// of them.
///
/// `std::sync::Mutex` is enough: both sides of the sequence are synchronous and
/// no `.await` happens under the guard.
///
/// **Lock order**: this guard is the OUTERMOST lock of the pair it participates
/// in — name lock → the store's connection lock, which is a leaf. It is always
/// taken BEFORE the `SecurityStore` calls it guards, and nothing takes it while
/// the store's lock is held, so there is no cycle.
///
/// It does **not** guard every store call: the presented-`device_id` lookup in
/// [`admit_node`] happens BEFORE the lock is taken. That is deliberate — that
/// lookup is by id, not by name, so it cannot race with the name → row
/// resolution this lock serializes.
static NODE_NAME_LOCK: Mutex<()> = Mutex::new(());

/// Upper bound on a peer-presented `device_id`.
///
/// The id arrives in the `connect` frame, so it is peer-controlled text that
/// would otherwise be written verbatim into the `devices` table — as the key
/// of the very row that makes the node addressable. 128 bytes is far above the
/// 36 bytes of the UUIDs this cluster mints, and far below anything a real
/// device id needs.
const MAX_PRESENTED_DEVICE_ID_BYTES: usize = 128;

/// Admission decision for a node `connect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeAdmission {
    /// Admitted: register the session under this `node_id`. `minted` = a new
    /// device record was created this time (the node should persist the
    /// `node_id` to disk).
    Admitted { node_id: String, minted: bool },
    /// Rejected: this node has been deregistered via `cluster.deregister`
    /// (device record `revoked_at` is non-null). Deregistration must **stick** —
    /// otherwise a kicked node would just self-revive on the next backoff
    /// reconnect cycle.
    Deregistered { node_id: String },
    /// Rejected: the presented id already names a **paired remote Panel**
    /// (`device_type = "panel"`) or any **non-node** row (`role != "node"`),
    /// not a node. Refused rather than adopted so the two halves of the shared
    /// `devices` namespace cannot take over each other's rows — and so a node
    /// cannot claim an operator/panel identity it never enrolled under (the
    /// offline deregister fallback filters on `role == "node"`, so such a node
    /// would become un-removable while offline). Carried as its own variant (not
    /// folded into [`Self::Deregistered`]) so the log line and the regression
    /// test stay honest; both refusals are terminal and share one wire status.
    IdentityConflict { node_id: String },
    /// Rejected: the enrolled-device store could not be read or written, so
    /// the center cannot tell whether this node is admitted, revoked, or
    /// already enrolled. **Retryable, not terminal** — the node backs off and
    /// dials in again. The center must NOT admit it (a revoked node would come
    /// straight back) and must NOT mint a row (that is how same-name ghosts
    /// accumulate).
    StoreUnavailable { detail: String },
    /// Rejected: several **active** enrolled rows share this node's name, so
    /// the center cannot tell which one this machine is. **Terminal** — minting
    /// a third row under a name two rows already share is what buries the
    /// fleet in rows nobody can address; the duplicates have to be repaired by
    /// id first.
    Ambiguous { name: String, count: usize },
}

/// Why a mint could not produce a row.
///
/// The two classes are kept apart because their callers treat them
/// differently: an unaddressable name is the operator's mistake (retrying the
/// same request changes nothing), a failed write is a server fault (the
/// request was fine).
#[derive(Debug, Clone, PartialEq, Eq)]
enum MintError {
    /// The name can never be addressed again.
    Unaddressable(String),
    /// The store write failed.
    Store(String),
}

/// Mint a **fresh** `role=node` device record and return its `node_id` (UUID).
///
/// `public_key` is a placeholder (LAN-trust has no hardware key, same as
/// `connect.rs`); `fingerprint` uses the first 16 chars of the UUID to satisfy
/// the UNIQUE constraint on the table.
///
/// Private on purpose: every caller must go through [`admit_node`] (node
/// self-registration) or [`enroll_node_device`] (operator pre-enrollment), both
/// of which try [`match_by_name`] first. Calling this directly is how
/// `cluster.enroll` used to mint a duplicate row per click.
///
/// `node_name` must already be bounded by [`bound_device_name`]: the store and
/// the online registry have to agree on the spelling, or a long name would be
/// stored under one form and matched under another. Both callers bound it
/// first.
///
/// Refuses a name that cannot be addressed ([`unaddressable_name_reason`]): a
/// row stored under it would be invisible to every later by-name lookup, and a
/// repeat enrollment would mint a second one.
fn mint_node_device(store: &SecurityStore, node_name: &str) -> Result<String, MintError> {
    if let Some(reason) = unaddressable_name_reason(node_name) {
        return Err(MintError::Unaddressable(reason));
    }
    let device_id = uuid::Uuid::new_v4().to_string();
    // (B2-02) `&device_id[..16]` was a tacit assumption that the id is an
    // ASCII UUID. Today every minted id is, but the private helper could be
    // called from elsewhere later. Use `truncate_on_char_boundary` so the
    // mint path is consistent with `admit_node` path 4, which already uses
    // it. (Both sites slice to 16 bytes to feed the device-table UNIQUE
    // constraint on fingerprint.)
    let fingerprint = truncate_on_char_boundary(&device_id, 16);
    store
        .upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name: node_name,
            device_type: None,
            public_key: &[0u8; 32],
            fingerprint,
            role: "node",
            scopes: &["node".to_string()],
            user_id: None,
        })
        .map_err(|e| {
            // (B2-03) The caller already warns, but the source of the failure
            // is here — emit a warning at the source too so the log line and
            // the error string refer to the same call frame.
            warn!(
                node_name,
                error = %e,
                "failed to register node device row in security store"
            );
            MintError::Store(format!("failed to register node device: {e}"))
        })?;
    Ok(device_id)
}

/// Outcome of matching **active** (non-revoked) `role=node` devices by
/// normalized name. Ambiguity is a distinct state rather than a second flavour
/// of "no match": **both** writers refuse to mint past it — the operator path
/// (`cluster.enroll`) and the node's own first boot. Minting a third row under
/// a name two rows already share is what buries the fleet in same-name rows
/// nobody can address (P3 "make illegal states unrepresentable").
enum NameMatch {
    /// Exactly one active record normalizes to this name.
    Unique(String),
    /// No active record does.
    None,
    /// Several do; carries the count for an actionable operator message.
    Ambiguous(usize),
}

/// Match active `role=node` devices by normalized name.
///
/// Name normalization shares the same source of truth as online addressing
/// [`normalize_node_key`], so a pre-enrolled "GPU Box" in the Panel and a node
/// dialing in as `--name gpu-box` merge into the same record.
///
/// Store read failures are surfaced as `Err` so callers that mint new rows
/// on `NameMatch::None` (notably [`enroll_node_device`]) cannot accidentally
/// mint a duplicate while the store is wedged — the previous silent
/// fall-through to `None` would create a second `role=node` row under the
/// same name, which the by-name fallback could no longer disambiguate.
fn match_by_name(store: &SecurityStore, node_name: &str) -> Result<NameMatch, rusqlite::Error> {
    let key = normalize_node_key(node_name);
    if key.is_empty() {
        return Ok(NameMatch::None);
    }
    let devices = store.list_devices()?;
    let mut hits = devices
        .into_iter()
        .filter(|d| d.role == "node" && normalize_node_key(&d.device_name) == key);
    let Some(first) = hits.next() else {
        return Ok(NameMatch::None);
    };
    let extra = hits.count();
    if extra > 0 {
        Ok(NameMatch::Ambiguous(extra + 1))
    } else {
        Ok(NameMatch::Unique(first.device_id))
    }
}

/// Why a node name cannot be addressed, or `None` when it can.
///
/// [`normalize_node_key`] folds a name to its lookup key; a name without
/// letters or digits (empty, whitespace-only, punctuation-only) folds to an
/// EMPTY key, and the by-name match skips empty keys on BOTH sides. A row
/// stored under such a name can therefore never be found again: it shows up in
/// the fleet view as an un-addressable ghost, `cluster.deregister` cannot name
/// it, and a repeat enrollment mints a SECOND such row — breaking the
/// idempotence [`enroll_node_device`] promises.
///
/// The message is operator-facing: it names the offending value and says what
/// to do instead.
fn unaddressable_name_reason(node_name: &str) -> Option<String> {
    if normalize_node_key(node_name).is_empty() {
        return Some(format!(
            "node name '{node_name}' is not addressable: it contains no letters or digits, \
             so it can never be matched by name again; use a name with at least one letter or digit"
        ));
    }
    None
}

/// Why an operator pre-enrollment could not be completed.
///
/// One taxonomy for **both** faces of the operation — the `cluster.enroll` RPC
/// and the `node_manage` tool — so the Panel and the model cannot disagree
/// about whether a refusal was the operator's mistake or the server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollError {
    /// The name can never be addressed again (it normalizes to an empty key).
    /// The operator's mistake: nothing was written.
    Unaddressable(String),
    /// Several enrolled rows already share this name. The operator's mistake:
    /// the fleet must be repaired by id, not deepened by another row.
    Ambiguous(String),
    /// The enrolled-device store could not be read or written. A **server**
    /// fault — the request was fine, the center was not.
    StoreUnavailable(String),
}

impl std::fmt::Display for EnrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unaddressable(m) | Self::Ambiguous(m) | Self::StoreUnavailable(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for EnrollError {}

/// **Idempotent** operator pre-enrollment (`cluster.enroll` / the `node_manage`
/// tool): reserve a fleet slot for `node_name` and return
/// `(node_id, minted)` — `minted=false` means an existing record was returned
/// unchanged.
///
/// Idempotence is the whole point. `mint_node_device` alone made every click of
/// the Panel's "+ Enroll" (or every retried RPC) create another `role=node` row
/// with the same name, and those duplicates are **self-sealing**: the second row
/// makes `match_by_name` ambiguous, so the node's own first boot stops
/// adopting either and mints a *third*, while `cluster.deregister`'s offline
/// fallback — which requires a unique name match — can no longer address any of
/// them. The operator ends up with permanent ghost rows removable only by
/// editing the database.
///
/// Ambiguity is therefore an error here, not a mint: a fleet that already has
/// duplicates must be repaired by id, not deepened.
pub fn enroll_node_device(
    store: &SecurityStore,
    node_name: &str,
) -> Result<(String, bool), EnrollError> {
    // One bound for the device name, shared with the online registry: the
    // registry bounds the name it gets from the connect frame, so the store has
    // to bound it the same way — otherwise a long name is stored under one
    // spelling and matched under another. The device id is not known yet (the
    // row is minted after the name is bounded); the parameter only feeds the
    // truncation log.
    let node_name = bound_device_name(node_name.to_string(), "");
    // Serialize the read-then-insert sequence: without the guard, two
    // concurrent enrollments of the same name can both read "no such name" and
    // both insert, and the duplicate row is self-sealing damage (see the
    // function's doc).
    let _guard = NODE_NAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match match_by_name(store, &node_name) {
        Ok(NameMatch::Unique(id)) => Ok((id, false)),
        Ok(NameMatch::None) => mint_node_device(store, &node_name)
            .map(|id| (id, true))
            .map_err(|e| match e {
                MintError::Unaddressable(m) => EnrollError::Unaddressable(m),
                MintError::Store(m) => EnrollError::StoreUnavailable(m),
            }),
        Ok(NameMatch::Ambiguous(n)) => Err(EnrollError::Ambiguous(format!(
            "{n} enrolled nodes already share the name '{node_name}'; \
             deregister the duplicates by id first"
        ))),
        Err(e) => Err(EnrollError::StoreUnavailable(format!(
            "enroll_node_device: store read failed for name '{node_name}' \
             (refusing to mint a duplicate row): {e}"
        ))),
    }
}

/// Resolve admission for a node `connect`. `presented_id` = the `device_id` from
/// the connect frame (`None` on first boot).
///
/// Decision order:
/// 1. Has id AND that record is **revoked** → [`NodeAdmission::Deregistered`]
///    (deregistration sticks).
/// 2. Has id AND that record is a **paired Panel** (`device_type = "panel"`) or
///    any **non-node** row (`role != "node"`) →
///    [`NodeAdmission::IdentityConflict`].
///    `devices` is one namespace shared by nodes and paired Panels, and both ids
///    are peer-asserted, so a node claiming a Panel's id would take over that
///    row's `last_seen_at` and put the Panel one `cluster.deregister` away from
///    losing its credential. Mirror of the guard in
///    `DeviceTokenManager::exchange_bootstrap_ticket`. The same reasoning
///    covers every non-node role: an adopted operator/panel row would carry a
///    node identity the operator never enrolled, and the offline deregister
///    fallback filters on `role == "node"`, so the node would be un-removable
///    while offline.
/// 3. Has id AND record is active → reuse as-is (stable identity).
/// 4. Has id but **no such record** (center DB reset / switched centers) →
///    adopt the id and backfill the row so the node keeps its persisted identity
///    without re-registering — unless the NAME already belongs to another
///    enrolled row, in which case this node converges on that row instead of
///    shadowing it.
/// 5. No id (first boot) → try to adopt an operator's pre-enrolled row by name
///    first, else mint new; `minted=true` signals the node to persist the id.
///
/// **The presented id wins over the presented name.** The id is the bearer
/// secret the node persisted on disk; the name is a display affordance a peer
/// may re-spell. Under LAN trust (no tokens) the id IS the credential, so a
/// node whose declared name drifted from the stored one keeps its identity
/// instead of being re-homed under a name that may belong to somebody else.
///
/// A store read/write failure is **not** degraded to "admit anyway": the center
/// cannot tell whether the node's row is revoked, so admitting it would let a
/// deregistered node straight back in, and minting would bury a pre-enrolled
/// row the operator cannot see. Both are reported as
/// [`NodeAdmission::StoreUnavailable`], which the caller must treat as
/// **retryable** — the node backs off and dials in again.
pub fn admit_node(
    store: &SecurityStore,
    presented_id: Option<&str>,
    node_name: &str,
) -> NodeAdmission {
    // The presented id is peer-controlled text that would otherwise be written
    // verbatim into the `devices` table — as the key of the very row that makes
    // the node addressable. Refuse an over-long one outright: it is never looked
    // up and never written. The node then falls through to the ordinary
    // first-boot path, which hands it a usable identity (the fleet row for its
    // name, or a fresh one).
    let presented_id = presented_id.filter(|s| !s.is_empty());
    let presented_id = match presented_id {
        Some(id) if id.len() > MAX_PRESENTED_DEVICE_ID_BYTES => {
            warn!(
                bytes = id.len(),
                limit = MAX_PRESENTED_DEVICE_ID_BYTES,
                "node presented an over-long device_id; refusing it and falling back \
                 to first-boot admission"
            );
            None
        }
        other => other,
    };
    // One bound for the device name, shared with the online registry: the
    // registry bounds the name it gets from the connect frame, so the store has
    // to bound it the same way — otherwise a long name is stored under one
    // spelling and matched under another.
    let node_name = bound_device_name(node_name.to_string(), presented_id.unwrap_or(""));

    if let Some(id) = presented_id {
        match store.get_device(id) {
            Ok(Some(row)) if row.revoked_at.is_some() => {
                return NodeAdmission::Deregistered {
                    node_id: id.to_string(),
                };
            }
            Ok(Some(row))
                if row.device_type.as_deref()
                    == Some(crate::gateway::security::PANEL_DEVICE_TYPE) =>
            {
                return NodeAdmission::IdentityConflict {
                    node_id: id.to_string(),
                };
            }
            // Defense in depth: `devices` also holds operator/panel rows, and
            // the admission path is the only place that decides which of them a
            // node may claim. A non-node row must never be adopted as a node
            // identity — the offline deregister fallback filters on
            // `role == "node"`, so the node would become un-removable while
            // offline.
            Ok(Some(row)) if row.role != "node" => {
                return NodeAdmission::IdentityConflict {
                    node_id: id.to_string(),
                };
            }
            Ok(Some(_)) => {
                return NodeAdmission::Admitted {
                    node_id: id.to_string(),
                    minted: false,
                }
            }
            Ok(None) => {
                // Unknown id: the node holds a persisted identity this center has
                // no record of (fresh DB, restored backup). Adopt it and backfill
                // the row so the offline fleet view stays honest.
                // A presented id is peer-supplied, so it is NOT guaranteed to be
                // an ASCII UUID — byte-slicing it would panic on a multi-byte
                // boundary (P7).
                //
                // The backfill is only safe while the NAME is still unclaimed.
                // A row that already owns this normalized name must be
                // CONVERGED ON, not shadowed: a second row under the same name
                // makes the by-name match ambiguous, the offline deregister
                // fallback (which needs a unique name) can no longer address
                // either of them, and every later first boot of that name mints
                // yet another ghost row.
                let _guard = NODE_NAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                match match_by_name(store, &node_name) {
                    Ok(NameMatch::Unique(existing)) if existing != id => {
                        // Converge on the row that already owns this name — the
                        // same convergence the no-id path performs. The node is
                        // handed the existing id and told to persist it
                        // (`minted`), so the fleet keeps exactly ONE row for
                        // this name instead of two half-identities.
                        return NodeAdmission::Admitted {
                            node_id: existing,
                            minted: true,
                        };
                    }
                    // Several rows already claim this name: there is no single
                    // row to converge on, and a backfill would make it worse.
                    // Adopt the presented id WITHOUT persisting it — the node
                    // keeps working, it just does not appear in the offline
                    // fleet view until the duplicates are repaired by id.
                    Ok(NameMatch::Ambiguous(n)) => {
                        warn!(
                            node_name = %node_name,
                            count = n,
                            "unknown node id presented under an ambiguous name; \
                             not backfilling a duplicate row"
                        );
                        return NodeAdmission::Admitted {
                            node_id: id.to_string(),
                            minted: false,
                        };
                    }
                    // No row owns this name (or the only one is this very id):
                    // backfill below.
                    Ok(NameMatch::None) | Ok(NameMatch::Unique(_)) => {}
                    // The store could not be read, so whether this name is
                    // already taken is UNKNOWN. Admitting without persisting
                    // would hide the node from the fleet view; backfilling
                    // could duplicate a row the operator cannot see. Neither
                    // is safe — report the store as unavailable and let the
                    // node retry.
                    Err(e) => {
                        warn!(node_id = id, error = %e, "match_by_name store read failed before backfilling the node device record");
                        return NodeAdmission::StoreUnavailable {
                            detail: format!("the enrolled-device records could not be read ({e})"),
                        };
                    }
                }
                // A name that normalizes to an empty key can never be addressed
                // again, so a row stored under it is a permanent ghost: do not
                // create one. The node still gets its presented id back.
                if let Some(reason) = unaddressable_name_reason(&node_name) {
                    warn!(node_id = id, reason = %reason, "not backfilling the node device record");
                    return NodeAdmission::Admitted {
                        node_id: id.to_string(),
                        minted: false,
                    };
                }
                let fingerprint = truncate_on_char_boundary(id, 16);
                if let Err(e) = store.upsert_device(&DeviceUpsertData {
                    device_id: id,
                    device_name: &node_name,
                    device_type: None,
                    public_key: &[0u8; 32],
                    fingerprint,
                    role: "node",
                    scopes: &["node".to_string()],
                    user_id: None,
                }) {
                    warn!(node_id = id, error = %e, "failed to backfill node device record");
                    // A backfill that failed must NOT degrade to "admitted
                    // without a row": the node would come up with an identity
                    // the center cannot see, and the offline fleet view would
                    // lie about the fleet. Retryable — the node dials in again.
                    return NodeAdmission::StoreUnavailable {
                        detail: format!("the node device record could not be written ({e})"),
                    };
                }
                return NodeAdmission::Admitted {
                    node_id: id.to_string(),
                    minted: false,
                };
            }
            Err(e) => {
                // The store could not be read, so whether this node's record is
                // revoked is UNKNOWN. Admitting on LAN trust would let a
                // deregistered node straight back in — the sticky
                // deregistration would stop sticking.
                warn!(node_id = id, error = %e, "node device lookup failed");
                return NodeAdmission::StoreUnavailable {
                    detail: format!("the enrolled-device records could not be read ({e})"),
                };
            }
        }
    }

    // First boot: adopt an operator's pre-enrolled row for this name if there is
    // exactly one, else mint. Either way the node persists what we hand back.
    //
    // If the store is wedged during the read we do NOT mint past it: a fresh
    // mint under a name an operator already pre-enrolled would be exactly the
    // duplicate row this module exists to prevent, and the center cannot tell
    // the node apart from the pre-enrolled one. The store being unavailable is
    // reported as such — retryable, and honest.
    //
    // The read-then-mint sequence is serialized by `NODE_NAME_LOCK`: an
    // enrollment racing this first boot could otherwise both read "no such
    // name" and both insert, which is the duplicate cascade this module exists
    // to prevent.
    let _guard = NODE_NAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match match_by_name(store, &node_name) {
        Ok(NameMatch::Unique(existing)) => {
            return NodeAdmission::Admitted {
                node_id: existing,
                minted: true,
            }
        }
        // Several active rows normalize to this name. Minting a third row
        // under it would bury the fleet in same-name rows that nobody — not
        // `cluster.deregister`, not the operator's by-name fallback — can
        // address any more. The node path now refuses, exactly like the
        // operator path: the duplicates have to be repaired by id first. This
        // is terminal, so the node exits with an actionable message instead of
        // retrying a request that can never succeed.
        Ok(NameMatch::Ambiguous(n)) => {
            warn!(
                node_name = %node_name,
                count = n,
                "multiple enrolled nodes share this name; refusing admission"
            );
            return NodeAdmission::Ambiguous {
                name: node_name.clone(),
                count: n,
            };
        }
        Ok(NameMatch::None) => {}
        Err(e) => {
            warn!(
                node_name = %node_name,
                error = %e,
                "match_by_name store read failed"
            );
            return NodeAdmission::StoreUnavailable {
                detail: format!("the enrolled-device records could not be read ({e})"),
            };
        }
    }
    match mint_node_device(store, &node_name) {
        Ok(node_id) => NodeAdmission::Admitted {
            node_id,
            minted: true,
        },
        Err(MintError::Unaddressable(reason)) => {
            // The name can never be addressed again, so a row stored under it
            // would be a permanent ghost. Give the node a usable (ephemeral)
            // identity rather than refusing it outright — it just never shows
            // up in the offline fleet view.
            warn!(
                node_name = %node_name,
                reason = %reason,
                "node name is not addressable; using an ephemeral id"
            );
            NodeAdmission::Admitted {
                node_id: uuid::Uuid::new_v4().to_string(),
                minted: true,
            }
        }
        Err(MintError::Store(e)) => {
            // The write failed: the node must not come up with an identity the
            // center cannot see. Retryable — the node dials in again.
            warn!(node_name = %node_name, error = %e, "node enrollment failed");
            NodeAdmission::StoreUnavailable {
                detail: format!("the node device record could not be written ({e})"),
            }
        }
    }
}

/// Why a deregister could not name a single node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeregisterError {
    /// Neither an online session nor an enrolled device record matches.
    NotFound,
    /// The query names more than one node, or could not be resolved to exactly
    /// one node at all (an ambiguous exact name); carries a renderable
    /// explanation.
    Ambiguous(String),
    /// The enrolled-device store could not be read or written. A **server**
    /// fault, not an operator mistake: the caller must not report it as a bad
    /// request, and — because the durable revocation is what makes a
    /// deregistration stick — it must not report success either.
    StoreUnavailable(String),
}

impl std::fmt::Display for DeregisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no online or enrolled node matches"),
            Self::Ambiguous(m) | Self::StoreUnavailable(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DeregisterError {}

/// What a deregister actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeregisterOutcome {
    pub node_id: String,
    /// A live session was evicted (and its socket asked to close).
    pub evicted: bool,
    /// The device record was revoked, which is what makes it **stick**.
    pub device_removed: bool,
}

/// Offline-fallback addressing: resolve from the store's registered node
/// devices (role=node, non-revoked) by ① exact `device_id` ② unique normalized
/// `device_name`. Fuzzy prefix/substring is deliberately NOT accepted here — an
/// operator removing a machine should name it precisely — but name equality
/// uses the same [`normalize_node_key`] as online addressing, so "GPU Box" is
/// removable as "gpu-box" whether it is online or not.
///
/// "No row matches" ([`NameMatch::None`]) and "several rows share this exact
/// name" ([`NameMatch::Ambiguous`]) stay DISTINCT: the caller has to be able
/// to report the second one instead of resolving it to some other candidate.
///
/// A store read failure is returned as `Err` rather than being flattened into
/// "not found": the caller falls back to the registry's fuzzy tier when the
/// precise lookup finds nothing, and "the store is wedged" must not be
/// mistaken for "no such node" — that fallback could evict the wrong node.
///
/// This tier reads the **active-only** list on purpose: a revoked row must
/// never compete for a name. The one tier that may see revoked rows is the
/// exact-`device_id` branch of [`resolve_deregister_target`], which uses
/// `get_device` instead.
fn resolve_enrolled_node(store: &SecurityStore, q: &str) -> Result<NameMatch, rusqlite::Error> {
    // One read of `list_devices`, then both the exact-id and the
    // normalized-name checks walk the same `Vec`. The previous shape called
    // `store.list_devices()` directly and then `match_by_name` which called
    // it again — two full table scans per offline-lookup.
    let devices = store.list_devices()?;
    if let Some(d) = devices
        .iter()
        .find(|d| d.role == "node" && d.device_id == q)
    {
        return Ok(NameMatch::Unique(d.device_id.clone()));
    }
    Ok(match_name_in(&devices, q))
}

/// Match active `role=node` devices in `devices` by normalized name — same
/// semantics as [`match_by_name`] but over an already-loaded `Vec`, so
/// `resolve_enrolled_node` can fold the exact-id and name checks into a
/// single store read.
fn match_name_in(devices: &[DeviceRow], q: &str) -> NameMatch {
    let key = normalize_node_key(q);
    if key.is_empty() {
        return NameMatch::None;
    }
    let mut hits = devices
        .iter()
        .filter(|d| d.role == "node" && normalize_node_key(&d.device_name) == key);
    let Some(first) = hits.next() else {
        return NameMatch::None;
    };
    let extra = hits.count();
    if extra > 0 {
        NameMatch::Ambiguous(extra + 1)
    } else {
        NameMatch::Unique(first.device_id.clone())
    }
}

/// Deregister a node — the single source of truth behind both the
/// `cluster.deregister` RPC and the `node_manage` tool, so the Panel and the
/// model cannot end up with different takedown semantics (R4: the RPC handler
/// stays pure I/O).
///
/// Two-phase takedown, in this order:
/// ① `revoke_device` sets `revoked_at` on the node's enrolled-device record,
///    which is what makes deregistration **stick**: the node's next `connect`
///    meets [`NodeAdmission::Deregistered`] and it exits instead of self-reviving
///    on the following backoff.
/// ② `NodeRegistry::forget` evicts the live session — it vanishes from
///    `environments.list`, stops being addressable by `node_invoke`/`node_file`,
///    and its connection is asked to close (so the revoked node cannot keep
///    running dispatched work or raising approval cards).
///
/// **The revocation comes first, and it is the authoritative one.** A failed
/// revoke used to be logged at `warn!` while the RPC still answered `Ok` — the
/// node was kicked but could come straight back, reported as success. Now a
/// failed write is an error ([`DeregisterError::StoreUnavailable`]): nothing is
/// evicted, no `node.disconnected` event is published, and the caller is told
/// the takedown did not happen. A row that was *already* revoked is an
/// **idempotent success**, not an error.
///
/// Addressing tries the PRECISE resolution first — the enrolled-device store's
/// exact `device_id` and its unique exact normalized `device_name` — and only
/// falls back to the online registry (whose match may be fuzzy) when no
/// enrolled row matches at all. See [`resolve_deregister_target`] for why the
/// order matters.
///
/// **Contract**: `Ok(DeregisterOutcome{..})` means the node's enrolled-device
/// record is revoked (or was already revoked) **and** the live session, if
/// there was one, was evicted. The `device_removed` field distinguishes
/// "newly revoked" (`true`) from "already revoked" (`false`) — both are
/// successes — while `evicted` reports whether a live session was there to
/// evict.
pub fn deregister_node(
    registry: &crate::cluster::NodeRegistry,
    store: &SecurityStore,
    query: &str,
    event_bus: Option<&crate::gateway::event_bus::GatewayEventBus>,
) -> Result<DeregisterOutcome, DeregisterError> {
    let node_id = resolve_deregister_target(registry, store, query)?;

    // REVOKE FIRST: the durable revocation is the part that sticks. If the
    // write fails, the node is still enrolled and would be admitted again on
    // its next reconnect — so the failure is reported as an error and NOTHING
    // else happens: no eviction, no event, no success.
    let device_removed = revoke_node_device(store, &node_id)?;

    let evicted_session = registry.forget(&node_id);
    let evicted = evicted_session.is_some();

    // Fire the operator-initiated `node.disconnected` event from the same
    // single source the WebSocket drop arm uses (`server/connection/cleanup.rs`). Without
    // this, an explicit `cluster.deregister` (RPC or `node_manage` tool)
    // evicts a live session but emits no event — the operator's Panel keeps
    // showing the node online until a hard refresh, after which it vanishes
    // entirely, so the transition is unobservable. `forget` returning the
    // evicted session (the audit's sw-cluster-3 ask) avoids a second lookup.
    if let (Some(session), Some(bus)) = (evicted_session, event_bus) {
        use crate::gateway::event_bus::TopicEvent;
        if let Err(e) = bus.publish_json(&TopicEvent::new(
            "node.disconnected",
            serde_json::json!({
                "node_id": session.node_id,
                "name": session.device_name,
                "conn_id": session.conn_id,
            }),
        )) {
            tracing::warn!(
                error = %e,
                node_id = %session.node_id,
                "failed to publish node.disconnected event on operator deregister"
            );
        }
    }

    Ok(DeregisterOutcome {
        node_id,
        evicted,
        device_removed,
    })
}

/// Resolve the node a deregister request targets — **precise first, fuzzy
/// last**:
///
/// ① the enrolled-device store's exact `device_id` — the only tier that may
///    look at **revoked** rows, so a repeat deregistration of an
///    already-deregistered node resolves to the same node instead of "not
///    found";
/// ② the store's UNIQUE exact normalized `device_name` (the offline path — an
///    enrolled node that is not connected has no registry entry at all). This
///    tier reads the **active-only** list: a revoked row must never compete for
///    a name, must never win over an active row, and must never turn an
///    ambiguous name into a no-op;
/// ③ the online registry, whose match may fall back to its FUZZY tier.
///
/// The registry's fuzzy tier matches an id PREFIX or a query that CONTAINS a
/// node's normalized name, and it used to be consulted FIRST. That let an
/// operator (or the model, through the `node_manage` tool) asking to remove an
/// OFFLINE enrolled node named `worker` land on the ONLINE `worker-2`: the
/// innocent live node was evicted AND its device row revoked — sticky, since
/// `admit_node` answers `Deregistered` for a revoked row, so it can never
/// rejoin — while the intended node stayed enrolled and the operator was told
/// the removal succeeded.
///
/// An ambiguous exact name is reported as ambiguous and NEVER resolved to a
/// fuzzy candidate: a fleet with duplicate names is repaired by id, not by
/// guessing.
fn resolve_deregister_target(
    registry: &crate::cluster::NodeRegistry,
    store: &SecurityStore,
    query: &str,
) -> Result<String, DeregisterError> {
    use crate::cluster::ResolveError;
    // ① EXACT `device_id`. `get_device` returns revoked rows too, which is
    // what makes a repeat deregistration idempotent: the node id is still
    // resolvable after the row was revoked, so the operator is told "already
    // deregistered" instead of "not found". A non-node row (an operator or
    // Panel device) is NOT a node — it falls through to the name tier.
    match store.get_device(query) {
        Ok(Some(row)) if row.role == "node" => return Ok(row.device_id.clone()),
        Ok(Some(_)) => {}
        Ok(None) => {}
        Err(e) => {
            // The store could not be read: whether the exact id names an
            // enrolled node is UNKNOWN, and the registry's fuzzy tier could
            // then evict the wrong node. Refuse instead of guessing.
            return Err(DeregisterError::StoreUnavailable(format!(
                "could not be resolved — the enrolled-node records could not be read ({e}); \
                 refusing to guess which node to remove"
            )));
        }
    }
    match resolve_enrolled_node(store, query) {
        Ok(NameMatch::Unique(id)) => Ok(id),
        Ok(NameMatch::Ambiguous(n)) => Err(DeregisterError::Ambiguous(format!(
            "ambiguous — {n} enrolled nodes share this exact name; deregister them by id"
        ))),
        // No enrolled row: the node never wrote one (the ephemeral-id path, or
        // an online session whose row was revoked). Only now may the registry
        // match — and it may match fuzzily.
        Ok(NameMatch::None) => match registry.resolve_id(query) {
            Ok(id) => Ok(id),
            Err(ResolveError::NotFound) => Err(DeregisterError::NotFound),
            Err(e @ ResolveError::Ambiguous(_)) => Err(DeregisterError::Ambiguous(e.to_string())),
        },
        // The store read failed: whether an enrolled row matches is UNKNOWN,
        // and the registry's fuzzy tier could then evict the wrong node.
        // Refuse instead of guessing.
        Err(e) => Err(DeregisterError::StoreUnavailable(format!(
            "could not be resolved — the enrolled-node records could not be read ({e}); \
             refusing to guess which node to remove"
        ))),
    }
}

/// Revoke a node's enrolled-device record; `Ok(true)` iff the row was newly
/// revoked, `Ok(false)` iff there was nothing to revoke (already revoked, or
/// no enrolled row at all).
///
/// `revoke_device` updates `WHERE device_id = ? AND revoked_at IS NULL` and
/// answers `false` for BOTH "already revoked" and "no such row", while a
/// failed write surfaces as `Err`. The three states mean very different things
/// to an operator — "the device was already revoked" is an idempotent success,
/// "the write failed" is a server fault — so they are told apart here instead
/// of being flattened into one `false` that reads as a half-done takedown.
///
/// A failure is reported as [`DeregisterError::StoreUnavailable`]: the caller
/// must NOT evict the live session and must NOT report success, because the
/// node would come straight back on its next reconnect.
fn revoke_node_device(store: &SecurityStore, node_id: &str) -> Result<bool, DeregisterError> {
    match store.revoke_device(node_id) {
        Ok(true) => Ok(true),
        Ok(false) => match store.get_device(node_id) {
            Ok(Some(row)) if row.revoked_at.is_some() => {
                warn!(
                    node_id,
                    "node device was already revoked; nothing to revoke"
                );
                Ok(false)
            }
            Ok(Some(_)) => {
                // The row exists and is still ACTIVE, yet the revoke did not
                // take: the write silently failed. Reporting success would be
                // a lie the node undoes by reconnecting.
                Err(DeregisterError::StoreUnavailable(format!(
                    "failed to revoke the node device '{node_id}': the device record was not revoked"
                )))
            }
            Ok(None) => {
                // No enrolled row at all (an online-only node, or one whose row
                // was removed): there is nothing durable to revoke, so the
                // eviction below is the whole takedown.
                warn!(
                    node_id,
                    "node has no enrolled device record; nothing to revoke"
                );
                Ok(false)
            }
            Err(e) => Err(DeregisterError::StoreUnavailable(format!(
                "failed to revoke the node device '{node_id}': the device record could not be read ({e})"
            ))),
        },
        Err(e) => Err(DeregisterError::StoreUnavailable(format!(
            "failed to revoke the node device '{node_id}': {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;

    fn store() -> SecurityStore {
        SecurityStore::in_memory().expect("in-memory store")
    }

    #[test]
    fn first_boot_mints_and_tells_node_to_persist() {
        let s = store();
        let a = admit_node(&s, None, "worker-1");
        let NodeAdmission::Admitted { node_id, minted } = a else {
            panic!("first boot must be admitted");
        };
        assert!(minted, "node must persist the freshly minted id");
        let devices = s.list_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, node_id);
        assert_eq!(devices[0].role, "node");
    }

    #[test]
    fn reconnect_with_persisted_id_reuses_the_same_row() {
        let s = store();
        let NodeAdmission::Admitted { node_id, .. } = admit_node(&s, None, "worker-1") else {
            panic!("admitted");
        };
        let again = admit_node(&s, Some(&node_id), "worker-1");
        assert_eq!(
            again,
            NodeAdmission::Admitted {
                node_id: node_id.clone(),
                minted: false
            }
        );
        // No ghost rows: still exactly one enrolled node.
        assert_eq!(s.list_devices().unwrap().len(), 1);
    }

    #[test]
    fn first_boot_adopts_the_operators_pre_enrolled_row_by_normalized_name() {
        let s = store();
        // Operator pre-enrolls via `cluster.enroll` with a spaced, mixed-case name.
        let pre = mint_node_device(&s, "GPU Box").unwrap();
        // The node dials in with the dash-spelled variant and no persisted id.
        let a = admit_node(&s, None, "gpu-box");
        assert_eq!(
            a,
            NodeAdmission::Admitted {
                node_id: pre,
                minted: true
            },
            "must adopt the pre-enrolled row, not mint a duplicate"
        );
        assert_eq!(
            s.list_devices().unwrap().len(),
            1,
            "no duplicate ghost row may appear in the offline fleet view"
        );
    }

    #[test]
    fn first_boot_adopts_pre_enrolled_cjk_name_without_churn() {
        let s = store();
        // Operator pre-enrolls a Chinese-named node.
        let pre = mint_node_device(&s, "工作站").unwrap();
        // The node dials in with the same CJK name and no persisted id. It must
        // ADOPT the pre-enrolled row, not mint a duplicate. The ASCII-only
        // normalize_node_key used to fold "工作站" to "" → reuse_by_name returned
        // None → a fresh id every boot, proliferating offline ghost rows.
        let a = admit_node(&s, None, "工作站");
        assert_eq!(
            a,
            NodeAdmission::Admitted {
                node_id: pre,
                minted: true
            },
            "CJK-named node must adopt its pre-enrolled row, not mint a duplicate"
        );
        assert_eq!(
            s.list_devices().unwrap().len(),
            1,
            "no duplicate CJK ghost row may appear across reconnects"
        );
    }

    #[test]
    fn deregistered_node_is_refused_on_reconnect() {
        let s = store();
        let NodeAdmission::Admitted { node_id, .. } = admit_node(&s, None, "worker-1") else {
            panic!("admitted");
        };
        // Operator runs cluster.deregister → the device row is revoked.
        assert!(s.revoke_device(&node_id).unwrap());

        // The node still holds its identity file and dials back in.
        assert_eq!(
            admit_node(&s, Some(&node_id), "worker-1"),
            NodeAdmission::Deregistered {
                node_id: node_id.clone()
            },
            "deregistration must stick across the node's reconnect backoff"
        );
    }

    /// Mirror of `exchange_bootstrap_ticket`'s namespace guard, from the node
    /// side: `devices` is one table shared with paired Panels and both ids are
    /// peer-asserted, so a node-shaped `connect` presenting a Panel's device id
    /// must be refused — not adopted. Adopting it let the squatter take over the
    /// Panel's `last_seen_at` and put its credential one `cluster.deregister`
    /// away from being revoked.
    #[test]
    fn node_claiming_a_paired_panel_id_is_refused() {
        let s = store();
        s.upsert_device(&DeviceUpsertData {
            device_id: "device-abc",
            device_name: "Chrome on macOS",
            device_type: Some(crate::gateway::security::PANEL_DEVICE_TYPE),
            public_key: b"",
            fingerprint: "device-abc",
            role: "operator",
            scopes: &["*".to_string()],
            user_id: None,
        })
        .unwrap();

        assert_eq!(
            admit_node(&s, Some("device-abc"), "worker-1"),
            NodeAdmission::IdentityConflict {
                node_id: "device-abc".to_string()
            }
        );
        // And the Panel row keeps its type — no silent takeover.
        let row = s.get_device("device-abc").unwrap().unwrap();
        assert_eq!(
            row.device_type.as_deref(),
            Some(crate::gateway::security::PANEL_DEVICE_TYPE)
        );
    }

    #[test]
    fn unknown_id_is_adopted_and_backfilled() {
        let s = store();
        // Center DB was reset; the node still has its persisted identity.
        let a = admit_node(&s, Some("orphan-id"), "worker-1");
        assert_eq!(
            a,
            NodeAdmission::Admitted {
                node_id: "orphan-id".to_string(),
                minted: false
            }
        );
        let devices = s.list_devices().unwrap();
        assert_eq!(devices.len(), 1, "the row is backfilled");
        assert_eq!(devices[0].device_id, "orphan-id");
    }

    #[test]
    fn enroll_is_idempotent_across_name_spellings() {
        let s = store();
        let (first, minted) = enroll_node_device(&s, "GPU Box").unwrap();
        assert!(minted);
        // A double-clicked "+ Enroll", a retried RPC, or the operator asking the
        // model twice must all land on the SAME record. Minting a second row is
        // self-sealing damage: it makes the node's own by-name merge ambiguous
        // (so first boot mints a third) and makes the offline deregister
        // fallback, which needs a unique name, unable to remove any of them.
        let (again, minted_again) = enroll_node_device(&s, "gpu-box").unwrap();
        assert_eq!(again, first);
        assert!(!minted_again, "an existing row is reused, not re-minted");
        assert_eq!(s.list_devices().unwrap().len(), 1);
    }

    #[test]
    fn enroll_refuses_to_deepen_a_pre_existing_duplicate() {
        let s = store();
        // A fleet already damaged by the old unconditional mint.
        mint_node_device(&s, "Worker 1").unwrap();
        mint_node_device(&s, "worker-1").unwrap();
        let err = enroll_node_device(&s, "worker-1").expect_err("must refuse");
        match err {
            // An operator mistake, not a server fault (D10).
            EnrollError::Ambiguous(detail) => {
                assert!(detail.contains("already share the name"), "{detail}");
            }
            other => panic!("must refuse with an ambiguity error, got {other:?}"),
        }
        assert_eq!(
            s.list_devices().unwrap().len(),
            2,
            "refusing must not add a third row"
        );
    }

    #[test]
    fn admit_node_survives_a_non_ascii_presented_id() {
        // Path ③ (unknown id) backfills a device row whose fingerprint is the
        // id's first 16 bytes. A peer-supplied id is not guaranteed to be an
        // ASCII UUID, and byte 16 of this one lands mid-scalar — `&id[..16]`
        // panicked the whole connection task (P7).
        let s = store();
        // 15 ASCII bytes, then a 3-byte scalar spanning bytes 15..18.
        let id = "aleph-node-1234工作站";
        assert!(!id.is_char_boundary(16), "fixture must exercise the hazard");
        assert_eq!(
            admit_node(&s, Some(id), "worker-1"),
            NodeAdmission::Admitted {
                node_id: id.to_string(),
                minted: false
            }
        );
        assert_eq!(s.list_devices().unwrap().len(), 1, "row is backfilled");
    }

    #[test]
    fn deregister_is_sticky_and_reaches_offline_nodes() {
        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        let (node_id, _) = enroll_node_device(&s, "cold-box").unwrap();

        // Never connected → nothing to evict, but the revoke still lands.
        let out = deregister_node(&reg, &s, "Cold Box", None).expect("offline deregister");
        assert_eq!(
            out,
            DeregisterOutcome {
                node_id: node_id.clone(),
                evicted: false,
                device_removed: true
            }
        );
        // Sticky: if that machine dials back in with its persisted id it is
        // refused rather than silently rejoining on the next backoff.
        assert_eq!(
            admit_node(&s, Some(&node_id), "cold-box"),
            NodeAdmission::Deregistered { node_id }
        );
        assert_eq!(
            deregister_node(&reg, &s, "cold-box", None),
            Err(DeregisterError::NotFound),
            "a revoked node is no longer addressable"
        );
    }

    #[test]
    fn deregister_publishes_node_disconnected_on_live_eviction() {
        // Regression for sw-cluster-3: operator-initiated `deregister_node`
        // (RPC or `node_manage` tool) previously dropped a live session
        // without emitting the `node.disconnected` lifecycle event, so the
        // operator's Panel kept showing the node online until a hard refresh
        // (after which it vanished entirely). The publish now fires from the
        // shared single source.
        use crate::cluster::{CommandDescriptor, NodeSession, ReverseRpcChannel};
        use crate::gateway::event_bus::GatewayEventBus;
        use tokio::sync::mpsc;

        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        let (node_id, _minted) = enroll_node_device(&s, "live-box").unwrap();

        // Bring the node online so the deregister is an *eviction* (not just
        // a device revocation).
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: node_id.clone(),
            conn_id: "conn-live-box".to_string(),
            device_name: "live-box".to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }],
            tags: vec![],
            version: None,
            connected_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        });

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        let out = deregister_node(&reg, &s, "live-box", Some(&bus))
            .expect("online deregister must succeed");
        assert!(out.evicted, "live session must be evicted");
        assert!(out.device_removed, "device record must be revoked");

        // One publish, with the right id + name + conn_id. `subscribe()`
        // yields the JSON-serialized `TopicEvent`; we parse and check.
        let frame = rx.try_recv().expect("one event must have been published");
        let event: serde_json::Value =
            serde_json::from_str(&frame).expect("frame must be valid JSON");
        assert_eq!(event["topic"], serde_json::json!("node.disconnected"));
        assert_eq!(event["data"]["node_id"], serde_json::json!(node_id));
        assert_eq!(event["data"]["name"], serde_json::json!("live-box"));
        assert_eq!(event["data"]["conn_id"], serde_json::json!("conn-live-box"));
    }

    /// A name two ACTIVE rows already own cannot be resolved to "this
    /// machine". The old policy minted a THIRD row under it, which buried the
    /// fleet in rows nobody can address (and every later first boot of that
    /// name minted another one). The node path now obeys the same rule as the
    /// operator path: refuse, and let the operator repair the duplicates by id.
    #[test]
    fn ambiguous_first_boot_is_refused_instead_of_minting_a_third_row() {
        let s = store();
        mint_node_device(&s, "Worker 1").unwrap();
        mint_node_device(&s, "worker-1").unwrap();
        assert_eq!(
            admit_node(&s, None, "worker-1"),
            NodeAdmission::Ambiguous {
                name: "worker-1".to_string(),
                count: 2
            },
            "an ambiguous name must fail closed"
        );
        assert_eq!(
            s.list_devices().unwrap().len(),
            2,
            "no third row may be minted past an ambiguity"
        );
    }

    /// The fuzzy registry tier must never decide **who** gets removed.
    ///
    /// An operator (or the model, through the `node_manage` tool) asking to
    /// deregister the OFFLINE enrolled node "worker" was fuzzy-matched against
    /// the ONLINE "worker-2" — the registry matches a query that CONTAINS a
    /// node's normalized name. The innocent live node was evicted AND its
    /// device row revoked (sticky: `admit_node` answers `Deregistered` for a
    /// revoked row, so it can never rejoin), while the intended node stayed
    /// enrolled and the operator was told the removal succeeded.
    #[test]
    fn deregister_does_not_fuzzy_hit_a_live_neighbour() {
        use crate::cluster::{NodeSession, ReverseRpcChannel};
        use tokio::sync::mpsc;

        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        // The intended target: enrolled, never connected.
        let (worker_id, _) = enroll_node_device(&s, "worker").unwrap();
        // The innocent neighbour: enrolled AND online.
        let (worker2_id, _) = enroll_node_device(&s, "worker-2").unwrap();
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: worker2_id.clone(),
            conn_id: "conn-worker-2".to_string(),
            device_name: "worker-2".to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });

        let out = deregister_node(&reg, &s, "worker", None).expect("deregister must succeed");
        assert_eq!(
            out.node_id, worker_id,
            "the OFFLINE node named 'worker' is the target"
        );
        assert!(!out.evicted, "the target was never connected");
        assert!(out.device_removed, "the target's device row is revoked");

        // The live neighbour is untouched: still registered, still enrolled.
        assert!(
            reg.resolve_id("worker-2").is_ok(),
            "the live neighbour must still be registered"
        );
        let neighbour = s
            .get_device(&worker2_id)
            .unwrap()
            .expect("the neighbour's row must still exist");
        assert!(
            neighbour.revoked_at.is_none(),
            "the live neighbour must NOT be revoked"
        );
    }

    /// A node presenting an id the center does not know, under a name an
    /// enrolled row already owns, must CONVERGE on that row instead of
    /// shadowing it: a second row under the same name makes the by-name match
    /// ambiguous, the offline deregister fallback (which needs a unique name)
    /// can no longer address either of them, and every later first boot of that
    /// name mints yet another ghost row.
    #[test]
    fn unknown_id_under_an_enrolled_name_converges_instead_of_duplicating() {
        let s = store();
        // An operator pre-enrolled "worker-3".
        let (existing_id, _) = enroll_node_device(&s, "worker-3").unwrap();
        // The node presents an id this center does not know, under that name.
        assert_eq!(
            admit_node(&s, Some("orphan-id"), "worker-3"),
            NodeAdmission::Admitted {
                node_id: existing_id.clone(),
                minted: true
            },
            "the node must converge on the row that already owns the name"
        );
        let rows: Vec<DeviceRow> = s
            .list_devices()
            .unwrap()
            .into_iter()
            .filter(|d| d.role == "node" && normalize_node_key(&d.device_name) == "worker-3")
            .collect();
        assert_eq!(
            rows.len(),
            1,
            "the name must still own exactly ONE row, got {:?}",
            rows.iter().map(|r| &r.device_id).collect::<Vec<_>>()
        );
    }

    /// The name → row resolution is serialized (`NODE_NAME_LOCK`); the
    /// observable contract is that a repeated enrollment of the SAME name lands
    /// on the same row instead of adding a duplicate.
    #[test]
    fn enrolling_the_same_name_twice_leaves_one_row() {
        let s = store();
        let (first, minted) = enroll_node_device(&s, "worker-4").unwrap();
        assert!(minted);
        let (again, minted_again) = enroll_node_device(&s, "worker-4").unwrap();
        assert_eq!(again, first);
        assert!(!minted_again, "an existing row is reused, not re-minted");
        assert_eq!(s.list_devices().unwrap().len(), 1);
    }

    /// `devices` also holds operator/panel rows. A node must not be able to
    /// claim one as its node identity — the offline deregister fallback filters
    /// on `role == "node"`, so such a node would be un-removable while
    /// offline.
    #[test]
    fn node_claiming_a_non_node_role_is_refused() {
        let s = store();
        s.upsert_device(&DeviceUpsertData {
            device_id: "device-operator",
            device_name: "Ops Laptop",
            device_type: None,
            public_key: b"",
            fingerprint: "device-operator",
            role: "operator",
            scopes: &["*".to_string()],
            user_id: None,
        })
        .unwrap();

        assert_eq!(
            admit_node(&s, Some("device-operator"), "worker-1"),
            NodeAdmission::IdentityConflict {
                node_id: "device-operator".to_string()
            },
            "a non-node row must not be adopted as a node identity"
        );
    }

    /// A name that normalizes to an empty key can never be addressed again, so
    /// a row stored under it is a permanent ghost — and a repeat enrollment
    /// would mint a SECOND one, breaking the documented idempotence.
    #[test]
    fn enroll_refuses_a_name_that_can_never_be_addressed() {
        let s = store();
        let err = enroll_node_device(&s, "  -_- ").expect_err("must refuse");
        match err {
            EnrollError::Unaddressable(detail) => {
                assert!(detail.contains("not addressable"), "{detail}");
            }
            other => panic!("must refuse with an unaddressable-name error, got {other:?}"),
        }
        assert_eq!(
            s.list_devices().unwrap().len(),
            0,
            "no un-addressable row may be stored"
        );
        // A repeat enrollment must not mint a second ghost either.
        assert!(enroll_node_device(&s, "").is_err());
        assert_eq!(s.list_devices().unwrap().len(), 0);
    }

    /// The backfill path must refuse the same name: the node is admitted (it
    /// still needs a usable identity) but no un-addressable row is written.
    #[test]
    fn unknown_id_with_an_unaddressable_name_is_not_backfilled() {
        let s = store();
        assert_eq!(
            admit_node(&s, Some("orphan-id"), "  -_- "),
            NodeAdmission::Admitted {
                node_id: "orphan-id".to_string(),
                minted: false
            }
        );
        assert_eq!(
            s.list_devices().unwrap().len(),
            0,
            "a row that can never be addressed must not be stored"
        );
    }

    /// `revoke_device` answers `false` both for "already revoked" and for "no
    /// such row". The outcome keeps its meaning (nothing was removed), while
    /// the log distinguishes the two — the already-revoked case is a no-op, not
    /// a failed write.
    #[test]
    fn deregister_reports_no_device_removal_for_an_already_revoked_row() {
        use crate::cluster::{NodeSession, ReverseRpcChannel};
        use tokio::sync::mpsc;

        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        let (node_id, _) = enroll_node_device(&s, "half-revoked").unwrap();
        // The device row is already revoked (an earlier takedown landed) while
        // the session is still online.
        assert!(s.revoke_device(&node_id).unwrap());
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: node_id.clone(),
            conn_id: "conn-half-revoked".to_string(),
            device_name: "half-revoked".to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });

        let out = deregister_node(&reg, &s, "half-revoked", None).expect("must succeed");
        assert!(out.evicted, "the live session is evicted");
        assert!(
            !out.device_removed,
            "the device row was ALREADY revoked — nothing was removed"
        );
    }

    /// Break the `devices` table so every read and write fails — the shape of a
    /// store outage, produced without touching the production code.
    fn break_the_devices_table(s: &SecurityStore) {
        let conn = s.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch("DROP TABLE devices")
            .expect("must drop the devices table");
    }

    /// A peer-presented id is written verbatim into the `devices` table as the
    /// key of the very row that makes the node addressable, so an over-long one
    /// is refused outright and the node falls back to the ordinary first-boot
    /// path.
    #[test]
    fn an_over_long_presented_id_is_refused_and_the_node_falls_back_to_first_boot() {
        let s = store();
        let long_id = "a".repeat(MAX_PRESENTED_DEVICE_ID_BYTES + 1);

        match admit_node(&s, Some(long_id.as_str()), "worker-1") {
            NodeAdmission::Admitted { node_id, minted } => {
                assert!(minted, "the node gets a fresh identity to persist");
                assert_ne!(node_id, long_id, "the over-long id must not be adopted");
                assert!(
                    node_id.len() <= MAX_PRESENTED_DEVICE_ID_BYTES,
                    "the handed-out id must respect the bound, got {} bytes",
                    node_id.len()
                );
            }
            other => panic!("the node must still come up, got {other:?}"),
        }

        let rows = s.list_devices().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "exactly one row — the over-long id is never stored"
        );
        assert_ne!(rows[0].device_id, long_id);
    }

    /// A store outage must never be degraded into an admission: the center
    /// cannot tell whether the node's row is revoked, so admitting it would
    /// let a deregistered node straight back in — and minting would bury a
    /// pre-enrolled row the operator cannot see. The outcome is retryable.
    #[test]
    fn a_store_outage_is_retryable_and_never_an_admission() {
        let s = store();
        break_the_devices_table(&s);

        match admit_node(&s, Some("aleph-node-1"), "worker-1") {
            NodeAdmission::StoreUnavailable { detail } => {
                assert!(detail.contains("could not be read"), "{detail}");
            }
            other => panic!("a store outage must not admit the node, got {other:?}"),
        }

        match admit_node(&s, None, "worker-1") {
            NodeAdmission::StoreUnavailable { detail } => {
                assert!(detail.contains("could not be read"), "{detail}");
            }
            other => panic!("a store outage must not mint a node, got {other:?}"),
        }
    }

    /// Repeating a deregistration **by exact id** is an idempotent success:
    /// the exact-id branch may look at revoked rows, so the operator who
    /// clicks twice is told "already gone", not "not found".
    #[test]
    fn a_repeat_deregistration_by_exact_id_is_idempotent() {
        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        let (node_id, _) = enroll_node_device(&s, "repeat-box").unwrap();

        let first = deregister_node(&reg, &s, &node_id, None).expect("first deregister");
        assert!(first.device_removed, "the row is revoked the first time");

        let second = deregister_node(&reg, &s, &node_id, None).expect("repeat must be idempotent");
        assert_eq!(second.node_id, node_id);
        assert!(
            !second.device_removed,
            "nothing was removed the second time"
        );
        assert!(!second.evicted, "no session was online");

        // The NAME branch still only sees active rows: a revoked node is no
        // longer addressable by name.
        assert_eq!(
            deregister_node(&reg, &s, "repeat-box", None),
            Err(DeregisterError::NotFound),
        );
    }

    /// A store fault during a deregistration is a **server** error: nothing is
    /// evicted, no `node.disconnected` is published, and the operator is not
    /// told the node is gone while it is still online.
    #[test]
    fn a_failed_deregistration_evicts_nothing() {
        use crate::cluster::{NodeSession, ReverseRpcChannel};
        use tokio::sync::mpsc;

        let s = store();
        let reg = crate::cluster::NodeRegistry::new();
        let (node_id, _) = enroll_node_device(&s, "faulty-box").unwrap();
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: node_id.clone(),
            conn_id: "conn-faulty-box".to_string(),
            device_name: "faulty-box".to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });

        break_the_devices_table(&s);

        match deregister_node(&reg, &s, &node_id, None) {
            Err(DeregisterError::StoreUnavailable(detail)) => {
                assert!(!detail.is_empty(), "the detail must explain the fault");
            }
            other => panic!("a store fault must not report success, got {other:?}"),
        }

        assert!(
            reg.node_identity_by_conn("conn-faulty-box").is_some(),
            "the live session survives a failed deregistration"
        );
    }
}
