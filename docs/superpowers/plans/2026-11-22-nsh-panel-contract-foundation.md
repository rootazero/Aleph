# NSH Panel 契约基础（S0）/ Contract Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for direct work and superpowers:subagent-driven-development for isolated implementation/review when justified. Steps use checkbox (`- [ ]`) syntax for tracking. Follow the user's selective delegation rule; do not dispatch an agent for every task.

**Goal:** 修正现有 Gateway 的协议协商，提供下一阶段可复用的帧边界，并让架构文档区分迁移目标与实际挂载。

**Architecture:** 共享 Protocol 提供纯协商与长度边界，不持有 I/O、身份或业务状态。现有 Gateway hello handler 使用真实实现集合；目前不启用任何可选特性。S0 不接新入口，旧 `/ws` 仍以 `connect` 认证并传输 text JSON。

**Tech Stack:** Rust、Serde、现有 `aleph-protocol` 与 `alephcore`，无新依赖。NSH 仍锁定 `=0.6.0`。

**Spec:** [已批准设计](../specs/2026-11-22-nsh-panel-modular-refactor-design.md) §5、§7 S0、§8、§9。随后按 [S1–S6 路线图](2026-11-22-nsh-panel-migration-roadmap.md) 分包计划。

## Global Constraints / 全局约束

- 只在 `nsh-panel-refactor` worktree，产品变更需先得到本计划确认。每一任务本地单独提交；不操作 main、远端、已安装应用、正式服务、正式登录项或凭据。
- Rust Core 保持业务事实、授权和 Capability 唯一权威。不得新建 Registry、Business API、事件数据库、UI runtime 或独立授权路径。
- `ProtocolVersion { major: 1, minor: 0 }`；4 字节 unsigned big-endian 长度，正文 UTF-8 JSON，v1 默认正文上限 `8_388_608` 字节（不含头）。限长判定发生在申请正文缓冲之前。
- `/ws` 保留原 `connect`、text JSON、分发和传输行为；不得启用 `/v1/ws`、Cookie exchange、UDS、Named Pipe 或 transport 自动回退。S0 唯一生产行为变化是现有 opt-in `system.hello` 如实协商版本与可选特性。
- Gateway `IDEMPOTENCY_KEY_REQUIRED = -32030` 的含义保持不变；不分配新错误码。现有 `HelloResult` 形状不变。S0 不宣告 snapshot、event replay 或 binary 支持。
- 修改前跑受影响 baseline；每次 Cargo 调用用 `python qa/browser_live/check_memory.py && cargo …`，门禁不通过即停。已有测试先读项目适用技能；任何阻塞如实记录，不以源码检查冒充运行结果。
- ADR-006～009 仍 Proposed。未落地的 transport/UI 标记目标，Phase11/历史证据不改写；真实三平台 + 共同 14 天双轨闸门和回退路径不变。

## Review Focus / 五类容易漏测的输入

1. 客户端给 `minor = u32::MAX`，服务端不能回显为支持版本；另测服务端更高时以客户端为准（任务 1）。
2. 同名能力重复或认得名字却未实现，双方交集不重复、不把识别当支持；生产注册路径真的返回空集合（任务 1）。
3. 必填参数缺失或错误类型，必须返回 `-32602` 与原 request ID，不落入成功协商（任务 1）。
4. 读头分段或声明巨长但没有正文，只有完整头可确定长度，有完整非法头必须立即拒绝（任务 2）。
5. 多帧接续中的 UTF-8/边界：下一帧不得吞掉；字符编码错误拒绝，未读完正文不得误判 UTF-8（任务 2）。

---

## File map / 文件归属

| File | Responsibility |
|---|---|
| `shared/protocol/src/system_hello.rs` | 已有 wire DTO 与无 I/O 的版本、特性协商；不迁出 wire schema |
| `src/gateway/handlers/system_hello.rs` | 已有注册路径解析和错误映射，传入实际支持集合 |
| `shared/protocol/src/framing.rs`（新）及 `shared/protocol/src/lib.rs` | 新 v1 可重用头/正文边界与 UTF-8，不是 transport、JSON parser 或 RPC middleware |
| `AGENTS.md`、`CLAUDE.md`、`docs/reference/{REDLINES,DESKTOP_SHELL,FEATURE_LOCATOR}.md`、`docs/research/aleph_modularization_plan.md` | 对齐宪法、现状、目标及路线；保留历史记录 |

