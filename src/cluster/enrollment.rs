//! Node admission (center side): resolves a `connect` into "which `node_id` to
//! register, or reject".
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
}

/// Mint a `role=node` device record and return its `node_id` (UUID).
///
/// `public_key` is a placeholder (LAN-trust has no hardware key, same as
/// `connect.rs`); `fingerprint` uses the first 16 chars of the UUID to satisfy
/// the UNIQUE constraint on the table.
pub fn mint_node_device(store: &SecurityStore, node_name: &str) -> Result<String, String> {
    let device_id = uuid::Uuid::new_v4().to_string();
    store
        .upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name: node_name,
            device_type: None,
            public_key: &[0u8; 32],
            fingerprint: &device_id[..16],
            role: "node",
            scopes: &["node".to_string()],
        })
        .map_err(|e| format!("failed to register node device: {e}"))?;
    Ok(device_id)
}

/// Uniquely match an **active** (non-revoked) `role=node` device by normalized
/// name. Single hit → reuse its id; zero hits or ambiguous → `None` (caller
/// mints a new one).
///
/// Name normalization shares the same source of truth as online addressing
/// [`normalize_node_key`], so a pre-enrolled "GPU Box" in the Panel and a node
/// dialing in as `--name gpu-box` merge into the same record.
fn reuse_by_name(store: &SecurityStore, node_name: &str) -> Option<String> {
    let key = normalize_node_key(node_name);
    if key.is_empty() {
        return None;
    }
    let devices = store.list_devices().ok()?;
    let mut hits = devices
        .into_iter()
        .filter(|d| d.role == "node" && normalize_node_key(&d.device_name) == key);
    let first = hits.next()?;
    if hits.next().is_some() {
        // Two active rows normalize to the same name — refuse to guess which one
        // this node is. Mint a fresh id instead of silently adopting one of them.
        warn!(
            node_name,
            "multiple enrolled nodes share this name; minting a fresh node id"
        );
        return None;
    }
    Some(first.device_id)
}

/// Resolve admission for a node `connect`. `presented_id` = the `device_id` from
/// the connect frame (`None` on first boot).
///
/// Decision order:
/// 1. Has id AND that record is **revoked** → [`NodeAdmission::Deregistered`]
///    (deregistration sticks).
/// 2. Has id AND record is active → reuse as-is (stable identity).
/// 3. Has id but **no such record** (center DB reset / switched centers) →
///    adopt the id and backfill the row so the node keeps its persisted identity
///    without re-registering.
/// 4. No id (first boot) → try to adopt an operator's pre-enrolled row by name
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
                let fingerprint = &id[..id.len().min(16)];
                if let Err(e) = store.upsert_device(&DeviceUpsertData {
                    device_id: id,
                    device_name: node_name,
                    device_type: None,
                    public_key: &[0u8; 32],
                    fingerprint,
                    role: "node",
                    scopes: &["node".to_string()],
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
    if let Some(existing) = reuse_by_name(store, node_name) {
        return NodeAdmission::Admitted {
            node_id: existing,
            minted: true,
        };
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
    fn ambiguous_name_mints_fresh_rather_than_guessing() {
        let s = store();
        mint_node_device(&s, "Worker 1").unwrap();
        mint_node_device(&s, "worker-1").unwrap();
        let NodeAdmission::Admitted { node_id, minted } = admit_node(&s, None, "worker-1") else {
            panic!("admitted");
        };
        assert!(minted);
        assert_eq!(s.list_devices().unwrap().len(), 3, "minted a third, distinct row");
        assert!(!node_id.is_empty());
    }
}
