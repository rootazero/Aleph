# FIX-IF1 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- IF1-03: FIXED
- IF1-02: FIXED
- IF1-07: FIXED
- IF1-05: FIXED
- IF1-06: FIXED
- IF1-04: FIXED
- IF1-09: FIXED
- IF1-11: FIXED
- IF1-13: FIXED
- IF1-12: FIXED
- IF1-10: FIXED
- IF1-18: FIXED
- IF1-16: FIXED
- IF1-17: FIXED
- IF1-19: FIXED
- IF1-14: ERROR(HTTP Error 524: status code 524)
- IF1-15: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- IF1-06: FIXED
- IF1-05: FIXED
- IF1-02: FIXED
- IF1-11: FIXED
- IF1-04: FIXED
- IF1-09: FIXED
- IF1-12: FIXED
- IF1-13: FIXED
- IF1-19: FIXED
- IF1-17: FIXED
- IF1-16: FIXED
- IF1-14: FIXED
- IF1-15: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- IF1-06: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- IF1-06: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- IF1-13: FIXED
- IF1-02: FIXED

---

# 第二轮区域拼接修复 / second-wave region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `kimi/kimi-for-coding`. One API call per finding.

- IF1-03: FIXED
- IF1-07: FIXED
- IF1-18: FIXED
- IF1-10: FIXED


---

# 第二轮修复复核 / second fix wave (appended)

# IF1 — Second fix wave / 第二轮修复

Implemented in the assigned worktree only. Existing unrelated/concurrent changes were preserved; no Git history operations or Cargo commands were run.
仅在指定工作树实施；保留既有及并发修改，未执行 Git 历史操作或任何 Cargo 命令。

The statuses below describe implemented repairs, not executed-test results. / 以下状态指代码修复完成，不代表测试运行通过。

## IF1-02 — FIXED — Ordered per-account polling acknowledgements / 按账户顺序确认轮询

`src/gateway/interfaces/telegram/polling.rs` processes updates sequentially through the actual handler and advances the cursor only after handler completion. Diagnostic fetches no longer advance it. `offset.rs` persists a separate initialization fact using row presence, including offset zero, and scopes progress to each account. Existing legacy progress prevents a fresh backlog discard without copying another bot's cursor.

按处理器完成顺序推进账户游标；诊断请求不再确认更新。初始化事实与数值零分离；旧游标迁移保留待处理消息，不跨 Bot 复制进度。

Regression coverage / 回归覆盖: `polling_acknowledges_only_completed_handlers_in_update_order`; `zero_offset_initializes_only_once_and_accounts_do_not_share_progress`; retained monotonic/persistence tests.

Trade-off / 取舍: sequential dispatch sacrifices concurrent handler throughput to make acknowledgement order explicit. Completion means the adapter handler finished (including enqueue), not durable completion of a downstream agent run. Persistence failures may replay updates; no exactly-once guarantee is introduced.

## IF1-05 — FIXED — Effective chat/topic configuration reaches normal delivery / 正常交付使用有效配置

`src/gateway/interfaces/telegram/mod.rs::send` clones the selected account's exact chat/topic resolution, not its startup defaults. Existing audit/error handling is retained.

Regression coverage / 回归覆盖: `channel_send_uses_effective_topic_config_and_live_bot_transport` checks topic typing suppression, topic ID, actual returned message ID and live transport reuse.

## IF1-06 — FIXED — Core resolves the producing account and topic policy / Core 按来源账户及 topic 授权

All Telegram inbound variants carry adapter-owned account context. The Core permission boundary resolves that account/chat/topic against the live channel's canonical resolver, using the shared `ChannelConfig::from(&ResolvedConfig)` reduction. Topic conversations use the chat portion for group allowlists. Unknown/missing multi-account context fails closed. Core permission and pairing checks remain authoritative; unrelated tool/workspace policies are preserved.