### Task 1: 协商实际版本与特性 / Negotiate actual support

**Files:** Modify and test `shared/protocol/src/system_hello.rs`, `src/gateway/handlers/system_hello.rs`。`src/gateway/handlers/mod.rs:326` 已注册 `system.hello`；`src/gateway/server/connection/mod.rs` 与 `shared/client/src/connection.rs` 不修改。

**Interfaces:** 保留 `pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse`、`HelloParams`、`HelloResult` 和既有 Feature wire 名。提供 `pub fn negotiate_version(client: &ProtocolVersion, server: &ProtocolVersion) -> Option<ProtocolVersion>` 及 `pub fn negotiate_features(requested: &[Feature], implemented: &[Feature]) -> Vec<Feature>`，位于 `aleph_protocol::system_hello`。不让握手协商函数验证身份、构造连接 ID 或承诺 limits；S1 由经过认证的 transport owner 传入真实连接 ID 和限制。

- [ ] **Baseline.** 在 worktree 根运行下列命令；记录已有用例结果和工具版本。若目标测试没有运行（例如 filter 命中 0），先改正命令再继续。

```bash
python qa/browser_live/check_memory.py && cargo test --locked -p aleph-protocol
python qa/browser_live/check_memory.py && cargo test --locked -p alephcore --lib gateway::handlers::system_hello::tests
python qa/browser_live/check_memory.py && cargo test --locked -p aleph-client
```

- [ ] **RED: 改现有 handler 用例并新增失败输入。** `hello_reports_implemented_features_and_negotiated_minor` 修改原成功用例：真实 `handle(req(...))` 传入 `{major: 1, minor: u32::MAX}` 与 `event_replay`、`snapshot`、`binary_channel`、`future`，断言 `protocol == {major:1,minor:0}`、`features == []`、`server.name == "aleph-server"`、ID `1` 和非空 `connection_id`。用 `major_version_mismatch_returns_error_not_success` 断言 error `-32602`、无 result、原 ID；`invalid_hello_params_keep_request_id` 分别覆盖 params 缺失、缺 `client`、`minor` 为字符串；删除原未知特性用例中“recognition 即支持”的断言，未知项已由新成功用例覆盖。
- [ ] **运行 RED。** 重跑 Core hello 命令，预期 `hello_reports_implemented_features_and_negotiated_minor` 因现有回显/宣告行为失败；意外编译错误或 timeout 不是本缺陷的 RED 证据。
- [ ] **添加共享函数用例。** `version_uses_lower_minor_and_rejects_major_mismatch`：`(1,0),(1,3) -> Some((1,0))`，`(1,u32::MAX),(1,0) -> Some((1,0))`，`(2,0),(1,0) -> None`。`features_are_an_ordered_unique_intersection`：requested `[Unknown("future"), BinaryChannel, Snapshot, BinaryChannel, EventReplay]`，implemented `[Snapshot, BinaryChannel]`，assert `[BinaryChannel, Snapshot]`；空 implemented 得 `[]`。另断言未知字符串 DTO 往返保留（复用同一文件既有 serde 测试）。
- [ ] **实现共享签名。** major 精确匹配，minor 取较小值；按 requested 顺序返回实际 implemented 的各唯一 feature，`Unknown` 不是可协商的实现项。`is_known()` 可用于排除未知项，不能作为支持集合的替代。
- [ ] **接到既有 handler。** `handle` 用本地 1.0 实现版本及空 implemented 列表，调用上述函数、保留 `INVALID_PARAMS` 映射及现有返回组装。更新模块文档：legacy hello 仅是 connect 后的探测；本任务不以 `connection_id` 作为登录主体。现有数值 `8388608/128` 仅保留为 legacy `HelloResult` 的兼容提示，不宣称是 `/ws` 实际执行的上限；不新增/收紧 legacy 限流。新 v1 上限未接线前不能供准入使用，这一局限记录在任务 3。
- [ ] **GREEN。** 重跑三条 baseline 命令；预期既有客户端套件及改后 hello assertions 全绿。检查 `git diff --check`，确认没有改 `/ws` route、`connect`、认证/分发及幂等错误映射。
- [ ] **Commit.** `git add shared/protocol/src/system_hello.rs src/gateway/handlers/system_hello.rs && git commit -m "fix(protocol): negotiate implemented hello features"`。

