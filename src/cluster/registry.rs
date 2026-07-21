//! 集群节点登记表（中心侧）。
//!
//! 追踪「哪些已连 WS 连接是已登记节点」，并把它们投影成只读「环境」视图供
//! `environments.list` 渲染。消费 Phase 0a 的 [`ReverseRpcChannel`]——每个
//! `NodeSession` 持一份 channel clone，0c 的 `node_invoke` 经它向节点下发。
//!
//! 红线：纯数据结构，无 LLM 推理（R7），不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync_primitives::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cluster::ReverseRpcChannel;

/// 节点声明的一个 command（名字 + 自描述 schema）。0b 不解析 schema，原样透传。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandDescriptor {
    pub name: String,
    pub schema: Value,
}

/// 一个已连入的节点会话（中心侧视图）。
pub struct NodeSession {
    /// = `device_id，直接当环境` id。
    pub node_id: String,
    /// 对应 0a `reverse_rpc` 表的键，断线清理对账用。
    pub conn_id: String,
    /// 人类可读名（来自 connect 帧）。
    pub device_name: String,
    /// 0a 通道的 clone —— 0c 的 `node_invoke` 经它下发。
    pub channel: ReverseRpcChannel,
    /// 节点自声明的 command 目录，0b 只存只显。
    pub declared_commands: Vec<CommandDescriptor>,
    /// Operator-assigned free-text labels (e.g. "gpu", "region=us"). Selection
    /// only — never an authorization gate (R7). Stored verbatim; not kv-parsed.
    pub tags: Vec<String>,
    /// 登记时刻（Unix 秒）。
    pub connected_at: i64,
}

/// 节点寻址失败的结构化结果（取代旧的 `Option`，让歧义对调用方显式可见）。
/// 映射 openclaw `node-match.ts` 的多级匹配，但用类型安全枚举表达——
/// 让"歧义"成为不可忽略的一等状态，而非 stringly error。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// 没有任何在线节点匹配该 name/id。
    NotFound,
    /// 多个在线节点匹配——附带可读候选标签（`name (short-id)`），供 LLM 收窄。
    Ambiguous(Vec<String>),
    /// 内部状态不一致：match_id 返回的 id 在 nodes_by_id 中缺失。
    NodeNotFound { name_or_id: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no online node matches"),
            Self::Ambiguous(c) => write!(f, "ambiguous — matches: {}", c.join(", ")),
            Self::NodeNotFound { name_or_id } => {
                write!(f, "internal node lookup failed for '{name_or_id}'")
            }
        }
    }
}

/// `environments.list` 的对外序列化视图（薄渲染契约，R4）。绝不含凭证。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub status: &'static str,
    pub commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
    pub connected_at: i64,
    /// 最近一次在线时刻（Unix 秒）。仅对 `status == "offline"` 的已登记节点有
    /// 意义（在线节点恒 `None`）；`None` + offline = 登记后从未连入。
    #[serde(default)]
    pub last_seen_at: Option<i64>,
}

/// A matched online node for tag-selected fan-out: enough to dispatch over
/// reverse RPC and run the same per-node fail-fast check `node_invoke` uses.
/// `tags` is carried so the caller can build a "available tags" hint on a
/// zero-match. Cloneable; holds a `ReverseRpcChannel` clone.
#[derive(Clone)]
pub struct NodeMatch {
    pub node_id: String,
    pub name: String,
    pub channel: ReverseRpcChannel,
    pub declared_commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
}

#[derive(Default)]
struct RegistryInner {
    /// `node_id` → session（权威）。
    nodes_by_id: HashMap<String, NodeSession>,
    /// `conn_id` → `node_id（断线反查`）。
    nodes_by_conn: HashMap<String, String>,
}

/// 节点注册表。线程安全；锁中毒按 P7（`unwrap_or_else(|e| e.into_inner())`）。
#[derive(Default)]
pub struct NodeRegistry {
    inner: RwLock<RegistryInner>,
}

