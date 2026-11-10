# Hand-written fixes — orchestrator appendix

The fix phase ran on three tracks: the Agent-tool wave (cheap tier,
`MiniMax-M3.1-Flash`), the region-splice track (raw magpie API,
`kimi/kimi-for-coding` — the reviews that gated it ran on
`88apiclaude/claude-sonnet-5-5` / `minimax-cn/MiniMax-M3`), and a hand-written
track for everything the first two could not express against the real APIs. This file records the third
track, because the per-batch notes (`FIX-<BID>.md`) only carry one line per
finding and would otherwise hide who wrote what.

Every entry below was verified by the unified `cargo check -p alephcore
--all-targets` in the worktree; the regression tests that accompany them were
compiled but **not executed** (this round ran no `cargo test`).

## Reverted to HEAD — the region track's change could not be expressed

These three files were restored to HEAD, so the findings that lived in them
are **not fixed** and are listed in README §6 (有意没做的事):

- `src/gateway/mcp_face/http.rs` — GX6-01 (P0, MCP sessions not bound to the
  authenticated principal). The change needed an owner column that the
  in-memory MCP session table does not have; inventing one is a schema change,
  not a fix.
- `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs` —
  IF1-09 / IF1-10 / IF1-14 / IF1-15 / IF1-16. The region built a
  `ResolvedConfig` out of a `ReplyEmitterConfig` (which has neither
  `streaming` nor `error_cooldown` fields) and added an unbounded spin-wait
  (`loop { if capacity == max_capacity { break } yield_now().await }`) in front
  of the event send.
- `src/gateway/interfaces/nostr/mod.rs` — the relay-confirmation wait needs a
  pending-confirmation map that `NostrChannel` does not have.

## Reverted as scope creep (no finding asked for it)

- `src/gateway/interfaces/telegram/mod.rs` — the poll-answer access
  pre-filter. A `PollAnswer` update carries the voter but not the chat the poll
  lives in, so a per-chat policy cannot be resolved at the interface layer; the
  inbound router re-checks every message anyway (see `src/gateway/CLAUDE.md`:
  the interface side is a config-level pre-filter, the router owns the
  authoritative decision).

## Corrected after the fact (the region track's version was wrong)

- `src/gateway/security/device_token_manager.rs` (GX5-03, P2) — the region
  reordered the device-id conflict check to *after* the one-time ticket was
  consumed, so a sequential device-id conflict burned the operator's ticket.
  The check is back before `consume_bootstrap_ticket`, with a comment that the
  remaining race is advisory (the loser fails with `Invalid`).
- `src/gateway/interfaces/telegram/mod.rs` (IF1-18, P2) — the region dropped
  the health-task's `oneshot` sender on the spot, which ends the task on its
  first tick and silently disables the health probe. The sender now lives in
  `BotInstance::health_shutdown_tx` and `stop()` sends it; `stop()` also
  releases the instances so a later start does not select obsolete bots.
- `src/gateway/interfaces/telegram/mod.rs` (IF1-01, P0) — the instance
  selection now prefers an account that is configured for the target chat
  before falling back to the account-level catch-all.
- `src/gateway/handlers/providers/handlers.rs` (H5-03 / H5-04 / H5-05) —
  `update_provider_inner` was rewritten by hand: the DTO credential is reused
  through `resolve_api_key`, the candidate config is saved **before** the
  shared config is overwritten, and the provider is constructed before the save
  so a failed hot-reload reports an error instead of success.
- `src/gateway/handlers/mcp.rs` — the region's version referenced a
  non-existent `crate::protocol::JsonRpcError::method_not_found`; the real
  shape is `JsonRpcResponse::error(request.id, -32601, "…")`.
- `src/gateway/handlers/acp_config.rs` — the region's hand-built
  `AcpAdapterEntry { id, name, description, api_key_env_var, … }` literal
  does not exist (the real fields are `display_name`, `executable`, `args`, …,
  `preset`, `presence`); the fix is `user.hydrate_from_preset(&preset)`.
  The dropped `event_bus` parameter of `handle_set_enabled` was restored.

