# routing review — 2026-10-01

## 概览

- **模块规模**：9 文件，约 4,100 行（含内联测试）
- **角色**（GRAPH_REPORT.md）：VESR 通道感知会话路由核心——按 channel/guild/team/peer/account 层级解析路由绑定，驱动 SessionKey 构建与 VESR 经验存储/召回。
- **估计节省**：低优先级；本模块结构清晰，无重大死代码块。

## P0 — 安全/阻塞（无）

## P1 — 逻辑/正确（高置信度）

- **`src/routing/recall.rs:114-117` — `set_task_emb` 失败静默吞掉，导致记忆条目键值错误**
  - 现象：`RoutingRecall::build_routing_experience_message` 内部 `attribution.set_task_emb(task_emb.clone())` 失败时执行空 match 体，不传播错误，不降级。
  - 证据：
    ```rust
    // recall.rs:114-117
    let task_emb = match attribution.set_task_emb(task_emb.clone()) {
        Ok(()) => task_emb,
        Err(()) => {
            tracing::debug!(...);
            // 静默：fallthrough 到后续逻辑，task_emb 被覆盖为旧值
        }
    };
    ```
  - 后果：`task_emb` 保持旧的 `attribution` 值（可能来自父 run），后续 `store.recall()` 用错误键查询，导致召回结果为空或跨 run 混淆。
  - 建议：`Err(())` 应返回 `Err(AlephError::...)` 或记录 `warn!` 并 `return Ok(None)`；`#[instrument]` 中静默 debug 不够。

- **`src/routing/observer.rs:160-163` — `record` future 的错误与 panic 被完全丢弃**
  - 现象：`tokio::spawn(async move { let _record_permit = permit; record.await; })` 中 `record.await` 的结果未检查。
  - 证据：`observer.rs:160-163`——`record: Pin<Box<dyn Future<Output=Result<..., AlephError>>>>`，其 `Output` 被完全忽略。
  - 后果：若 DB 写入失败或 future panic，没有任何观测信号；`permit` 持有者在整个 spawn 生命周期内都被阻塞。
  - 建议：至少 `.await.inspect_err(|e| tracing::error!(...))` 或传播到 `OutcomeObserver` 的错误 channel。

## P2 — 复杂度/错误处理

- **`src/routing/observer.rs:40-46` — `#[allow]` 粒度过粗，掩盖 5 个不必要的 clone**
  - 现象：整个 `on_trace` 函数体的 `#[allow]` 掩盖了 5 处 clone：store、agent_id、model_id、provider_id，以及 Semaphore 的 `try_acquire_owned`。
  - 证据：`observer.rs:40` `#[allow(...)]` → `observer.rs:150-157` 逐行 disable 注释。
  - 分析：Semaphore 的 `try_acquire_owned` 确实是必要的（跨 future 边界）；但 store/agent_id/model_id/provider_id clone 是因为 `record_to_store` 签名接收 owned 值。
  - 建议：缩小 `#[allow]` 到仅覆盖 `try_acquire_owned`（`self.record_slots.clone()`）这一行；其余 clone 应在调用链上游消除（如 `record_to_store` 改用引用）。

- **`src/routing/resolve.rs:66-67` — hot path 无条件堆分配**
  - 现象：`resolve_route` 每次调用都执行 `input.channel.trim().to_lowercase()`，每次都堆分配新 String。
  - 证据：`resolve.rs:67` `let channel = input.channel.trim().to_lowercase();`——`&str` 已知大小，堆分配无必要性。
  - 影响：所有路由决策（包含最高频的 default 路径）都触发此分配；channel 名通常为短 ASCII（`telegram`、`slack` 等）。
  - 建议：使用零分配转换（如 `fastrand::StrHash` 或手动 `char` 循环），或用 `SmallVec<[u8; 32]>` 避免堆分配。

