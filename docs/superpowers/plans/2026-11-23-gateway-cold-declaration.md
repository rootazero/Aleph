# Gateway Cold Declaration Implementation Plan / Gateway 冷通道声明实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal / 目标:** Boot and create leave explicitly disabled channels uninstantiated; discover them from Config and apply current policy before an authorized explicit start can admit inbound messages. / 启动与创建保持显式禁用的声明为冷；从配置发现，授权显式启动须先落实当前策略。

**Architecture / 架构:** Config keeps declarations; `ChannelRegistry` keeps runtime identity, intent and admission. Reuse the router's existing policy projection for live and continuation consumers; cold activation commits the policy at registry publication, before transport start. / 配置管声明，registry 管运行身份、意图和准入。复用路由器的策略投影给运行与续跑消费者，冷激活在 registry 发布边界落实策略，再启动传输。

**Tech Stack / 技术栈:** Rust, Tokio, serde JSON, existing Gateway RPC and Cargo/`just` checks. / 复用现有 RPC 与检查。

**Spec / 设计:** [`2026-11-23-gateway-cold-declaration-design.md`](../specs/2026-11-23-gateway-cold-declaration-design.md), approved at `51f04846e`.

## Global Constraints / 全局约束

- Work only in the `gateway-cold-declaration` worktree. No edits, merges, pushes, or checkout changes on `main`. 小步本地提交，不动 `main`。
- Exact JSON boolean `false` is the only new boot/create disable decision; omitted or non-boolean values use incumbent parsing. Only live channels are callable; `channel.start` does not rewrite `enabled`.
- Retain existing Admin authorization, RPC names, enabled/default behavior and iMessage/BlueBubbles RPC factory limits. 无新依赖、Capability kind、supervisor、传输或 UI；不做 Tauri、WASM、NSH 优化。
- No gateway Zahir mount, Feishu NO-GO bypass, harness changes, native unload or post-stop reclamation claims. 先验证实际效果，再更新现状文档。
- Keep Task 1 as a supporting refactor queued before the behavioral commit; Task 2 is the first user-visible change. Do not ship only the boot gate without the activation-policy path.

## Review Focus / 重点审查

1. Cold alias with a live same-platform sibling → no policy overwrite or incumbent start (Tasks 1–2).
2. Cold declaration created after boot or edited before start → first ingress sees latest policy, including slash and tool restrictions (Tasks 1–2).
3. A malformed policy or refused publication → no fallthrough to defaults or incumbent runtime; no new ingress (Tasks 1–2).
4. Boot fails to see exact JSON `false` for iMessage or `channel.create` reads a supplied secret back from vault → prove no constructor/read or start, including option/default controls (Tasks 2–3).
5. A live row still has disabled config, or a concurrent duplicate create targets a config-only ID → runtime wins list/status once and original config and credentials remain intact (Tasks 2–3).

## File Map / 文件分工

- `src/config/structs.rs` — share declaration/type inference and the explicit disable test.
- `src/gateway/channel_policy.rs`, `src/gateway/inbound_router/{mod,types,permission,executor}.rs`, `src/gateway/pair_loop_guard.rs` — credential-free boot common fields, strict activation policy parsing, one shared Gateway policy projection for permission/slash/execution/resume.
- `src/gateway/channel_registry.rs` — use the existing source publication lock and entry operation admission for a vacant ID; share existing construction/publication/forwarding internals.
- `src/gateway/handlers/channel.rs`, `src/bin/aleph-server/commands/start/{mod.rs,builder/subsystems.rs,builder/handlers/session.rs}` — no boot/create factory work for cold rows, read-side union, authorized activation and policy handle wiring.
- `src/gateway/interfaces/signal/mod.rs` — test-only constructor observation if a factory test double alone cannot prove real Signal construction at the composition boundary. No change to Signal transport.
- `docs/reference/{FEATURE_LOCATOR,GATEWAY}.md` — verified current semantics. Evidence under `docs/superpowers/runs/` if runtime proof needs a durable receipt. Keep one-off runtime probe and raw logs outside the repo unless required to reproduce a checked-in regression.

### Task 1: Share policy projection and pin publication / 共享策略投影、验证发布边界

