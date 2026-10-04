# Subsystem Routing (Read Before Editing)

> **改动任何子系统前先查对应行。** Tier 1 只放指针；详情在这里。
>
> **FL** = [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md)（`FL §x.y` 指它的正文章节）；**E.N** 指它的**附录 E** 判据分组。真机装置的**每个阶段在证明什么**见 [`qa/README.md`](qa/README.md)。

| 你要动的目录 | 先读 | 判据 | 真机 QA |
|---|---|---|---|
| `src/harness/` | [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) · FL §3.1 | E.0 | — |
| `src/thinker/` `src/context/` | FL §2.1 §2.3 §2.18 §2.19 §2.20 | E.1 | — |
| `src/tool_output/` | FL §2.7 §3.14 | E.2 | — |
| `src/tools/` `src/builtin_tools/` | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SECURITY.md](docs/reference/SECURITY.md) · FL §3.2–§3.14 | E.3 | — |
| `src/builtin_tools/file_search/` | FL §3.4 | E.0 E.3 | `qa/file_search/run.sh {floor,page,reach,steer}` · `cargo bench --bench file_search_scan` |
| `src/gateway/` | [GATEWAY.md](docs/reference/GATEWAY.md) · [`src/gateway/CLAUDE.md`](src/gateway/CLAUDE.md) · FL §4.8 §5.6 [§5.14](docs/reference/FEATURE_LOCATOR.md#telegram-channel) §5.18 §5.26 §6.9 | E.4 | `qa/channels/run.sh {reach,errors,approval}` · `qa/resume_boundary/run.sh {claims,denied,rewind,knobs,holes,parked,unanswered,ratchet,parallel,undecodable,attribute,tombstone}` |
| `src/gateway/btw/` | FL §4.14 的机制图 · [SECURITY.md](docs/reference/SECURITY.md) 只读地板 | E.4 | `qa/btw_tui/run.sh {frames,promote}` |
| `src/gateway/session_store/` `session_manager/` | FL §6.9 | E.0 | `qa/session_order/run.sh` |
| `src/gateway/pty/` `interfaces/webchat/.../views/terminal/` | FL §6.11 · 判据清单 §0（分派表的静默 no-op · 有损可观测量） | E.0 | — |
| `src/gateway/runtime/` `crates/agent-detect/` `src/builtin_tools/terminal.rs` | [TERMINAL_RUNTIME.md](docs/reference/TERMINAL_RUNTIME.md) · FL §6.12 | E.4 | `qa/terminal/run.sh {identify,wait,quiet,cwd,real,tui}`（`panel` 要浏览器） |
| `src/gateway/mcp_face/` | [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27 | E.4 E.9 | `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`（每阶段证明什么见 [`qa/README.md`](qa/README.md)） |
| `src/memory/` `src/note/` | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) + memory/ 三分册 · FL §2.5 §2.9 §2.16 | E.5 | `qa/memory_curated/run.sh` |
| `src/providers/` | [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) · FL §3.6 §4.9 | E.9 | — |
| `src/spend/` `src/providers/metering.rs` | FL §5.22（round-7 的 per-principal 美元上限：`[policies.spend]` → `SpendLedger` → 两条执行臂）· FL §5.25（`install_ledger` / `install_policy` 两个进程级句柄——`MeteringProvider` 的生产构造点散在多个模块（普查 `rg "MeteringProvider::new\("`，剥掉测试模块），所以裁决是进程级而非构造参数穿线） | E.0 E.9 | `qa/spend_budget/run.sh`（**需要真 python3**，Windows 主机上是 UNRUN 而不是 PASS——见 [`qa/README.md`](qa/README.md) 该条目） |
| `src/search/` `src/builtin_tools/search.rs` | FL §3.18 | E.3 E.9 | `qa/web_search/run.sh {reach,order,degrade,empty,fanout,demote}`（SearXNG 是唯一能指向 mock 的后端，其余八个由 `providers/capability_census.rs` 在源码级覆盖）|
| `src/browser/` `crates/aleph-cdp/` `src/builtin_tools/browser_tools/` | FL §3.12 | E.9 | `qa/browser_managed/run.sh` · `qa/browser_dual/run.sh`（两个真引擎）——**两套的阶段清单、`ALEPH_QA_DRIVER` 轴、以及每个阶段在证明什么，全见 [`qa/README.md`](qa/README.md)；数目和阶段名都不写在这里** —— ⚠️ 理由要说准：`qa/README.md` **同样**没有任何测试或脚本会让它变红，**搬过去买到的不是「可证伪」，是「一份而不是两份」**（判据 §1），而活下来的那一份就躺在脚本旁边。这张表曾因为自己抄了一个数而漂过一次 |
| `src/mcp/` · `src/hub/` | FL §5.20 §5.24 · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) FL §5.21 | E.9 | `qa/plugins/run.sh` |
| `src/loop_graph/` `src/workflow/` · `src/identity/` | [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) FL §4.12 · [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) FL §5.17 | E.3 E.0 | — |
| `src/config/` `src/diagnostics/` · `src/sandbox/` | FL §5.8 §5.9 §5.10 §5.24 · [SANDBOX.md](docs/reference/SANDBOX.md) FL §3.8 §3.15 | E.8 E.3 | — |
| `src/orchestrator/` | [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) · FL §3.16 · run 的 flow 通道契约（`FlowRequest.event_tx` · `flow_event_channel` · `FlowStreamEvent::Trace`）→ FL §6.13 | E.0 E.4 | — |
| `src/agents/` `src/teams/` · `src/tasks/cron/` `src/tasks/heartbeat/` | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) FL §4.4 §4.5 §4.13a–c（cron/heartbeat 是孪生，共用 `src/tasks/shared/{alert,delivery}.rs`） | E.0 | `qa/teamchat_rooms/run.sh` · `qa/agents_viz/run.sh {claims,panel}`（直播树 = **后台** tracker 的视图，同步 `run` 孩子不发树事件） |
| `desktop/` | [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) · [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) · [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · FL §7.1–§7.4 | E.6 | — |
| `interfaces/webchat/` | [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · FL §4.7 §6.8 §6.9 · [CANVAS.md](docs/reference/CANVAS.md) · FL §6.10 | E.7 | `qa/picker_nav/run.sh` · `cargo test -p alephcore --features test-helpers --test canvas_wire` |
| `src/canvas/` + Panel canvas 视图 | [CANVAS.md](docs/reference/CANVAS.md) · FL §6.10 | E.7 | `qa/canvas/run.sh` |
| `interfaces/tui/` `interfaces/cli/` `shared/protocol/` | FL §5.4 §5.11 §5.13 §5.23 | E.0（跨 crate wire 契约） | `qa/agents_viz/run.sh claims`（无过滤连接 = TUI 的形状；没有 pty，不启动 `aleph-tui`） |
| `shared/ui_logic/` + 呈现侧信道（`protocol/src/{file_change,context_breakdown}.rs` · `exec/masker.rs` · `handlers/{tool_output,context_breakdown}.rs`）+ trace 镜像（`execution_engine/{agent_trace_emit_sink,run_trace_sinks}.rs`） | [TRANSCRIPT_RENDERING.md](docs/reference/TRANSCRIPT_RENDERING.md) · FL §6.13 | E.0 E.1 E.3 E.4 E.7 E.10 | — （Phase B 起 TUI 是 `transcript/` 与 `context.breakdown` 的客户端，但 Phase S 的 `reducer` / `step` / `detail` 在 Phase T 之前零客户端——TR §7.3；**`trace.tool_output` 仍零客户端**）|

> **对照表已做完，别重做**：openclaw · codex · hermes · pi · LangGraph · RouteLLM/LiteLLM/Bifrost · DeepSeek-Reasonix · FluidVoice/WhisperLive · SkillOpt · buzz · deepseek-harness。逐项结论与"刻意不做清单"都在对应 reference 文档里。