impl NodeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个节点会话。同 `node_id` 重连 → 覆盖旧会话，并清掉旧 conn 映射。
    pub fn register(&self, session: NodeSession) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let node_id = session.node_id.clone();
        let conn_id = session.conn_id.clone();
        // Drop any stale conn→node mapping the previous session for this node_id held,
        // so an old connection's later cleanup can't evict the new session.
        if let Some(prev) = inner.nodes_by_id.get(&node_id) {
            let prev_conn = prev.conn_id.clone();
            inner.nodes_by_conn.remove(&prev_conn);
        }
        inner.nodes_by_conn.insert(conn_id, node_id.clone());
        inner.nodes_by_id.insert(node_id, session);
    }

    /// 注销一个连接的节点会话。仅当该 `node_id` 当前会话确属此 `conn_id` 时才移除
    /// （重连安全：旧连接 cleanup 不会误删新会话）。返回是否移除了会话。
    pub fn deregister(&self, conn_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let Some(node_id) = inner.nodes_by_conn.remove(conn_id) else {
            return false;
        };
        match inner.nodes_by_id.get(&node_id) {
            Some(s) if s.conn_id == conn_id => {
                inner.nodes_by_id.remove(&node_id);
                true
            }
            _ => false,
        }
    }

    /// 在线节点的只读投影快照。结果按 `(name, id)` 稳定排序——`nodes_by_id` 是
    /// `HashMap`，迭代序不定；排序后 Panel 舰队列表与模型可见的 `node_list` 不会
    /// 每次刷新都抖动（测试也得以断言确定序）。
    pub fn list_environments(&self) -> Vec<Environment> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut envs: Vec<Environment> = inner
            .nodes_by_id
            .values()
            .map(|s| Environment {
                id: s.node_id.clone(),
                name: s.device_name.clone(),
                status: "online",
                commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
                connected_at: s.connected_at,
                last_seen_at: None,
            })
            .collect();
        envs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        envs
    }

    /// Resolve `(node_id, device_name)` for a connection that is a registered
    /// node. Returns `None` for non-node / unregistered connections. The center
    /// uses this to stamp node identity from the AUTHENTICATED connection rather
    /// than trusting request params (anti-spoof).
    pub fn node_identity_by_conn(&self, conn_id: &str) -> Option<(String, String)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let node_id = inner.nodes_by_conn.get(conn_id)?;
        let s = inner.nodes_by_id.get(node_id)?;
        Some((s.node_id.clone(), s.device_name.clone()))
    }

    /// 把 name/id 解析为唯一的 `node_id（多级匹配，registry` 只存在线会话故无需
    /// "prefer-connected" tie-break——所有候选都在线）。匹配级别（强→弱）：
    /// ① 精确 `node_id`（原样，id 是 UUID） ② 归一化 `device_name` 等值
    /// ③ 模糊（id 前缀 ≥4 OR 归一化 name 子串）。名字匹配经 [`normalize_node_key`]
    /// 大小写 + 标点/空格不敏感（映射 openclaw `node-match.ts::normalizeNodeKey`），
    /// 故 "GPU Box" 可用 "gpu-box" 寻址。每级若多命中即 `Ambiguous`，绝不静默挑第一个。
    fn match_id(inner: &RegistryInner, q: &str) -> std::result::Result<String, ResolveError> {
        // ① 精确 id（UUID，大小写敏感、不归一化——避免折叠掉 id 内的连字符语义）。
        if inner.nodes_by_id.contains_key(q) {
            return Ok(q.to_string());
        }
        let nq = normalize_node_key(q);
        // ② 归一化精确 name（device_name 不保证唯一 → 可能歧义）。空键（全标点查询）
        //    跳过名字匹配，否则会与同样归一化为空的脏名字误配。
        if !nq.is_empty() {
            let exact: Vec<&NodeSession> = inner
                .nodes_by_id
                .values()
                .filter(|s| normalize_node_key(&s.device_name) == nq)
                .collect();
            match exact.as_slice() {
                [s] => return Ok(s.node_id.clone()),
                [] => {}
                many => return Err(ResolveError::Ambiguous(candidate_labels(many))),
            }
        }
        // ③ 模糊：id 前缀（≥4 字符原样小写，避免 1 字符炸开）或归一化 name 子串。
        let ql = q.to_ascii_lowercase();
        let fuzzy: Vec<&NodeSession> = inner
            .nodes_by_id
            .values()
            .filter(|s| {
                (q.len() >= 4 && s.node_id.to_ascii_lowercase().starts_with(&ql))
                    || (!nq.is_empty() && normalize_node_key(&s.device_name).contains(&nq))
            })
            .collect();
        match fuzzy.as_slice() {
            [s] => Ok(s.node_id.clone()),
            [] => Err(ResolveError::NotFound),
            many => Err(ResolveError::Ambiguous(candidate_labels(many))),
        }
    }

    /// 按 name 或 id 解析一个在线节点，返回其反向 RPC 通道 + 声明的命令目录。
    /// `node_invoke` / `node_file` 用它寻址 + fail-fast 校验。歧义/未命中以
    /// 结构化 [`ResolveError`] 返回，让调用方给 LLM 精确提示。
    pub fn resolve(
        &self,
        name_or_id: &str,
    ) -> std::result::Result<(ReverseRpcChannel, Vec<CommandDescriptor>), ResolveError> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let id = Self::match_id(&inner, name_or_id)?;
        let s = inner
            .nodes_by_id
            .get(&id)
            .ok_or_else(|| ResolveError::NodeNotFound {
                name_or_id: name_or_id.to_string(),
            })?;
        Ok((s.channel.clone(), s.declared_commands.clone()))
    }

    /// 同 [`resolve`] 的多级匹配，但只回 `node_id` —— `cluster.deregister` 用它把
    /// operator 给的 name/id 落到唯一节点身份，再驱逐 + 撤 token。
    pub fn resolve_id(&self, name_or_id: &str) -> std::result::Result<String, ResolveError> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        Self::match_id(&inner, name_or_id)
    }

    /// All online nodes carrying EVERY tag in `tags` (AND match). An empty
    /// `tags` slice matches every online node (the "broadcast" case). Used by
    /// `node_invoke_many` for tag-selected concurrent fan-out. Returns a clone
    /// snapshot so the caller dispatches without holding the registry lock.
    pub fn resolve_all_by_tags(&self, tags: &[String]) -> Vec<NodeMatch> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .nodes_by_id
            .values()
            .filter(|s| tags.iter().all(|t| s.tags.contains(t)))
            .map(|s| NodeMatch {
                node_id: s.node_id.clone(),
                name: s.device_name.clone(),
                channel: s.channel.clone(),
                declared_commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
            })
            .collect()
    }

    /// 按 `node_id` 主动驱逐一个会话（operator deregister 用）。从两张表都抹除，
    /// 持有的 [`ReverseRpcChannel`] clone 随之 drop。返回是否确有会话被移除。
    /// 与 [`deregister`](Self::deregister)（按 `conn_id` 的断线对账）正交。
    pub fn forget(&self, node_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        match inner.nodes_by_id.remove(node_id) {
            Some(s) => {
                inner.nodes_by_conn.remove(&s.conn_id);
                true
            }
            None => false,
        }
    }
}