**Files:** `src/gateway/channel_policy.rs` (projection, continuation slot and tests); `src/gateway/inbound_router/{mod,types,permission,executor}.rs` (reads and conversions); `src/gateway/pair_loop_guard.rs` (checked config decode); `src/gateway/channel_registry.rs` (vacant lifecycle and tests).

**Interfaces / 接口:** `ChannelPolicyProjection` owns the existing per-channel `ChannelConfig` and `PairLoopGuardConfig` projection keyed by runtime ID. Add `ChannelPolicyProjection::get(&self, id: &str) -> Option<ChannelConfig>` and `ChannelPolicyProjection::set_activation(&self, id: &str, policy: ChannelConfig, pair_loop: Option<PairLoopGuardConfig>)`. Router and existing continuation slot share the same `Arc<ChannelPolicyProjection>`. Add `parse_channel_activation_policy(channel_type: &str, runtime_config: &Value, declared_config: &Value) -> Result<(ChannelConfig, Option<PairLoopGuardConfig>), ChannelError>` in the Gateway policy module. `runtime_config` may contain hydrated auth; `declared_config` is the same snapshot without injected auth and is the source of common fields. Add `ChannelRegistry::register_and_start_vacant(&self, channel: Box<dyn Channel>, before_publish: impl FnOnce(&ChannelId) + Send) -> ChannelResult<()>` for cold activation only.

- [ ] **1.1 Write tests / 先写测试.** `activation_policy_uses_latest_declared_denies` compares current and previous config with `allow_admin_from` and `tool_permissions`; assert a non-admin slash action is denied and continuation metadata carries the new tool deny. `activation_policy_rejects_invalid_access` checks malformed `tool_permissions`, slash list and pair-loop fields and no policy update. `vacant_activation_publishes_policy_before_start` uses a fake adapter whose `start` asserts the policy already exists for its ID; `vacant_activation_refuses_incumbent_without_publishing` checks zero callback/start calls and unchanged incumbent. Capture both failure and success, not call ordering via source strings.
  ```rust
  assert!(result.is_err());
  assert_eq!(incumbent_start_calls.load(Ordering::Relaxed), 0);
  assert_eq!(new_policy_publications.load(Ordering::Relaxed), 0);
  assert_eq!(projection.get("feishu-team").unwrap().group_policy, GroupPolicy::Disabled);
  ```
- [ ] **1.2 Verify red / 验证失败.** From the isolated worktree, capture the first baseline Cargo result and run `cargo test --locked -p alephcore --lib activation_policy` and `cargo test --locked -p alephcore --lib vacant_activation`. Expect the missing seam or failed behavioral assertions, not an unrelated compile error.
- [ ] **1.3 Replace the boot-only policy copies / 替换启动期副本.** Move existing maps behind the shared handle; boot registrations still assemble the same values. Publish that handle through the incumbent `CapabilitySlot` and keep readiness notifications. Wire permission, slash/tier/workspace/tool identity, pair-loop lookup and `system_continuation_identity` to it; take owned snapshots before async work. Reuse Telegram V1/V2 parser, Feishu/LINE/Matrix conversions and common access deserializers rather than a second table of meanings. Validate malformed fields on cold activation, keep enabled boot compatibility; if the pair-loop reader collapses invalid and omitted, expose checked decoding and keep the old defaulting behavior only for legacy boot. Redact errors returned to RPC clients; do not echo secret-bearing inputs. Preserve the current test helpers or migrate callers mechanically.
- [ ] **1.4 Pin registry publication / 固定 registry 发布边界.** Extend the incumbent registration internals with an explicit vacant path and truthful error return; keep original public legacy replacement wrappers for other callers. Reject an incumbent under the source publication lock before teardown or policy changes. Publish the policy after the last fallible registration check and before exposing the new entry. Reuse the existing entry operation marker for start and avoid re-looking up a replacement by ID. Do not hold a global registry lock across adapter start; test refusal and first admission rather than assuming callback success is admission.
- [ ] **1.5 Verify green / 验证转绿.** Rerun 1.2 plus `cargo test --locked -p alephcore --lib gateway::inbound_router` and `cargo test --locked -p alephcore --lib gateway::channel_policy`; each must pass. Include test evidence for both permission and system continuation readers. Commit this internal change only once the shared seam is consumed in Task 2, or commit the independently reachable projection refactor and stage the vacancy path with Task 2 so no unused production API ships.

### Task 2: Wire cold boot, create, reads and explicit start / 接通启动、创建、发现、显式激活

