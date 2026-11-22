# region-splice 损伤与修复（2026-11-10）

## 1. 背景

第 3 波修复代理改用 raw magpie API 的 **region splice** 工具（`/tmp/region2.py` → `region4.py`）：
把模型重写的 ±30 行区域直接拼回原文件。选它的原因不是技术偏好，而是额度事故：

- `88apigpt`（Agent 工具的 opus/sonnet/haiku 别名）余额归零：`403 余额不足：当前余额 ¥0.01`；
- 随后 `88apiclaude` 也归零：`88apiclaude: 余额不足：当前余额 ¥-0.14`；
- 仍可用的：`kimi/kimi-for-coding`、`minimax-cn/MiniMax-M3`、`minimax-cn/MiniMax-M3.1-Flash`。

**该工具不解析 Rust**：它按行号窗口替换文本，不检查括号平衡、不检查语法、不检查语义。
结果是**静默**的——`cargo check` 只暴露语法与类型层面的破坏，语义删除需要人工对照基线才能发现。

受影响批次（region-splice 实际处理过的 10 批）：
H3、IF1、E5、IF5、IF6、H5、H1、GX1、EE1、G1。

## 2. 损坏清单

### 2.1 语法损坏（9 个文件，`rustfmt --edition 2021 --check` 报 parse error）

| 文件 | 错误 | 处置 |
|---|---|---|
| `src/gateway/handlers/hooks_admin.rs` | `unexpected closing delimiter` (245:1) | 删除 `append_user_hook` 结尾多出的 `}` |
| `src/gateway/interfaces/matrix/dedupe.rs` | `unexpected closing delimiter` (71:9) | 文件里有两个 `try_insert` 实现，删除 67-83 行的残留片段 |
| `src/gateway/interfaces/nostr/message_ops/ops.rs` | `unterminated double quote string` | 补回 `"Nostr: unrecognized relay message: {}",` 的收尾引号 |
| `src/gateway/interfaces/telegram/delivery.rs` | `unexpected closing delimiter` (396:9) | 删除 `match first_msg_id` 之后的 `return Err(err); } }` 残片；把被复制错乱的相册 flush 尾巴恢复成单份 `if !image_buf.is_empty() { flush_image_album(...).await?; image_buf.clear(); }` |
| `src/gateway/interfaces/telegram/handlers.rs` | `mismatched closing delimiter` (186:21) | 补回 `u64::from(largest.file.size),` 的 `)` |
| `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs` | `unexpected closing delimiter` (78:14) | 模型把区域**带着行号**回写（11 行匹配 `^\s*[0-9]+ \| `）并丢掉了测试模块头；模块头按基线 + 新的 3 参数 API 手工重建（见下） |
| `src/gateway/server/attachment_route.rs` | `unexpected closing delimiter` (84:1) | 删除多余 `}` |
| `src/gateway/server/connection/dispatch.rs` | `unexpected closing delimiter` (203:5) | 删除两段重复的尾巴，保留 `revalidate_for_dispatch` 的原意 |
| `src/gateway/session_manager/ops/modify.rs` | `mismatched closing delimiter` (79:28) / `unclosed delimiter` (711:3) | 补上 `self.query_blocking(move |conn| {` 的闭合 `})` + `.await` |

### 2.2 语义损坏（编译通过、行为被静默改写）