### Task 2: 共享帧边界 / Define reusable v1 frame boundaries

**Files:** Create and unit-test `shared/protocol/src/framing.rs`; export `pub mod framing;` in `shared/protocol/src/lib.rs`。不接任何 socket 或 TS client，不新增 crate。

**Interfaces:** `pub const HEADER_BYTES: usize = 4;`、`pub const DEFAULT_MAX_PAYLOAD_BYTES: u32 = 8_388_608;`。公开 `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FrameError { Empty, Oversize, InvalidUtf8 }`。供 S1 acceptor/transport adapter 使用：

```rust
pub fn encode_header(payload_len: usize, max_payload: u32) -> Result<[u8; HEADER_BYTES], FrameError>;
pub fn decode_header(header: [u8; HEADER_BYTES], max_payload: u32) -> Result<u32, FrameError>;
pub fn split_frame(bytes: &[u8], max_payload: u32) -> Result<Option<(&str, &[u8])>, FrameError>;
```

`Ok(None)` = 还不完整，无消费；`Ok(Some((payload, remaining)))` = 仅消费一帧，视图借用输入；`Err` = 终止此 v1 framing，不扫描下一段重同步。调用方传入有效 transport 上限（可比默认低）；JSON 语法/深度/schema 检查和读写 EOF 由未来拥有 I/O/解析的入口负责，不能把 `split_frame` 成功当 JSON-RPC 校验成功。

- [ ] **RED: 添加用例。** 先建同文件最小未实现函数签名和 module export，测试断言如下。
  - `header_is_big_endian_and_limits_are_inclusive`：`encode_header(258, 258) == Ok([0,0,1,2])`、`decode_header([0,0,1,2], 258) == Ok(258)`；`8_388_608` 可接受，`8_388_609` 拒绝（只构造 header）；`encode_header(9, 8) == Err(Oversize)`，在 64-bit 输入 `u32::MAX as usize + 1` 不截断。
  - `empty_and_oversize_headers_fail_before_payload`：仅 `[0,0,0,0]` 得 `Err(Empty)`，仅 `[255;4]` 在默认 cap 得 `Err(Oversize)`，`encode_header(0,8) == Err(Empty)`。不需要大分配。
  - `frame_prefixes_do_not_consume_partial_header_or_body`：构造 `[0,0,0,5] + b"false"`；每个长度 `0..9` 的前缀 `Ok(None)`，全帧 `Ok(Some(("false", &[])))`。
  - `split_returns_borrowed_exact_frame_and_rejects_invalid_utf8`：连续 `[0,0,0,4] + b"null"` 与 `[0,0,0,4,34,0xc3,0xa9,34]`，先返回 `"null"` 与完整第二帧，后返回 JSON 文本 `"\"é\""` 与空余；第二帧长度为 6 的前缀含半截 Unicode，仍 `Ok(None)`；`[0,0,0,1,0xff]` 得 `Err(InvalidUtf8)`。
- [ ] **运行 RED。** `python qa/browser_live/check_memory.py && cargo test --locked -p aleph-protocol framing::tests`。预期四个测试进入并在尚未实现函数处失败，而不是编译失败或未运行。
- [ ] **实现三处签名。** 以 `u32::from_be_bytes` / `to_be_bytes` 和 checked conversion 实现同一非零/上限判据。`split_frame` 完整头后立即调用 `decode_header`；从去头后的 slice 比较正文实际长度，避免 `header + length` 溢出。只校验完整正文 UTF-8，返回借用分片，不分配。无 async/runtime 依赖。
- [ ] **GREEN。** `python qa/browser_live/check_memory.py && cargo test --locked -p aleph-protocol`；预期新增 4 个测试全绿且既有包测试无失败；用 `git diff --check` 确认变更仅在共享边界。
- [ ] **Commit.** `git add shared/protocol/src/{framing,lib}.rs && git commit -m "feat(protocol): add bounded v1 frame primitives"`。

