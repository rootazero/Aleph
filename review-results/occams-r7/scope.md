# scope review — 2026-10-01

## 概览
- 模块规模：4 文件，约 950 行（含测试）
- 角色（graph.json）：scope 模块是 Aleph 的**权限/作用域抽象核心**，`authority` 管理 fire-time 授权裁决，`carried` 封装跨 `tokio::spawn` 的任务局部传递，`directory` 是用户显示名的进程级只读缓存，`mod.rs` 定义 `ScopeId`/`ScopeAttribution`/`FlowScope` 词汇表和全部 ambient 任务局部。
- **估计节省**：~3 行（无结构性大块删除）

## P0 — 安全/阻塞（无）

## P1 — 逻辑/正确（无）

## P2 — 复杂度/错误处理

- **`src/scope/directory.rs:52` — `display_name` 中冗余的 `RwLock` guard 转换**
  - 现象：`guard.read().unwrap_or_else(|e| e.into_inner()).get(user_id).cloned()`
  - 证据：`RwLockReadGuard<T>` 通过 `Deref` 指向 `T = HashMap<String, String>`，`guard.get()` 已返回 `Option<&String>`，`.unwrap_or_else(|e| e.into_inner())` 在 guard 层面没有任何效果（`RwLock` 的 poisoning 不是这个 guard 能处理的——它是 `PoisonError<RwLockReadGuard<T>>`，内部并无 `HashMap`）。可直接 `.read().unwrap().get(user_id).cloned()`。
  - 建议：删除 `unwrap_or_else` 层，改为 `guard.read().unwrap().get(user_id).cloned()`。性质上无害（逻辑等效），但是不必要的认知噪音——后续读者会误认为此处处理了某种错误。

- **`src/scope/directory.rs:44` — 同上，`hydrate`/`record` 中 `write().unwrap_or_else`**
  - 现象：`guard.write().unwrap_or_else(|e| e.into_inner()).insert(...)`
  - 证据：同上，`RwLockWriteGuard<T>` 的 poisoning error 构造没有有意义的内部值（`e.into_inner()` 返回被污染的 guard，但 `guard` 变量本身生命周期已结束）；`PoisonError::into_inner()` 官方文档建议仅用于诊断，此处无诊断价值。
  - 建议：改为 `guard.write().unwrap().insert(...)`。

- **`src/scope/authority.rs:113` — `stamp()` 中对 `&'static str` 做不必要的 `to_string()`**
  - 现象：`metadata.insert(AUTHOR_USER_KEY.to_string(), author.clone())`，其中 `AUTHOR_USER_KEY` 是 `crate::gateway::execution_engine::AUTHOR_USER_KEY`（`&'static str`）。此 key 的 value `author` 本身是 `Option<String>` 来自 `.map(str::to_string)`，其 `.clone()` 不可避免；但 key `AUTHOR_USER_KEY.to_string()` 每调用一次分配一次。
  - 证据：grep 全文，`AUTHOR_USER_KEY` 恒为 `&'static str`（`gateway/execution_engine/mod.rs` 定义）。`stamp()` 在每次 fire 授权通过时调用，非超热路径但不是冷路径。
  - 建议：若 `metadata: &mut HashMap<String, String>` 可改为 `&mut HashMap<&str, String>`，则 `AUTHOR_USER_KEY` 和 `"caller_role"` 两个 literal key 均无需 `.to_string()`。此改动需审查所有 `stamp_metadata` 的调用方是否统一接受 `HashMap<&str, String>`。若不愿意动类型，可保留当前做法（分配轻微，语义正确）。

## P3 — 性能/风格（仅总数）
- 3 条（见 P2）

## 误报自检

- **`ScopeId::Org` 变体 — 考虑作为 dead code 删除**：放弃。模块文档明确说明 Org 是"vocabulary only"，无生产方；`scope_from_metadata`/`ScopeAttribution::from_persisted` 可构造它（来自手写数据库行），但所有调用方均做 `Project`/`Personal` 过滤或直接丢弃。此行为是 spec 的有意识设计，非遗留死代码。

- **`Granted::carried_role: Option<String>` 考虑改为 `Option<&'static str>`**：放弃。改为引用需在 `Granted::legacy`/`Granted::carried`/`resolve_with_users` 所有调用点额外分配 `String`，而 `carried()`（spawn 路径，非热路径）反而多一次 clone。现有方案（`carried_role: String`）对 `carried()` 是零额外分配，对其他调用点是一次 clone，不可更优。

- **`pub(crate) fn from_parts` 加 `#[must_use]`**：`from_parts` 在 `Granted::carried` 中被立即 `.into()` 消费，不加 `#[must_use]` 结果被静默丢弃的可能性为零；`carried()` 自身已有 `#[must_use]`，属重复标注。P3 无需报告。

## 未审查清单
- `carried.rs::reestablish` 中的 `Box::pin` 动态分配（文档已说明 load-bearing，跳过）
- `authority.rs::resolve_with_users` 中 `UsersAuthority::Degraded` 分支的 `reason` 字段传播链路（需对照 `security/store/slot.rs` 的 degraded 状态机）
- `FlowScope` 与 `flow_scope_census` 的五层登记契约（模块文档已详述，行为测试覆盖）
- `teams/dispatcher/schedule/authority.rs` 中 `CarriedAttribution` 跨 schedule 的持有链（跨越多 crate 边界）