- **`src/routing/resolve.rs:148-159` — `build` 闭包空字符串分支为 dead branch**
  - 现象：`let build = |agent_id: &str, ...| { if trimmed.is_empty() { normalize_agent_id(trimmed) } else { trimmed.to_string() } }`。
  - 证据：两个调用点——（1）`&b.agent_id` with guard `!b.agent_id.trim().is_empty()`（resolve.rs:169）；（2）`default_agent`（resolve.rs:176），`default_agent: &str` 来自 `session_cfg.default_agent.trim()`，其类型为 `String` 或 `&str`，若为空则逻辑矛盾（无人配置空字符串作为默认 agent）。
  - 分析：guard `!b.agent_id.trim().is_empty()` 已排除空字符串；`normalize_agent_id("")` 返回 `"main".to_string()` 是 `normalize_agent_id` 的定义行为，但 `build` 永远不接收空字符串。
  - 建议：移除空字符串分支（消除死代码 + 消除不必要的 `normalize_agent_id` 调用），或确认 `default_agent` 理论上可能为空。

- **`src/routing/resolve.rs:170` — `Option<String>` 参数不必要 clone**
  - 现象：`build(&b.agent_id, matched_by, b.match_rule.workspace.clone())` 中的 `.clone()` 将 owned `String` 再 clone 一份。
  - 证据：`b.match_rule.workspace` 类型为 `Option<String>`，闭包签名 `|agent_id, matched_by, workspace: Option<String>|`，接收 ownership 后向下传递（无再使用）。
  - 建议：改为 `build(&b.agent_id, matched_by, b.match_rule.workspace)`（move 语义，无需 clone）。

## P3 — 性能/风格（低置信度或边缘案例）

- **`src/routing/resolve.rs:223-224` — `"main".to_string()` 重复 3 次**
  - `resolve.rs:207`、`resolve.rs:223`、`session_key.rs:167`、`session_key.rs:215` 多处手写 `"main".to_string()`。虽然编译器可内联，但代码可读性差，且 `DEFAULT_MAIN_KEY` 已存在却未复用。
  - 置信度低：可能是有意避免跨模块 const 引用（crate 内部依赖问题）。

- **`src/routing/session_key.rs:369-373, 430-432` — `with_epoch`/`with_next_epoch` 克隆全结构**
  - `SessionKey` 含 `HashMap<String, Vec<u8>>` 等字段，clone 代价高；但这是公共 API 向后兼容约束，非本模块内部问题。

- **`src/routing/overlay.rs:56-64` — `match` 穷举 6 个 `MatchedBy` 变体**
  - 每个分支仅返回字符串字面量；可简化为 `HashMap<&MatchedBy, &str>` 或 `const` 数组查找。但这是 6 行可接受的穷举，复杂度在可接受范围。

## 误报自检

| 考虑项 | 放弃理由 |
|--------|----------|
| `resolve.rs` 中 `"main".to_string()` 是无必要的堆分配 | `DEFAULT_MAIN_KEY` 是 `pub const` 字符串，可能故意不跨模块引用以避免循环依赖；置信度 <80% |
| `ResolvedRoute::matched_by` 在 `gateway/router.rs` 中未使用 | `router.rs:233` 确实没用到 `resolved.matched_by`，但 `builtin_tools/gateway_route.rs` 有使用；`agent_resolver.rs` 保留返回值为未来扩展（comment 中注明）；不确定性高 |
| `resolve.rs` 的 `RouteInput` 构造在 `router.rs` 中的 clone | `router.rs:221` 中 `channel` 需要从 `Option<&str>` 转 owned，`args.channel.clone()` 后 args 仍需后续使用；clone 必要 |
| `experience_store.rs` 的 `normalize_key` 在每次 record 时分配 | `normalize_key` 内有多次 `to_lowercase()` + 正则，但每条记录写入才调用，不在 hot path；置信度低 |
| `recall.rs:67` 的 `unwrap_or(false)` | 语义正确：`configured_keys.get()` 找不到 = 未配置 = 不可用；非错误 |

## 未审查清单

- `src/routing/session_key.rs` 中 `SessionKey::serialize_to_wire_string` / `from_key_string` 完整解析路径（1464 行文件，约 1200 行为解析/测试逻辑，未逐行阅读）
- `src/routing/resolve.rs:291-285` 区域 `scope_satisfied` 函数的 `to_lowercase()` 调用（与 resolve_route hot path 相同的模式，但仅在绑定匹配时调用；频率低）
- `src/routing/experience_store.rs` `record_routing_experience` 中 `normalize_key` 的双重 lowercase（在 normalize_key 内部 + 调用处）；边界情况
- `src/routing/config.rs` 中 `JsonSchema` derive 宏展开是否产生冗余代码（宏层问题，本次只看手写代码）