/// 把候选会话渲染成可读标签 `name (short-id)`，给歧义错误用。short-id 取前 8 位
/// 足以辨识又不喧宾夺主。结果排序保证错误信息稳定（便于测试与日志比对）。
fn candidate_labels(sessions: &[&NodeSession]) -> Vec<String> {
    let mut labels: Vec<String> = sessions
        .iter()
        .map(|s| {
            let short: String = s.node_id.chars().take(8).collect();
            format!("{} ({})", s.device_name, short)
        })
        .collect();
    labels.sort();
    labels
}

/// 把人类可读的节点名归一化成稳定查找键：转小写 + 把每段非字母数字折叠为单个
/// `-` + 去掉首尾 `-`。**字母数字判定用 Unicode 感知的 [`char::is_alphanumeric`]**
/// （非 ASCII-only），故 CJK / 带重音的拉丁字母被**保留**而非丢弃——`"工作站"`
/// 归一化后仍非空、仍可按名寻址（"GPU Box" / "gpu_box" 仍折叠为 `gpu-box`）。
/// 旧的 ASCII-only 实现会把纯非 ASCII 名整段折成空键 ⇒ 中文/日文节点名根本无法
/// 按名寻址、且每次重连 [`crate::cluster::admit_node`] 都重铸一个新 id（幽灵行堆积）。
///
/// 映射 openclaw `node-match.ts::normalizeNodeKey` 演进后的 Unicode 版
/// （NFC + `[^\p{L}\p{M}\p{N}]+ → -`）的**常见分支**。**有意的偏差（R3 核心轻量化——
/// 不为单一 helper 引入 `unicode-normalization` crate）**：组合记号（`\p{M}`，如天城文
/// 元音符号 / 分解式重音）被当作分隔符、且不做 NFC。这只影响键的**外观**、不影响可
/// 寻址性——归一化对 query 与库存名**对称**施加，两侧折叠一致即可匹配。
///
/// 在线 [`NodeRegistry::match_id`] 与离线 `cluster.deregister` 回退寻址共用此单一
/// 真源，杜绝两路语义漂移。空键（全标点/全记号查询）由各调用点的 `is_empty` 守卫跳过。
pub(crate) fn normalize_node_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            // `char::to_lowercase` 可能产出多个 char（如 İ → i̇），用 extend 而非 push。
            out.extend(ch.to_lowercase());
        } else {
            // Defer the separator so leading/trailing/repeated runs collapse and
            // never produce a boundary dash.
            pending_dash = true;
        }
    }
    out
}

