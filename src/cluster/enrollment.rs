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
//! * `connect` seam (`gateway/server/handler.rs`) — node self-registration
//!   (first boot, no id).
//! * `cluster.enroll` RPC (`gateway/handlers/cluster.rs`) — operator
//!   pre-enrollment from the Panel.
//!
//! Both share the same [`mint_node_device`], so a pre-enrolled row and the
//! node's first dial-in are **merged by name** into the same `node_id`, rather
//! than each minting a separate UUID that leaves a permanent ghost offline row.
//!
//! Redline: deterministic table lookup + database writes, no LLM reasoning (R7);
//! does not enter `src/harness/` (R10).

use tracing::warn;

use crate::cluster::normalize_node_key;
use crate::cluster::registry::truncate_on_char_boundary;
use crate::gateway::security::store::{DeviceUpsertData, SecurityStore};

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
    /// (`device_type = "panel"`), not a node. Refused rather than adopted so the
    /// two halves of the shared `devices` namespace cannot take over each
    /// other's rows. Carried as its own variant (not folded into
    /// [`Self::Deregistered`]) so the log line and the regression test stay
    /// honest; both refusals are terminal and share one wire status.
    IdentityConflict { node_id: String },
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
fn mint_node_device(store: &SecurityStore, node_name: &str) -> Result<String, String> {
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
            format!("failed to register node device: {e}")
        })?;
    Ok(device_id)
}

/// Outcome of matching **active** (non-revoked) `role=node` devices by
/// normalized name. Ambiguity is a distinct state rather than a second flavour
/// of "no match": node self-registration mints past it (it has no operator to
/// ask), but operator pre-enrollment must **refuse** — minting past it is what
/// buries the fleet in same-name rows nobody can address (P3 "make illegal
/// states unrepresentable").
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
/// dialing in as `--name gpu-box` merge into the same record. A store read
/// failure degrades to [`NameMatch::None`] (P7: the caller still gets a usable
/// identity; the only cost is one row in the offline view).
fn match_by_name(store: &SecurityStore, node_name: &str) -> NameMatch {
    let key = normalize_node_key(node_name);
    if key.is_empty() {
        return NameMatch::None;
    }
    let Ok(devices) = store.list_devices() else {
        return NameMatch::None;
    };
    let mut hits = devices
        .into_iter()
        .filter(|d| d.role == "node" && normalize_node_key(&d.device_name) == key);
    let Some(first) = hits.next() else {
        return NameMatch::None;
    };
    let extra = hits.count();
    if extra > 0 {
        NameMatch::Ambiguous(extra + 1)
    } else {
        NameMatch::Unique(first.device_id)
    }
}

/// **Idempotent** operator pre-enrollment (`cluster.enroll` / the `node_manage`
/// tool): reserve a fleet slot for `node_name` and return
/// `(node_id, minted)` — `minted=false` means an existing record was returned
/// unchanged.
///
/// Idempotence is the whole point. `mint_node_device` alone made every click of
/// the Panel's "+ Enroll" (or every retried RPC) create another `role=node` row
/// with the same name, and those duplicates are **self-sealing**: the second row
/// makes [`match_by_name`] ambiguous, so the node's own first boot stops
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
) -> Result<(String, bool), String> {
    match match_by_name(store, node_name) {
        NameMatch::Unique(id) => Ok((id, false)),
        NameMatch::None => mint_node_device(store, node_name).map(|id| (id, true)),
        NameMatch::Ambiguous(n) => Err(format!(
            "{n} enrolled nodes already share the name '{node_name}'; \
             deregister the duplicates by id first"
        )),
    }
}