所有入站类型携带账户 I/O 上下文；Core 使用相同有效配置，保留最终授权及配对判据，不以 adapter 放行替代 Core 检查。

Regression coverage / 回归覆盖: `telegram_core_permission_uses_producing_account_and_topic_policy` exercises allowed/disabled DM accounts, allowed/disabled topics, unknown accounts and missing multi-account context.

## IF1-04 — FIXED — Empty account configuration returns ConfigError / 空账户配置返回错误

The channel constructor creates controllers only for actual accounts; it no longer fabricates an unresolvable default account. `start()` retains its existing configuration error.

Regression coverage / 回归覆盖: `empty_accounts_reaches_start_config_error_without_constructor_panic`.

## IF1-09 — FIXED — Final delivery failure reaches the emitter caller / 最终交付错误向上传递

Orchestrator terminal text finalization returns its real error. The emitter owns and awaits the orchestrator task on terminal events, mapping failures to the existing event error type. No automatic whole-response retry is added.

Regression coverage / 回归覆盖: `terminal_delivery_error_reaches_the_emitter_caller` asserts a rejected final send is observable by `emit`.

## IF1-11 — FIXED — Streaming reuses the account's live Bot transport / 流式交付复用账户连接

`src/gateway/inbound_router/executor.rs` obtains `TelegramDelivery` from the registered live Telegram channel. It no longer reparses the first account or constructs a direct `Bot::new` for streaming. The emitter accepts the complete delivery object, retaining its configured client/proxy.

Regression coverage / 回归覆盖: `channel_send_uses_effective_topic_config_and_live_bot_transport` verifies the captured streaming delivery retains the live Bot API endpoint and topic configuration; emitter HTTP tests use that supplied delivery. No real proxy deployment test was run.

## IF1-13 — FIXED — Streaming disable configuration remains effective / 禁用流式配置保持有效

Restored group/topic `block_streaming` fields. Topic overrides inherit group decisions, with explicit false unblocking the group while preserving account-level disabled streaming. Emitter selection and lane previews honor `enabled=false` and disabled mode; final text delivery remains available. The execution boundary also applies this effective disable decision after channel-capability reconciliation, so the generic ReplyEmitter fallback cannot turn streaming back on.

Regression coverage / 回归覆盖: `block_streaming_inherits_and_topic_can_explicitly_unblock`; `disabled_streaming_accumulates_without_preview_but_delivers_final_text`.

## IF1-12 — FIXED — Zero-capacity stream buffers fail at parsing / 零容量缓冲在边界拒绝

The existing config parser rejects `streaming.buffer_size=0` with `serde_json::Error`, including legacy upgrade inputs. Valid defaults/backpressure are unchanged.

Regression coverage / 回归覆盖: `zero_stream_buffer_is_rejected_in_v2_and_legacy_configs` also pins the default capacity at 256.

## IF1-16 — FIXED — Final text settles before one media drain / 文本完成后单次排空媒体

Terminal completion awaits the shared orchestrator task before draining run media. Only the terminal caller taking the task can drain; duplicate terminal events do not trigger another drain. The media-only leg never receives text events.

Regression coverage / 回归覆盖: `final_text_settles_before_media_and_keeps_disabled_link_previews`; retained media-drain/no-media tests.

Trade-off / 取舍: terminal `emit` now waits for Telegram finalization rather than acknowledging queue insertion; media ordering and delivery-error reporting necessarily add this wait.

## IF1-17 — FIXED — Mixed attachments retain input order / 混合附件保持顺序

`delivery.rs::send_attachments` flushes consecutive image albums before sending each non-image immediately, rather than postponing all non-images. Existing ten-image album boundaries and error propagation remain.

Regression coverage / 回归覆盖: `tests/telegram_delivery_e2e.rs::mixed_attachments_keep_input_order`; existing album tests retained.

## IF1-19 — FIXED — Short enabled reasoning tails finalize once / 短推理尾部按 opt-in 完成