## Written from scratch against the real APIs

- E6-01 … E6-05 — the whole batch (see `FIX-E6.md`).
- H5-07 (P1, async catalogue handlers read the discovery cache synchronously)
  — `cached_models_async(...).await` replaces the sync `cached_models`, and the
  handler now clones the provider map and drops the config guard before the
  file I/O, so no lock is held across the await. The `has_api_key` gate is
  unchanged.
- `src/gateway/openai_api/completions/agent.rs` — the usage chunk is opt-in per
  request: `SseEventEmitter` carries `include_usage`, taken from
  `stream_options.include_usage`, and the usage numbers come from the run's
  token breakdown.
- `src/gateway/openai_api/responses/mod.rs` — the Responses dialect's flat
  tools are converted into the chat shape first, then through the shared
  `convert_openai_tools` / `convert_tool_choice`.
- `src/extension/manifest/cc_plugin_toml.rs` — an unknown `[aleph] runtime`
  is a manifest error instead of a silent fallback to `static`; the manifest's
  permission grants stay the converted list.
- `src/gateway/pty/screen/perform/mod.rs` — the screen patch reports the
  cursor position, and the last-sent cursor is tracked so an unchanged cursor
  is not republished.
- `src/gateway/execution_engine/gate.rs` + `execute.rs` (EE1-02, P1) — a run
  parked on the concurrency permit observes `/stop`: `admit_run` takes the
  run's cancel receiver and the token is consumed on purpose, because that run
  never starts.
- `src/gateway/execution_engine/slash_command.rs` (EE2-01, P1) — the direct
  tool fast path selects on the run's cancellation token.
- `src/gateway/channel_registry.rs` (G1-02, P1) — the inbound forwarder's
  `JoinHandle` is tracked in the registration entry, so a stop followed by a
  start does not install a second persistent forwarder.
- `src/gateway/interfaces/line/config.rs` — the duplicated group-policy
  method was renamed instead of shadowing the original.
- `src/gateway/pairing_store.rs` — an unreadable `created_at` counts as
  expired, so a pairing request cannot stay alive on a broken clock.
- `src/gateway/interfaces/matrix/media.rs` (IF4-28) — the media size cap is
  enforced on the downloaded body (`download_media_limited`). The *streaming*
  half of the finding is **not** fixed: matrix-sdk 0.19.1's
  `Media::get_media_content` returns a fully buffered `Vec<u8>` and exposes no
  streaming API, so a cap during the download would mean hand-building the
  media URL with the bearer token.
- `src/gateway/interfaces/discord/config.rs` — the derived `Debug` leaked the
  bot token; the implementation is now hand-written and prints `<redacted>`.
- `src/gateway/handlers/gateway_ticket.rs` — a non-string `user_id` is
  rejected with `INVALID_PARAMS` instead of minting an unbound owner ticket.

## The region-splice damage repair (orchestrator, after the fix waves)

The splice track's tool rewrites ±30-line regions without parsing Rust, so it
also **silently deleted working code**. Every damage site was repaired by hand
and re-verified against the base commit `79dacfa33` (never against the spliced
text). The full inventory — 9 syntax-broken files, 35 compile errors, ~15
semantic deletions, and the audit that confirmed nothing was left behind — is
in `evidence/splice-damage-repair.md`. The semantic deletions that mattered:

- `src/gateway/interfaces/cli.rs` — the stdin reader thread had been deleted;
  without it the CLI channel never receives input. Restored.
- `src/gateway/interfaces/nostr/message_ops/protocol.rs` — `build_dm` had been
  replaced by a `panic!`. Restored, and its doc now states the event is
  unencrypted and must never be published (the channel refuses DMs at the
  send site).
- `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs` — the
  `build_conversation_id` call and the `group_session_scope` resolution were
  gone. Restored.