**Files:** `src/config/structs.rs`; `src/gateway/handlers/channel.rs`; `src/bin/aleph-server/commands/start/{mod.rs,builder/subsystems.rs,builder/handlers/session.rs}`. Tests beside handlers and composition. Exact consumer signatures: `Config::resolved_channel(&self, id: &str) -> Option<ChannelInstanceConfig>`; `channel_explicitly_disabled(config: &Value) -> bool` alongside the declaration type; `handle_status(request: JsonRpcRequest, registry: Arc<ChannelRegistry>, app_config: Arc<RwLock<Config>>) -> JsonRpcResponse`. `handle_start` adds `Arc<ChannelPolicyProjection>` to its existing arguments. Production start passes one projection handle to boot assembly and the Admin start handler, then publishes that handle for continuations.

- [ ] **2.1 Write red assertions / 编写红测.** `disabled_channel_declaration_rpc` checks `create.status == "disabled"`, `registry.get(id).await.is_none()`, `list.summary.total == 1`, `list.summary.disabled == 1`, `status.channel_type == "signal"`, zero advertised capabilities, unknown/malformed `channels.status` errors, no `send` activation and deletion. Test two creates of the same cold ID, including concurrent attempts with different secret input: exactly one persists and no second write changes the vault. `cold_status_prefers_live_runtime` checks a started disabled ID appears once with runtime status and on-disk `enabled == false`. `cold_start_uses_latest_policy` and `cold_start_invalid_policy_never_publishes` exercise RPC with a real policy reducer and deterministic local/fake transport: test both post-boot creation and post-boot tightening, the first forwarded event's outcome and invalid no-start/no-publish. `cold_telegram_feishu_boot_leaves_vault_unread` asserts zero per-ID secret reads and unchanged live sibling policy; `cold_boot_skips_imessage_blue_bubbles` asserts skip precedes both platform branches.
  ```rust
  assert_eq!(create["status"], "disabled");
  assert!(registry.get(&ChannelId::new("signal-cold")).await.is_none());
  assert_eq!(list["summary"]["total"], 1);
  assert_eq!(list["summary"]["disabled"], 1);
  assert_eq!(status["channel_type"], "signal");
  assert_eq!(status["capabilities"]["max_message_length"], 0);
  ```
- [ ] **2.2 Run red / 验证失败.** `cargo test --locked -p alephcore --lib disabled_channel_declaration_rpc`; `cargo test --locked -p alephcore --lib cold_start`; `cargo test --locked -p alephcore --bin aleph-server cold_`. Observe the failing behavior or missing seams on the baseline.
- [ ] **2.3 Move boot and create gates / 前移启动与创建判据.** Implement `resolved_channel` using the existing shared protocol inference, preserving order and warnings for `resolved_channels`. Use the exact-false predicate before iMessage/BlueBubbles or generic instance work, then delete the late post-construction boot decision; do not delete public boot APIs with other callers. `handle_create` checks duplicate config key while holding its write lock, before secret writes; reuse the existing secret extraction, persistence and failure rollback, then return cold before hydration/factory. Keep transport providers, vault key names, enabled platform hooks and authorization intact.
- [ ] **2.4 Wire reads and activation / 连读取与激活.** In list/status, share the cold response builder and no-capabilities payload; preserve legacy pending rows and delivery queue fields; derive one registry summary from the same list view when adding the cold counts. For config-only `channel.start`, read one current declaration, validate common and transport policy using Task 1, hydrate only here, construct via the existing factory plus enabled-path platform preparation, require matching runtime ID and successful vacant admission. Read policy before transport opening; construction failure is an error, not an incumbent restart. Preserve the old restart branch for an existing runtime. Keep non-secret boot common policy under the cold instance's own ID (including tool deny and applicable pair-loop policy); skip its Telegram/Feishu credential-backed reducer at boot. Do not use legacy alias heuristics for a cold row; keep enabled boot policy untouched.
- [ ] **2.5 Run green and commit / 验证转绿并提交.** Run 2.2, `cargo test --locked -p alephcore --lib gateway::handlers::channel`, and Task 1 filters. All must pass. `git diff --check` and commit `gateway: activate disabled channel declarations only on explicit start` in the worktree, including Task 1 pieces not already committed. Check handler method keys and scopes against existing tests.