Run completion finalizes a nonempty reasoning lane when explicitly enabled, even below preview thresholds. Reasoning markers and answer sanitization remain intact.

Regression coverage / 回归覆盖: `short_reasoning_tail_is_flushed_only_when_opted_in` checks opt-in versus opt-out and sanitized answer text.

## IF1-14 — FIXED — Effective link-preview policy reaches streaming / 流式使用有效链接预览配置

The emitter no longer fabricates a second `ResolvedConfig`. The live delivery carries the resolved link-preview policy unchanged.

Regression coverage / 回归覆盖: `final_text_settles_before_media_and_keeps_disabled_link_previews` checks the outgoing disabled-preview API field.

## IF1-15 — FIXED — Reactions target the real inbound message / Reaction 使用真实入站消息

The orchestrator receives the parsed `ReplyRoute.inbound_message_id`; absent IDs skip reactions. Synthetic message ID 1 is removed.

Regression coverage / 回归覆盖: `reactions_use_real_inbound_id_and_skip_absent_ids` checks ID 987 and the absent-ID branch.

## IF1-N1 — FIXED — Invalid proxy construction falls back without panic / 无效代理构造不再 panic

`bot_instance.rs` logs proxy/client setup failure and falls back to the existing direct Bot construction. No second proxy parser is introduced.

Regression coverage / 回归覆盖: `invalid_proxy_falls_back_without_panicking`.

## IF1-N2 — FIXED — Once/AdminOnly share an atomic cooldown branch / 冷却策略共享原子分支

`error_cooldown.rs` combines equivalent modes while retaining DashMap entry exclusivity; a naive insert-then-check would lose the first-winner distinction.

Regression coverage / 回归覆盖: `once_and_admin_only_have_one_concurrent_winner_per_scope` plus existing cooldown/window tests.

## IF1-N3 — FIXED — Attachment-only success returns its actual ID / 纯附件成功返回真实 ID

`delivery.rs` tracks the first successful attachment result as `Option<i64>`, converting only at the public send result boundary. Empty placeholder IDs are removed.

Regression coverage / 回归覆盖: `attachment_only_send_returns_real_first_message_id`.

## IF1-N4 — FIXED — Lane preview updates stay in one lock scope / Lane 预览单锁更新

The preview send remains in its tracker lock scope; message ID and timestamp are committed only after successful send/edit. Accumulated content remains available on failure.

Regression coverage / 回归覆盖: `failed_preview_edit_does_not_commit_delivery_timestamp`; disabled-preview/final-delivery coverage above.

## Validation and remaining scope / 验证与未做事项

- Scoped `git diff --check` passed. / 本批文件空白差异检查通过。
- Read-only `rustfmt --check --config skip_children=true` parsed all 17 scoped Rust files without parser diagnostics, but returned formatting differences in 15 files. This is not a clean formatting check; mass formatting was not applied over concurrent changes. / 17 个 Rust 文件无解析诊断，15 个仍有格式差异，未覆盖并发修改进行整文件格式化。
- No Cargo check/test/clippy/build was run, as instructed. Added tests have not been compiled or executed; type checking and runtime verification remain for the unified validation pass. / 按要求未运行 Cargo；测试未编译或执行，类型及运行时验证留待统一验证。
- Graphify queries were attempted before source inspection, but the assigned worktree has no `graphify-out/graph.json`. No graph was rebuilt. / 已尝试 Graphify 查询，工作树缺图，未重建。
- No real Telegram, real proxy, restart/crash integration, or external exactly-once validation was performed. No Git history operations were performed. / 未做 Telegram 真机、真实代理、崩溃重启集成或 exactly-once 验证，未操作 Git 历史。
- Existing broader account-selection ambiguity in ordinary multi-account send/approval and default-only `send_typing` are not expanded into this batch; IF1-05 explicitly concerns effective config after account selection. / 未扩展修复普通多账户发送及审批选账户歧义、独立 typing 使用默认配置问题；它们需另行讨论。