- `src/gateway/interfaces/telegram/config_resolver.rs` — the group-level
  `streaming` override referenced a field that does not exist. Restored as the
  *intended* behaviour: the base never applied `block_streaming` at all, so the
  fix makes the account level map `StreamingMode::Disabled` to
  `enabled = false`, the group inherit the account setting and only block it,
  and a topic inherit the group (with `Some(false)` returning to the account
  setting).
- `src/gateway/handlers/memory/sessions.rs` — the store-level
  `list_sessions_by_ids` query was replaced by a full-corpus scan. Kept the
  scan, but batched it (`SCAN_BATCH = 100`), kept only the requested page and
  added `count_sessions` for the unrestricted caller's total.
- `src/gateway/interfaces/telegram/delivery.rs` — `send_attachment` returned
  `()`, which is the root cause of IF1-06: it now returns the real message id.
- `src/gateway/interfaces/telegram/streaming/orchestrator.rs` — a call to a
  non-existent `LaneHandle::flush()` was removed (the reasoning lane is
  settled by the existing `finalize_streamed()`), and the RunError arm now
  returns `ChannelError::SendFailed` instead of an `anyhow` error.
- `src/gateway/execution_engine/btw_wire_tests.rs` — the EE1 change added
  `ActiveRun::cancel_token` but the two test construction sites were not
  updated. Added `cancel_token: Default::default()` to both.
- `src/extension/marketplace/installer.rs` — two **pre-existing** digest tests
  still computed the old (no length prefix) digest while `directory_digest`
  had moved to the length-prefixed format; both tests were updated so they
  would not fail the moment `cargo test` runs.

---

## 最终跨族修复复核之后的返工（编排侧手写，2026-11-10 23:00–23:20）

跨族复核（`kimi/kimi-for-coding`，diff 基线 `79dacfa33`）在 15 批上给出
FIXED 128 / PARTIAL 27 / NOT_FIXED 14 / REGRESSION 2。下面这些条目由编排侧**对照基线提交**
逐个手改（不采信模型给的替换文本）：

| 条目 | 改动 |
|---|---|
| G1-01 | `OwnershipCache` 加代际；`cache_ownership_if_current` 在填充前校验代际，代际不符重读一次后 fail-closed；`forget_session` 在同一把锁内推进代际 |
| GX3-06 | REGRESSION：日志措辞回滚到基线原文（该文件对基线零 diff） |
| IF6-09 | REGRESSION：Signal 重试预算耗尽通过 `Arc<AtomicBool>` 带出，外层映射为 `ChannelStatus::Error` |
| IF5-09 | `WeChatRuntime` 持有注册时的 `ChannelId`，不再用平台名 `"wechat"` |
| IF1-08 | `EditedMessageIsTooLong` 移出良性错误集合；按纯文本切块（4096 字符）补发 |
| IF1-20 | 状态表情机的早退条件加上表情比较 |
| IF1-01 | 审批能力携带全部 bot 实例与按账号的 access 表 |
| IF4-29 | `convert_message` 增加 `muc_nick`，用实际 join 的 nick 过滤自己的房间消息 |
| H1-01 | `remove_user_hooks` 持 `HOOKS_MUTEX` |
| H1-04 | 非法 `active_hours` 返回 INVALID_PARAMS |
| H5-04 | `create_provider_inner` / `set_default_provider_inner` 先改副本、保存成功后替换共享 config |
| H6-02 | 搜索配置删除先存 config 再删 vault 密钥 |
| F1-02 | `FirecrawlFetchProvider::new_with_timeout(base_url, api_key, params.timeout_seconds)` |
| G1 / E5 / H5 / EE1 / EE2 / GX2 / GX4 / IF2 / E6 / H4 | 区域拼接删除的原有代码按基线语义补回（WeChat `build_conversation_id`、CLI 的 stdin 读取线程、Nostr 的 DM 构造等） |

未修完的复核条目（README §6 第 13 行）：GX1-08 / GX1-09 / GX1-10 / EE1-03 / IF6-12 / IF6-13 /
G1-04 / H5-14 / GX3-04 —— 需要新机制（任务注册表 / 槽位预留 / 对账循环），留到下一轮。