### Task 3: 同步架构现状和退场台账 / Align source of truth and migration ledger

**Files:** Modify `AGENTS.md`, `CLAUDE.md`, `docs/reference/REDLINES.md`, `docs/reference/DESKTOP_SHELL.md`, `docs/reference/FEATURE_LOCATOR.md`, `docs/research/aleph_modularization_plan.md`。Consumes Task 1–2 installed source anchors and verified evidence; produces dated current-vs-target note and roadmap links.

- [ ] **Pin the approved rule.** 同步两个入口的 R2、Tech Stack 和 `REDLINES.md` R2 引用，采用设计 §9 的规范：复杂业务 UI 使用唯一共享呈现源；宿主/Bridge 仅做平台与协议适配；Core 保持唯一业务事实与授权权威。目标明确写共享 TSX + Native Components/Web fallback；当前 Leptos/WASM、Tauri 回退及 NSH WebView 仍按事实记录。去掉将 Leptos 写成永久红线的表述，不删除现有功能约束。
- [ ] **区分当前挂载与阶段目标。** 在 `docs/reference/DESKTOP_SHELL.md` Phase10 后和 `docs/reference/FEATURE_LOCATOR.md` §7（约 4705 行）加交叉入口。写出已接线 [`desktop/nsh-shell/src/main.rs`] 的 WebView、本阶段已修正的 Gateway hello、`shared/protocol/src/framing.rs` 是未接线 library、legacy limits 为提示；新 v1 transport、shared TSX、S4 replay 尚未交付。记录 `panel-modular` 当前 feature 边界和真实 `PanelShell` 渲染点，链接 `docs/superpowers/plans/2026-11-22-nsh-panel-migration-roadmap.md` 的保留条件。只改 `aleph_modularization_plan.md` 描述当前/目标 UI 的段落（当前约 L655），不改 L1/L2/L3 契约或历史事实。
- [ ] **交付证据检查。** `git diff --check`；用 `rg -n 'R2|核心栈' AGENTS.md CLAUDE.md` 比较对应文本；`git diff -- src/gateway/server/connection src/gateway/protocol.rs` 应无输出；`git diff 4f951d4d1 --name-only` 复核无 phase receipts、installed/default、client、manifest 或锁文件改变。确认新增文档相对链接实际可打开，并用 `rg -n 'pub fn (split_frame|negotiate_version)|pub async fn handle|PanelShell' shared/protocol/src/{framing,system_hello}.rs src/gateway/handlers/system_hello.rs interfaces/webchat/src/app.rs` 固定可核验锚点。
- [ ] **Commit.** `git add AGENTS.md CLAUDE.md docs/reference/{REDLINES,DESKTOP_SHELL,FEATURE_LOCATOR}.md docs/research/aleph_modularization_plan.md && git commit -m "docs(nsh): distinguish panel migration target from current mounts"`。

## Exit criteria / S0 退出条件

- Task 1–2 baseline/RED/GREEN 结果和 commit 都可核验；测试未运行或失败即 S0 未完成。旧 `/ws` 入口本期静态未改 + 客户端现有测试通过，不宣称已真机验证旧服务。
- S0 不改变 Panel 源、构建或部署，不运行真机服务；单列待 S1 的真实 WS/IPC 接线回执（partial write/EOF、bound enforcement、连接内 ID/关闭、JSON 约束与 TS shape 对照）。S2/S3 再验证 NSH/Web 双端实际渲染，S4 通过才可称恢复完成；没有任何可删除的生产 UI 证明。
- 汇报本地提交、失败或未运行项；满足本期闸门才制定 S1-A 的可执行计划。无开始新阶段、变更生产服务或发布的隐含授权。