### Task 3: Verify real Signal, record evidence and update references / 真机验证与参考文档

**Files:** `src/gateway/interfaces/signal/mod.rs` only if a test-only constructor observer is needed; boot and RPC tests in the owners above; `docs/reference/{FEATURE_LOCATOR,GATEWAY}.md`; optional dated run receipt under `docs/superpowers/runs/`.

- [ ] **3.1 Pin real construction / 锁定真实构造效果.** Add `cold_signal_factory_boundary` via real composition and RPC tests, using the real factory and an observation scoped to test IDs (not a global mutable factory override). Assert exactly 0 Signal constructions for boot/create and 1 for explicit start; for omission and `true`, assert construction and transport activation on previous paths. For boot use the server test build and real composition; for create verify persistence plus dedup/status. Where fixtures do not permit real transport, use the loopback smoke in 3.3 in addition, not instead of the constructor check. First run the observation against pre-gate behavior where practical; no network requests alone cannot prove no constructor.
  ```rust
  assert_eq!(constructs_for("signal-cold-boot"), 0);
  assert_eq!(constructs_for("signal-cold-create"), 0);
  // For the same instance after its explicit start:
  assert_eq!(constructs_for("signal-cold-create"), 1);
  ```
- [ ] **3.2 Build and run targeted suites / 构建并运行定向套件.** `cargo test --locked -p alephcore --bin aleph-server cold_`; `cargo test --locked -p alephcore --lib cold_`; Task 1 checks; `cargo build --locked -p alephcore --bin aleph-server`. Require actual successes; preserve the first failing command and exact failure if blocked. Run against the worktree source; an external Cargo target cache is okay, but not a server binary built at a different revision.
- [ ] **3.3 Exercise the built server / 运行真实 server.** Start with isolated `ALEPH_HOME`, temporary config, loopback Gateway port, test-only credentials, `channel.create`/`channels.list`/`channels.status`/`channel.start` through the existing authenticated WebSocket handshake (reuse `qa/mcp_face/drive.py` protocol). Serve a local Signal SSE endpoint; record request paths by test phone number or separate listener. Assert zero cold upstream requests, normal enabled/default boot/create requests, explicit cold start reaching its own endpoint, disabled key still false on disk, and config-only delete. Use bounded waits and stop child processes on all exits. Evidence identifies the binary revision, commands, receipts, expected vs actual, and unavailable cross-platform/transport checks.
- [ ] **3.4 Correct reference wiring and commit / 修正文档并提交.** In `docs/reference/FEATURE_LOCATOR.md` and `docs/reference/GATEWAY.md`, distinguish cold declaration/read, active runtime and source-local Capability work; report status precedence and activation policy; do not claim stop reclamation or a Gateway Zahir mount. Run `just check-wiring`, `git diff --check` and `git status --short --branch` in the worktree and verify original `main` was not changed. Commit the relevant tests, docs and any evidence locally. Obtain one fresh whole-branch review before reporting completion; no cloud or desktop QA claims without evidence.

## Execution notes / 执行说明

- The Gateway library suite covers Tasks 1–2 together. For Task 3.1, the native probe drives the built server's real boot and RPC paths and counts ID-tagged events at the actual Signal constructor, instead of adding a separate server-unit seam. It also verifies the first inbound event against current policy. / Gateway 库测试合并覆盖任务 1–2；任务 3.1 用构建后的 server 验证真实启动和 RPC，并按实例统计实际 Signal 构造事件，不另加启动单测接缝。探针也用首条入站消息验证最新策略。
- Independent review tightened three existing boundaries: no Telegram V2-to-V1 downgrade for a malformed cold activation, the existing outbound delivery-policy admission gate stays in force, and failed cold creation cannot leave credentials from that request for a subsequent declaration. / 独立复审收紧三处既有边界：冷启动时错误 Telegram V2 不降级到 V1；继续执行现有出站投递策略闸门；冷创建失败不会为后续声明遗留本次凭据。
- Implementation `9976b7840`; see the [Linux validation receipt](../runs/2026-11-23-gateway-cold-declaration-linux.md) for the positive results and the five pre-existing whole-Gateway failures left out of scope. / 实现提交 `9976b7840`；通过项及本轮未修复的五项既有全量失败见 [Linux 验证回执](../runs/2026-11-23-gateway-cold-declaration-linux.md)。
