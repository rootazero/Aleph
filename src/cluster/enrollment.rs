//! 节点准入（中心侧）：把一次 `connect` 解析成「用哪个 `node_id` 登记，还是拒绝」。
//!
//! LAN-trust 下节点不持 token，但中心仍为每个节点在 `security_store` 留一条
//! `role=node` 设备记录——它是**离线舰队视图**（`environments.list` 的 offline 半边）
//! 与 `cluster.deregister` 的记账基础。本模块是这条记录的**唯一写入/解析真源**，
//! 被两个入口共用：
//!
//! * `connect` 接缝（`gateway/server/handler.rs`）——节点自助登记（首启无 id）。
//! * `cluster.enroll` RPC（`gateway/handlers/cluster.rs`）——operator 在 Panel 预登记。
//!
//! 二者共用同一 [`mint_node_device`]，故预登记的行与节点首次拨入**按名归并**到同一
//! `node_id`，不再各铸一个 UUID 留下永久幽灵离线条目。
//!
//! 红线：确定性查表 + 写库，无 LLM 推理（R7）；不进 `src/harness/`（R10）。

use tracing::warn;

use crate::cluster::normalize_node_key;
use crate::gateway::security::store::{DeviceUpsertData, SecurityStore};

/// 一次节点 `connect` 的准入判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeAdmission {
    /// 放行：用这个 `node_id` 登记会话。`minted` = 本次新建了设备记录
    /// （节点应把 `node_id` 落盘）。
    Admitted { node_id: String, minted: bool },
    /// 拒绝：该节点已被 `cluster.deregister` 注销（设备记录 `revoked_at` 非空）。
    /// 注销必须**粘住**——否则被踢掉的节点在下一轮退避重连里就自己复活了。
    Deregistered { node_id: String },
}

/// 铸一条 `role=node` 设备记录并返回其 `node_id`（UUID）。
///
/// `public_key` 是占位（LAN-trust 无硬件密钥，同 `connect.rs`）；`fingerprint`
/// 取 UUID 前 16 位以满足表上的 UNIQUE 约束。
pub fn mint_node_device(store: &SecurityStore, node_name: &str) -> Result<String, String> {
    let device_id = uuid::Uuid::new_v4().to_string();
    let fingerprint: String = device_id.chars().take(16).collect();
    store
        .upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name: node_name,
            device_type: None,
            public_key: &[0u8; 32],
            fingerprint: &fingerprint,
            role: "node",
            scopes: &["node".to_string()],
        })
        .map_err(|e| format!("failed to register node device: {e}"))?;
    Ok(device_id)
}

/// 在**活跃**（未吊销）的 `role=node` 设备里按归一化名唯一匹配。
/// 命中唯一 → 复用其 id；零命中或歧义 → `None`（调用方铸新的）。
///
/// 名字归一化与在线寻址 [`normalize_node_key`] 同源，故 Panel 里预登记的
/// "GPU Box" 与节点 `--name gpu-box` 拨入会归并到同一条记录。
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

/// 解析一次节点 `connect` 的准入。`presented_id` = connect 帧的 `device_id`
/// （首启为 `None`）。
///
/// 判定顺序：
/// 1. 带 id 且该记录**已吊销** → [`NodeAdmission::Deregistered`]（注销粘住）。
/// 2. 带 id 且记录活跃 → 原样复用（稳定身份）。
/// 3. 带 id 但**查无此记录**（中心库重置 / 换了中心）→ 采纳该 id 并补写记录，
///    让节点保住已落盘的身份，不必重新登记。
/// 4. 无 id（首启）→ 先按名复用 operator 预登记的行，否则铸新的；`minted=true`
///    提示节点把 id 落盘。
///
/// store 读写失败一律降级为「采纳/铸造但不落库」，节点仍能工作（P7 优雅降级）——
/// 代价只是离线视图少一条记录。
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
                let fingerprint: String = id.chars().take(16).collect();
                if let Err(e) = store.upsert_device(&DeviceUpsertData {
                    device_id: id,
                    device_name: node_name,
                    device_type: None,
                    public_key: &[0u8; 32],
                    fingerprint: &fingerprint,
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