/// Resolve admission for a node `connect`. `presented_id` = the `device_id` from
/// the connect frame (`None` on first boot).
///
/// Decision order:
/// 1. Has id AND that record is **revoked** → [`NodeAdmission::Deregistered`]
///    (deregistration sticks).
/// 2. Has id AND that record is a **paired Panel** → [`NodeAdmission::IdentityConflict`].
///    `devices` is one namespace shared by nodes and paired Panels, and both ids
///    are peer-asserted, so a node claiming a Panel's id would take over that
///    row's `last_seen_at` and put the Panel one `cluster.deregister` away from
///    losing its credential. Mirror of the guard in
///    `DeviceTokenManager::exchange_bootstrap_ticket`.
/// 3. Has id AND record is active → reuse as-is (stable identity).
/// 4. Has id but **no such record** (center DB reset / switched centers) →
///    adopt the id and backfill the row so the node keeps its persisted identity
///    without re-registering.
/// 5. No id (first boot) → try to adopt an operator's pre-enrolled row by name
///    first, else mint new; `minted=true` signals the node to persist the id.
///
/// Store read/write failures always degrade to "adopt/mint without persisting";
/// the node still works (P7 graceful degradation) — the only cost is one fewer
/// record in the offline view.
pub fn admit_node(
    store: &SecurityStore,
    presented_id: Option<&str>,
    node_name: &str,
) -> NodeAdmission {
    if let Some(id) = presented_id.filter(|s| !s.is_empty()) {
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
                let fingerprint = truncate_on_char_boundary(id, 16);
                if let Err(e) = store.upsert_device(&DeviceUpsertData {
                    device_id: id,
                    device_name: node_name,
                    device_type: None,
                    public_key: &[0u8; 32],
                    fingerprint,
                    role: "node",
                    scopes: &["node".to_string()],
                    user_id: None,
                }) {
                    warn!(node_id = id, error = %e, "failed to backfill node device record");
                }
                return NodeAdmission::Admitted {
                    node_id: id.to_string(),
                    minted: false,
                };
            }
            Err(e) => {
                warn!(node_id = id, error = %e, "node device lookup failed; admitting on LAN-trust");
                return NodeAdmission::Admitted {
                    node_id: id.to_string(),
                    minted: false,
                };
            }
        }
    }

    // First boot: adopt an operator's pre-enrolled row for this name if there is
    // exactly one, else mint. Either way the node persists what we hand back.
    match match_by_name(store, node_name) {
        NameMatch::Unique(existing) => {
            return NodeAdmission::Admitted {
                node_id: existing,
                minted: true,
            }
        }
        // Several active rows normalize to this name — refuse to guess which one
        // this node is, and mint it a distinct identity. (Unlike the operator
        // path, a dialling node has nobody to ask; refusing it outright would
        // take the machine offline over a bookkeeping mess.)
        NameMatch::Ambiguous(n) => warn!(
            node_name,
            count = n,
            "multiple enrolled nodes share this name; minting a fresh node id"
        ),
        NameMatch::None => {}
    }
    match mint_node_device(store, node_name) {
        Ok(node_id) => NodeAdmission::Admitted {
            node_id,
            minted: true,
        },
        Err(e) => {
            // Degrade: give the node a usable ephemeral identity rather than
            // refusing it outright. It just won't appear in the offline view.
            warn!(node_name, error = %e, "node enrollment write failed; using ephemeral id");
            NodeAdmission::Admitted {
                node_id: uuid::Uuid::new_v4().to_string(),
                minted: true,
            }
        }
    }
}

/// Why a deregister could not name a single node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeregisterError {
    /// Neither an online session nor an enrolled device record matches.
    NotFound,
    /// The query names more than one node; carries a renderable explanation.
    Ambiguous(String),
}

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
fn resolve_enrolled_node(store: &SecurityStore, q: &str) -> Option<String> {
    let devices = store.list_devices().ok()?;
    if let Some(d) = devices
        .iter()
        .find(|d| d.role == "node" && d.device_id == q)
    {
        return Some(d.device_id.clone());
    }
    match match_by_name(store, q) {
        NameMatch::Unique(id) => Some(id),
        NameMatch::None | NameMatch::Ambiguous(_) => None,
    }
}

/// Deregister a node — the single source of truth behind both the
/// `cluster.deregister` RPC and the `node_manage` tool, so the Panel and the
/// model cannot end up with different takedown semantics (R4: the RPC handler
/// stays pure I/O).
///
/// Two-phase takedown:
/// ① [`NodeRegistry::forget`] evicts the live session — it vanishes from
///    `environments.list`, stops being addressable by `node_invoke`/`node_file`,
///    and its connection is asked to close (so the revoked node cannot keep
///    running dispatched work or raising approval cards);
/// ② `revoke_device` sets `revoked_at`, which is what makes deregistration
///    **sticky**: the node's next `connect` meets
///    [`NodeAdmission::Deregistered`] and it exits instead of self-reviving on
///    the following backoff.
///
/// Addressing tries the online registry's multi-level match first, then falls
/// back to enrolled-but-offline device records — a node visible in the fleet
/// view must be removable whether or not it is currently connected (offline
/// removals report `evicted: false`).
pub fn deregister_node(
    registry: &crate::cluster::NodeRegistry,
    store: &SecurityStore,
    query: &str,
) -> Result<DeregisterOutcome, DeregisterError> {
    use crate::cluster::ResolveError;
    let node_id = match registry.resolve_id(query) {
        Ok(id) => id,
        Err(ResolveError::NotFound) => {
            resolve_enrolled_node(store, query).ok_or(DeregisterError::NotFound)?
        }
        Err(e @ (ResolveError::Ambiguous(_) | ResolveError::NodeNotFound { .. })) => {
            return Err(DeregisterError::Ambiguous(e.to_string()))
        }
    };

    let evicted = registry.forget(&node_id);
    let device_removed = store.revoke_device(&node_id).unwrap_or_else(|e| {
        warn!(node_id = %node_id, error = %e, "failed to revoke node device on deregister");
        false
    });
    Ok(DeregisterOutcome {
        node_id,
        evicted,
        device_removed,
    })
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
        assert!(err.contains("already share the name"), "{err}");
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
        let out = deregister_node(&reg, &s, "Cold Box").expect("offline deregister");
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
            deregister_node(&reg, &s, "cold-box"),
            Err(DeregisterError::NotFound),
            "a revoked node is no longer addressable"
        );
    }

    #[test]
    fn ambiguous_name_mints_fresh_rather_than_guessing() {
        let s = store();
        mint_node_device(&s, "Worker 1").unwrap();
        mint_node_device(&s, "worker-1").unwrap();
        let NodeAdmission::Admitted { node_id, minted } = admit_node(&s, None, "worker-1") else {
            panic!("admitted");
        };
        assert!(minted);
        assert_eq!(
            s.list_devices().unwrap().len(),
            3,
            "minted a third, distinct row"
        );
        assert!(!node_id.is_empty());
    }
}