/// connect→register 接缝：仅当 `role == Some("node")` 时把这条连接登记进
/// `NodeRegistry`。`params` 是 connect 帧的 params（取 `device_name` + commands）。
/// 返回是否登记。抽成纯函数以便单测，且让 `handler.rs` 保持薄。
pub fn maybe_register_node(
    registry: &NodeRegistry,
    role: Option<&str>,
    device_id: &str,
    conn_id: &str,
    params: Option<&Value>,
    channel: &ReverseRpcChannel,
) -> bool {
    if role != Some("node") {
        return false;
    }
    let device_name = params
        .and_then(|p| p.get("device_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let declared_commands = params
        .and_then(|p| p.get("commands"))
        .and_then(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()).ok())
        .unwrap_or_default();
    let tags = params
        .and_then(|p| p.get("tags"))
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default();
    registry.register(NodeSession {
        node_id: device_id.to_string(),
        conn_id: conn_id.to_string(),
        device_name,
        channel: channel.clone(),
        declared_commands,
        tags,
        connected_at: now_unix(),
    });
    true
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn test_channel() -> ReverseRpcChannel {
        let (tx, _rx) = mpsc::channel::<String>(8);
        ReverseRpcChannel::new(tx)
    }

    fn session(node_id: &str, conn_id: &str) -> NodeSession {
        NodeSession {
            node_id: node_id.to_string(),
            conn_id: conn_id.to_string(),
            device_name: format!("dev-{node_id}"),
            channel: test_channel(),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({"type": "object"}),
            }],
            tags: vec![],
            connected_at: 1,
        }
    }

    #[test]
    fn register_then_list_projects_environment() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-a");
        assert_eq!(envs[0].name, "dev-node-a");
        assert_eq!(envs[0].status, "online");
        assert_eq!(envs[0].commands.len(), 1);
        assert_eq!(envs[0].commands[0].name, "bash");
        assert!(envs[0].tags.is_empty());
    }

    #[test]
    fn deregister_removes_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.deregister("conn-1"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.resolve("node-a").is_err());
        assert!(!reg.deregister("conn-x"));
    }

    #[test]
    fn reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        reg.register(session("node-a", "conn-2"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(!reg.deregister("conn-1"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(reg.deregister("conn-2"));
        assert!(reg.list_environments().is_empty());
    }

    #[test]
    fn list_environments_is_sorted_by_name_then_id() {
        let reg = NodeRegistry::new();
        // Register out of name order; the projection must come back sorted so the
        // HashMap iteration order can't leak into the Panel / node_list view.
        reg.register(session("z-id", "c-z")); // device_name = "dev-z-id"
        reg.register(session("a-id", "c-a")); // device_name = "dev-a-id"
        reg.register(session("m-id", "c-m")); // device_name = "dev-m-id"
        let names: Vec<String> = reg
            .list_environments()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["dev-a-id", "dev-m-id", "dev-z-id"]);
    }

    #[test]
    fn resolve_by_id_then_by_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert!(reg.resolve("node-a").is_ok(), "by id");
        let (_, cmds) = reg.resolve("dev-node-a").expect("by name");
        assert_eq!(cmds[0].name, "bash");
        assert!(matches!(reg.resolve("nope"), Err(ResolveError::NotFound)));
    }

    #[test]
    fn resolve_by_unique_id_prefix_and_name_substring() {
        let reg = NodeRegistry::new();
        reg.register(session("abcd1234", "conn-1")); // device_name = "dev-abcd1234"
                                                     // id prefix (≥4) uniquely matches.
        assert_eq!(reg.resolve_id("abcd").unwrap(), "abcd1234");
        // name substring (case-insensitive) uniquely matches.
        assert_eq!(reg.resolve_id("ABCD1234").unwrap(), "abcd1234");
        assert_eq!(reg.resolve_id("dev-abcd").unwrap(), "abcd1234");
        // too-short prefix that isn't a name substring → not found.
        assert_eq!(reg.resolve_id("xyz").unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn normalize_node_key_folds_case_and_punctuation() {
        assert_eq!(normalize_node_key("GPU Box"), "gpu-box");
        assert_eq!(normalize_node_key("gpu_box"), "gpu-box");
        assert_eq!(normalize_node_key("gpu-box"), "gpu-box");
        assert_eq!(normalize_node_key("  GPU   Box!! "), "gpu-box");
        assert_eq!(normalize_node_key("--Worker__1--"), "worker-1");
        assert_eq!(normalize_node_key("Worker1"), "worker1");
        // All-punctuation / empty → empty key (callers skip name matching on it).
        assert_eq!(normalize_node_key("  -_- "), "");
        assert_eq!(normalize_node_key(""), "");
    }

    #[test]
    fn normalize_node_key_is_unicode_aware() {
        // Non-ASCII letters must SURVIVE rather than collapse to an empty key —
        // a pure-CJK node name was previously unaddressable and re-minted a fresh
        // id on every reconnect. Maps openclaw's Unicode-aware normalizeNodeKey.
        assert_eq!(normalize_node_key("工作站"), "工作站");
        assert_eq!(normalize_node_key("工作站 01"), "工作站-01");
        assert_eq!(normalize_node_key("GPU 工作站"), "gpu-工作站");
        // Precomposed accented Latin lowercases and is preserved (café, not caf).
        assert_eq!(normalize_node_key("Café"), "café");
        // Combining-mark scripts (Devanagari vowel signs are \p{M}, dropped in the
        // zero-dep impl) still fold to a stable NON-EMPTY key, so the node stays
        // addressable by name — the key is an internal match key, need not be
        // visually identical, and the same fold applies to query and stored name.
        assert!(!normalize_node_key("किताब").is_empty());
        // All-punctuation, including non-ASCII punctuation, still folds to empty.
        assert_eq!(normalize_node_key("。、！"), "");
    }

    #[test]
    fn resolve_cjk_name_is_addressable() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "id-cn".into(),
            conn_id: "c-cn".into(),
            device_name: "工作站".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        // Exact CJK name resolves (was NotFound before: "工作站" → "" → the
        // empty-key guard skipped name matching entirely).
        assert_eq!(reg.resolve_id("工作站").unwrap(), "id-cn");
        // Fuzzy substring on the normalized CJK form also resolves.
        assert_eq!(reg.resolve_id("工作").unwrap(), "id-cn");
    }

    #[test]
    fn resolve_name_is_case_and_punctuation_insensitive() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "id-1".into(),
            conn_id: "c1".into(),
            device_name: "GPU Box".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        // The operator/LLM can spell the spaced name with a dash, underscore, or
        // any case — all fold to the same key (maps openclaw normalizeNodeKey).
        assert_eq!(reg.resolve_id("gpu-box").unwrap(), "id-1");
        assert_eq!(reg.resolve_id("GPU_BOX").unwrap(), "id-1");
        assert_eq!(reg.resolve_id("gpu box").unwrap(), "id-1");
        // Substring fuzzy still works on the normalized form.
        assert_eq!(reg.resolve_id("box").unwrap(), "id-1");
        // An all-punctuation query matches nothing (empty normalized key).
        assert_eq!(reg.resolve_id("---").unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn normalized_names_that_collide_are_ambiguous() {
        let reg = NodeRegistry::new();
        // "Worker 1" and "worker-1" both normalize to "worker-1" — addressing by
        // name must report ambiguity rather than silently pick one.
        reg.register(NodeSession {
            node_id: "id-a".into(),
            conn_id: "ca".into(),
            device_name: "Worker 1".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "id-b".into(),
            conn_id: "cb".into(),
            device_name: "worker-1".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        assert!(matches!(
            reg.resolve_id("WORKER_1"),
            Err(ResolveError::Ambiguous(_))
        ));
    }

    #[test]
    fn resolve_reports_ambiguity_with_sorted_candidates() {
        let reg = NodeRegistry::new();
        // Two nodes whose names share the substring "work".
        reg.register(NodeSession {
            node_id: "id-two".to_string(),
            conn_id: "c2".to_string(),
            device_name: "worker-2".to_string(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "id-one".to_string(),
            conn_id: "c1".to_string(),
            device_name: "worker-1".to_string(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            connected_at: 1,
        });
        match reg.resolve_id("worker").unwrap_err() {
            ResolveError::Ambiguous(c) => {
                // sorted + labelled "name (short-id)".
                assert_eq!(c, vec!["worker-1 (id-one)", "worker-2 (id-two)"]);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn forget_evicts_by_node_id_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.forget("node-a"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.resolve("node-a").is_err());
        // A stale conn cleanup after forget is a harmless no-op.
        assert!(!reg.deregister("conn-1"));
        // Forgetting an unknown id reports nothing removed.
        assert!(!reg.forget("ghost"));
    }

    #[test]
    fn node_identity_by_conn_returns_id_and_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert_eq!(
            reg.node_identity_by_conn("conn-1"),
            Some(("node-a".to_string(), "dev-node-a".to_string()))
        );
        assert_eq!(reg.node_identity_by_conn("conn-x"), None);
    }

    #[test]
    fn maybe_register_node_registers_only_for_node_role() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({"device_name": "worker", "commands": [{"name": "bash", "schema": {}}]});
        assert!(!maybe_register_node(
            &reg,
            Some("operator"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));
        assert!(reg.list_environments().is_empty());
        assert!(!maybe_register_node(
            &reg,
            None,
            "d0",
            "c0",
            Some(&params),
            &ch
        ));
        assert!(reg.list_environments().is_empty());
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&params),
            &ch
        ));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "d2");
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn resolve_all_by_tags_and_semantics() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "a".into(),
            conn_id: "ca".into(),
            device_name: "node-a".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into(), "us".into()],
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "b".into(),
            conn_id: "cb".into(),
            device_name: "node-b".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into()],
            connected_at: 1,
        });
        // AND: both tags required → only "a".
        let both = reg.resolve_all_by_tags(&["gpu".into(), "us".into()]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].node_id, "a");
        assert_eq!(both[0].name, "node-a");
        // Single tag both carry → both.
        assert_eq!(reg.resolve_all_by_tags(&["gpu".into()]).len(), 2);
        // Empty tags → every online node.
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
        // Unmatched tag → none.
        assert!(reg.resolve_all_by_tags(&["fpga".into()]).is_empty());
        // NodeMatch carries the node's tags (used for the zero-match hint).
        let gpu = reg.resolve_all_by_tags(&["gpu".into()]);
        assert!(gpu.iter().any(|m| m.tags.contains(&"us".to_string())));
    }

    #[test]
    fn maybe_register_node_parses_tags_from_params() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({
            "device_name": "worker",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["gpu", "region=us"]
        });
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));
        let m = reg.resolve_all_by_tags(&["region=us".into()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].node_id, "d1");
        // Missing "tags" key → empty, not an error.
        let ch2 = test_channel();
        let no_tags = json!({"device_name": "w2", "commands": []});
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&no_tags),
            &ch2
        ));
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
    }
}
