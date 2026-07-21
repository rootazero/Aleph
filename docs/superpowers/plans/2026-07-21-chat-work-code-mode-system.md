# Plan: Chat / Work / Code 模式系统实施

Spec: [2026-07-21-chat-work-code-mode-system.md](../specs/2026-07-21-chat-work-code-mode-system.md)
Worktree: `.claude/worktrees/mode-system` (branch `worktree-mode-system`, base = local main HEAD 230dac34a)

## Phase 1 — 核心枚举与配置

1. 新建 `src/config/types/policies/session_mode.rs`（`exec_tier.rs` 的兄弟）：
   - `SessionMode { Chat, Work, Code }`，`#[default] Work`，serde snake_case；
   - `MODE_SESSION_KEY = "session_mode"`；`from_id` / `id`；`builtin_modes() -> &[ModePreset]`（只发 id，copy 归 Panel）；
   - `prompt_line()`（copy 与规则同文件）；
   - 分区表：`core_tools(&self, default_core: &[String]) -> …` 与 `deferred_prefixes(&self) -> &[&str]`（静态、内容盲、按名/前缀）；
   - 单元测试：id 往返、未知拒绝、默认 Work。
2. `PoliciesConfig.mode: SessionMode`（`#[serde(default)]`）+ TOML 反序列化测试。

## Phase 2 — 三孪生管道

3. `SendParams.mode: Option<String>`（chat.rs）→ `AgentRunParams.mode`（agent.rs，双入口 server_init.rs 同步）；
4. `build_run_request`：`SessionMode::from_id` 校验（未知 fail-loud）→ `RunRequest.metadata[MODE_SESSION_KEY]`（`thinking` 臂旁的第三孪生）；测试镜像 `build_run_request_carries_*` / `_rejects_*`；
5. 新建 `src/gateway/execution_engine/turn_mode.rs`（turn_thinking.rs 模板）：resolve request > session > global、malformed fail-soft、stamp-on-carry best-effort；pinned 优先级测试；`run_loop/inner.rs` 调用（主视图 + subagent 父视图）；
6. `sessions.patch` 校验臂（modify.rs，null 合法）+ `SessionInfo.mode` 投影（query.rs + types.rs）+ 测试。

## Phase 3 — scoped 分区 + prompt line

7. `ScopedToolService.mode` 字段（exec_tier 旁）+ `with_mode` builder setter + `bump_cache_generation` on 变化；
8. 分区落点：
   - deferred：`is_deferred` 合并 mode 的族前缀（`tool_search` 晋升路径不变）；
   - core/schema 折叠：mode 化 core 集传入 ProgressiveDisclosureRewriter 的输入端；
   - 四镜像（list/metadata_schema/describe/dispatchable_list）一致性测试 + 元工具豁免测试；
9. prompt line：`ResolvedContext` 增 mode 字段 → 既有 layer 渲染（approval_tier 模板）；`run_loop/inner.rs` FlowRequest 装配点旁接线。

## Phase 4 — R8 工具

10. `src/builtin_tools/sessions/set_mode_tool.rs`（set_topic_tool.rs 形状）：`session_set_mode(mode)`，写 MODE_SESSION_KEY，注册镜像 session 工具全部注册点。

## Phase 5 — Panel

11. `interfaces/webchat`：`mode_picker.rs`（exec_tier_picker 克隆）+ `mode_labels.rs`；
12. `ChatState.session_mode` 5 位点 + SessionSnapshot；3 发送路径 + `ChatApi::send` 参数；`api/sessions.rs` patch helper；sidebar 重选还原；
13. locales en/zh：`settings.policies.mode_*` 键族；
14. 右侧栏：`state/layout.rs`/events.rs 模式条件默认（chat 抑制 follow 自动开面；work run 开始 inspect Plan；code 现状）。

## Phase 6 — 验证与文档

15. `cargo check --bin aleph-server`；`cargo test` 定向（policies/scoped/turn_*/handlers/sessions 工具）；wasm 构建；
16. `docs/reference/MODE_SYSTEM.md`（设计+锚点+调研引文）；FEATURE_LOCATOR.md 增节；CLAUDE.md 增一行指针（如需）；CHANGELOG 暂不动（随发版写）；
17. 提交、merge --no-ff 回 main；memory 记录。