| 文件 | 被删/被改的行为 | 处置 |
|---|---|---|
| `src/gateway/interfaces/cli.rs` | **stdin 读取线程整段消失** —— CLI 通道永远收不到输入 | 按基线恢复 `std::thread::spawn(move || { let stdin = io::stdin(); for line in stdin.lock().lines() { … line_tx.blocking_send(text) … } });` |
| `src/gateway/interfaces/nostr/message_ops/protocol.rs` | `build_dm` 的实现被换成 `panic!("DM encryption not yet implemented…")` | 恢复原实现；doc 改写为"本实现不做 NIP-04 加密，事件体对所有索引它的 relay 明文可见，因此**不得**直接发布"；发布路径（`NostrChannel::send`）保留 `UnsupportedFeature` 拒绝 |
| `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs` | `build_conversation_id(...)` 调用与 `group_session_scope`（Group / User / Thread）整段消失 | 恢复调用与作用域分支 |
| `src/gateway/interfaces/telegram/config_resolver.rs` | 组级 `streaming` 覆盖被写成不存在的字段（E0609） | 恢复**意图中的行为**（不是删掉用法）：基线里 `TelegramGroupConfig` 根本没有 `streaming` 字段，`block_streaming` 在基线里是死配置；修复后 = 账户级把 `StreamingMode::Disabled` 映射为 `enabled=false`，组级继承账户设置且只能关，话题级继承组且 `Some(false)` 回到账户设置 |
| `src/gateway/handlers/memory/sessions.rs` | `list_sessions_by_ids` 的存储级查询被换成"全量扫描 + 内存过滤" | 不引入新的 trait 方法（要一次 IN 大量 id，受 SQLite 参数上限与跨批分页影响）；改为**分批扫描**（`SCAN_BATCH = 100`）+ 只保留请求的那一页 + 未受限调用方用新增的 `count_sessions` 取总数 |
| `src/extension/runtime/wasm/capability_kernel.rs` | 每执行计数器的重置被改名成不存在的字段（E0424） | 改为真实字段 `log_count`；并手写 `Debug` 实现（`secret_resolver: Arc<dyn SecretResolver>` 是 trait object，不能 derive） |
| `src/extension/runtime/wasm/mod.rs` | `LoadedWasmPlugin` 的 `kernel` 字段没初始化（E0063）；`self.reset_per_execution_counters()` 挂在无 self 的关联函数上（E0424） | 补 `kernel: kernel.clone(),`；改成 `loaded.kernel.reset_per_execution_counters();` |
| `src/fetch/providers/firecrawl.rs` | 新增的 `scrape_url: url::Url` 字段没有初始化（E0063） | 构造函数只解析一次 base URL，`path_segments_mut().extend(&["v2","scrape"])`；`fetch` 改用 `self.scrape_url.clone()`；删除随之无用的 `base_url` 字段 |
| `src/gateway/session_manager/ops/modify.rs` | `self.read_recent_messages(...)` → 关联函数 `Self::read_recent_messages(&conn, &key_str, 64)` | 已改 |
| `src/gateway/interfaces/slack/message_ops/debouncer.rs` | `%msg.conversation_id` / `%msg.id`（`MessageId`/`ConversationId` 没有 `Display`） | 改用 `.as_str()` |
| `src/gateway/interfaces/line/message_ops.rs` | flex 推送被写成不存在的 `PushMessage::Flex` / `FlexMessage` | 恢复 `LinePushPayload::Flex(Box::new(FlexPayload::new(to, alt_text, contents)))`；`FlexPayload.to` 加 `#[serde(skip)]`，信封由 `push()` 统一写 `{"to":…, "messages":[…]}` |
| `src/gateway/interfaces/telegram/streaming/orchestrator.rs` | 调用了不存在的 `LaneHandle::flush()`；`Err(anyhow::anyhow!(...))` 与返回类型不符 | 删除 flush 块（reasoning lane 由既有的 `finalize_streamed()` 收尾）；错误改为 `ChannelError::SendFailed` |
| `src/gateway/interfaces/telegram/delivery.rs` | `send_attachment` 返回 `()`，调用方拿不到消息 id（IF1-06 的根因） | 改成返回 `ChannelResult<i64>`，各分支统一 `Ok(sent.id.0)` |

### 2.3 既有测试被本轮改动作废（`cargo test` 范围外，但属真实缺陷）

`src/extension/marketplace/installer.rs` 的两个 digest 测试仍按旧格式（路径+内容直接拼接）计算期望值，
而 `directory_digest` 已改成**长度前缀**格式（`hasher.update((portable.len() as u64).to_be_bytes())` 等）。
两个测试（`test_verify_plugin_integrity_match`、`directory_digest_uses_portable_path_separators`）已按新格式改写。

## 3. 审计：怎么确认没有残留的语义删除

1. **`/tmp/deleted_scan.py`（确定性）**：把 diff 里"被删且在新文件里再也找不到"的行列出来 ——
   260 个改动文件共 829 条候选，10 个 splice 批次占 324 条。高风险调用点逐个人工对照基线：
   `channel_registry.rs::start_channel_inner`、`delivery_queue.rs` 的带 `channel_id` 守卫的 inflight UPDATE、
   `execution_engine/execute.rs::admit_run`、`gate.rs` 的 `concurrency.acquire` —— 全部保留 ✓。
2. **`/tmp/review_splice.py`（模型，跨族）**：把每批的 diff 交给推理模型，要求"找出 diff 删除的、
   这次改动本不该碰的行为"。两族：`minimax-cn/MiniMax-M3`（`/tmp/splice_audit/`）与
   `kimi/kimi-for-coding`（`/tmp/splice_kimi/`）。
   **实测：该模型在大 diff 上会凭空编造"被删的行"**（例如基线里根本不存在的
   `group.streaming.clone().unwrap_or_else(...)`）。因此每条判定都必须人工对照基线 `79dacfa33` 复核——
   本轮 10 个批次里，绝大多数"损伤"判定经复核为误报，真正的问题就是上面 2.1 / 2.2 里已经列出的那些。
3. **编译门**：`cargo check --all-targets` 兜住语法 / 类型层；**语义层只能靠人工 + 上述审计**。

## 4. 结论

- 9 个文件语法损坏、35 处编译错误、约 15 处静默语义删除 —— **全部已修复**，见 §2。
- 修复过程中没有引入新依赖，没有放宽任何安全语义。
- 教训：region splice 这类"按行号盲写"的工具**不能**用于 Rust 源码；
  如果额度受限必须用它，就必须配套一个"解析 + 类型检查 + 语义删除审计"的门，
  而不是只跑 `cargo check`（后者只覆盖前两者的一半）。
