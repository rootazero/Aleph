# Aleph 文档地图 / Documentation Map

> 这份地图回答两个问题：**某件事的"当前状态"写在哪**、**某份旧文档能不能当依据用**。
> 规则一句话：**当前状态以代码为准；文档按"当前层"与"证据层"分开存放，两层不可互相冒充。**

## 三层结构

| 层 | 位置 | 语义 | 能不能当"现状依据" |
|---|---|---|---|
| **L1 入口层** | `AGENTS.md` · `CLAUDE.md` · `README.md` · `README_CN.md` · `CONTRIBUTING.md` · `skills/architecture/SKILL.md` | 快速上手与红线摘要 | 能，但只到"摘要"精度 |
| **L2 当前层（reference）** | `docs/reference/`（74 份） | 当前架构、契约、运维的完整描述 | **能**——这是查现状的唯一文档入口 |
| **L3 证据层（point-in-time）** | `docs/superpowers/` · `docs/reports/` · `docs/engineering-reports/` · `docs/plans/` · `docs/research/` · `docs/audits/` · `docs/archive/` · `docs/brainstorms/` · `docs/designs/` · `docs/specs/` · `docs/implementation/` · `docs/testing/` | 某一天的设计、审计、验收、运行记录 | **不能**——只说明"当时" |

L3 的完整契约见 [`docs/reference/EVIDENCE_LAYER.md`](reference/EVIDENCE_LAYER.md)。

## L2：`docs/reference/` 分区

| 分区 | 文件（示例） | 回答什么问题 |
|---|---|---|
| 总览与红线 | `ARCHITECTURE.md` · `REDLINES.md` · `DESIGN_PATTERNS.md` · `CODE_ORGANIZATION.md` · `DOMAIN_MODELING.md` · `GLOSSARY.md` | 系统怎么分层、哪些不能做、术语是什么 |
| 桌面与客户端 | `DESKTOP_SHELL.md` · `DESKTOP_BRIDGE.md` · `PRODUCT_TOPOLOGY.md` · `BROWSER_LIVE_VIEWER.md` · `WINDOWS_RUNTIME.md` · `LINUX_DESKTOP.md` · `CANVAS.md` | 桌面宿主、Panel、画布怎么跑 |
| 运行与内核 | `HARNESS_PHILOSOPHY.md` · `AGENT_SYSTEM.md` · `AGENT_IDENTITY.md` · `AGENT_LOOP_TOOL_EXECUTION.md` · `AGENT_LOOP_CONTEXT_BUDGET.md` · `MULTI_AGENT_SYSTEM.md` · `SESSION_KNOBS.md` · `SESSION_SERVICE.md` · `BOOT_ASSEMBLY.md` | 大脑怎么思考、循环怎么转、会话怎么存 |
| 能力与工具 | `TOOL_SYSTEM.md` · `PLUGIN_SYSTEM.md` · `EXTENSION_SYSTEM.md` · `SKILL_MODEL_TAXONOMY.md` · `MULTI_AGENT_SYSTEM.md` · `GRAPH_LAYER.md` | 能力怎么注册、扩展怎么装载 |
| 网关与通道 | `GATEWAY.md` · [A2A.md](reference/A2A.md) · `SANDBOX.md` · `SECURITY.md` · `CLUSTER.md` · `MODEL_CATALOG.md` · `MODEL_PERCEIVABLE_ECOSYSTEM.md` | 消息怎么进来、模型怎么选、权限怎么管 |
| 记忆与状态 | `MEMORY_SYSTEM.md` · `docs/reference/memory/`（`NOTES.md` · `RETRIEVAL.md` · `DREAM_DAEMON.md` · `RAW_MEMORY.md`） | 记忆怎么存、怎么召回 |
| 特性定位 | `FEATURE_LOCATOR.md`（6611 行，功能→实现位置的索引）· `FEATURE_LOCATOR_BACKLOG.md` | "这个功能在哪实现 / 还没实现" |
| 运维与开发 | `DEVELOPMENT.md` · `SERVER_DEVELOPMENT.md` · `RELEASE.md` · `PROCESS_MANAGEMENT.md` · `WORKSPACE.md` · `SUBSYSTEM_ROUTING.md` · `TOOL_USAGE.md` | 怎么构建、怎么发版、怎么定位子系统 |
| 审计与终局评估 | `TWELVE_FACTOR_AUDIT.md` · `FEATURE_LOCATOR.md` · `PHASE_6B_BUILDER_AUDIT.md` | 12-factor 落地程度、历史审计结论 |

**查现状的顺序**：`docs/reference/FEATURE_LOCATOR.md` 定位 → 对应 subsystem 文档读契约 → 打开代码。文档与代码冲突时**以代码为准**，并顺手修文档（见下）。

## L3：证据层分区

| 目录 | 份数 | 里面是什么 |
|---|---|---|
| `docs/superpowers/` | ~1982 | 计划（`plans/`）、规格（`specs/`）、运行回执（`runs/*.json`）、笔记（`notes/`）；证据密度最高的一层 |
| `docs/engineering-reports/` | 117 | 工程审计报告、阶段交付报告 |
| `docs/reports/` | 51 | 验证报告、故障根因分析 |
| `docs/plans/` | 43 | 早期设计/实施计划 |
| `docs/research/` | 38 | 前置调研与阶段规格 |
| `docs/brainstorms/` | 15 | 需求脑暴（未决 / 已废） |
| `docs/guides/` | 13 | 面向使用者的操作指南（`agents.md` · `channels.md` · `cluster.md` · `cron.md` …） |
| `docs/implementation/` | 11 | 阶段完成状态汇总 |
| `docs/audits/` | 11 | 线缆审计、admission 审计 |
| `docs/designs/` · `docs/specs/` | 5 · 2 | 设计稿与规格 |
| `docs/archive/` | 3 | 已归档的早期设计 |
| `docs/testing/` | 2 | 阶段测试报告 |
| `docs/agents/` | 4 | Agent 运行守则（自成一体的操作约定） |

L3 文件名通常带日期（`2026-10-15-rightpane-stage1-slot-spec.md`），**日期即快照时点**。

## 保持文档可用的三条动作

1. **改代码时同步改 L2**。契约、路径、默认值变了，先改 `docs/reference/` 里对应那份，再改代码——顺序反过来就会留下过期描述。
2. **历史结论加横幅，不要删**。L2 里也混着时点文档（文件名带日期的那几份），已在开头标注"历史文档 / point-in-time"，当前状态指向代码。
3. **守卫脚本盯住漂移**。`scripts/doc_wiring_audit.py`（`just check-doc-wiring`，已并入 `just check-wiring`）检查：入口层必须指向当前桌面宿主 `desktop/nsh-shell`、L1 的 AGENTS/CLAUDE 除登记过的差异章节外必须一致、L2 里点名的路径和代码符号必须真实存在（已知失效项记在 `scripts/doc_wiring_audit_baseline.json`，新增失效才算失败）、`just` 目标必须存在。新写的文档跑一次 `python3 scripts/doc_wiring_audit.py`。

## 🗂 全部 reference 文档（完整清单）

共 76 份（含 `docs/reference/memory/` 子目录）。点进去之前先看 [EVIDENCE_LAYER.md](reference/EVIDENCE_LAYER.md) 判断它是当前层还是时点文档。

- [2026-03-20-cli-tui-separation-design.md](reference/2026-03-20-cli-tui-separation-design.md)
- [2026-03-21-hierarchical-slash-commands-design.md](reference/2026-03-21-hierarchical-slash-commands-design.md)
- [2026-04-10-self-growth-redesign-plan.md](reference/2026-04-10-self-growth-redesign-plan.md)
- [2026-04-10-self-growth-redesign.md](reference/2026-04-10-self-growth-redesign.md)
- [AGENT_DESIGN_PHILOSOPHY.md](reference/AGENT_DESIGN_PHILOSOPHY.md)
- [AGENT_IDENTITY.md](reference/AGENT_IDENTITY.md)
- [AGENT_LOOP_CONTEXT_BUDGET.md](reference/AGENT_LOOP_CONTEXT_BUDGET.md)
- [AGENT_LOOP_RECOVERY.md](reference/AGENT_LOOP_RECOVERY.md)
- [AGENT_LOOP_TOOL_EXECUTION.md](reference/AGENT_LOOP_TOOL_EXECUTION.md)
- [AGENT_SYSTEM.md](reference/AGENT_SYSTEM.md)
- [ALEPH_HUB.md](reference/ALEPH_HUB.md)
- [ARCHITECTURE.md](reference/ARCHITECTURE.md)
- [BOOT_ASSEMBLY.md](reference/BOOT_ASSEMBLY.md)
- [BROWSER_LIVE_VIEWER.md](reference/BROWSER_LIVE_VIEWER.md)
- [CANVAS.md](reference/CANVAS.md)
- [CLAW_CODE_GAP_ANALYSIS.md](reference/CLAW_CODE_GAP_ANALYSIS.md)
- [CLI_BACKEND_GAPS.md](reference/CLI_BACKEND_GAPS.md)
- [CLUSTER.md](reference/CLUSTER.md)
- [CODE_ORGANIZATION.md](reference/CODE_ORGANIZATION.md)
- [CONTEXT_PLUGINS.md](reference/CONTEXT_PLUGINS.md)
- [DESIGN_PATTERNS.md](reference/DESIGN_PATTERNS.md)
- [DESKTOP_BRIDGE.md](reference/DESKTOP_BRIDGE.md)
- [DESKTOP_SHELL.md](reference/DESKTOP_SHELL.md)
- [DEVELOPMENT.md](reference/DEVELOPMENT.md)
- [DOMAIN_MODELING.md](reference/DOMAIN_MODELING.md)
- [EVIDENCE_LAYER.md](reference/EVIDENCE_LAYER.md)
- [EXTENSION_SYSTEM.md](reference/EXTENSION_SYSTEM.md)
- [FEATURE_LOCATOR.md](reference/FEATURE_LOCATOR.md)
- [FEATURE_LOCATOR_BACKLOG.md](reference/FEATURE_LOCATOR_BACKLOG.md)
- [A2A.md](reference/A2A.md)
- [GATEWAY.md](reference/GATEWAY.md)
- [GLOSSARY.md](reference/GLOSSARY.md)
- [GOOGLE_MEET_BRIDGE.md](reference/GOOGLE_MEET_BRIDGE.md)
- [GRAPH_LAYER.md](reference/GRAPH_LAYER.md)
- [HARNESS_PHILOSOPHY.md](reference/HARNESS_PHILOSOPHY.md)
- [LINUX_DESKTOP.md](reference/LINUX_DESKTOP.md)
- [MARKDOWN_SKILL_AUTHORING.md](reference/MARKDOWN_SKILL_AUTHORING.md)
- [MEMORY_SYSTEM.md](reference/MEMORY_SYSTEM.md)
- [MODEL_CATALOG.md](reference/MODEL_CATALOG.md)
- [MODEL_PERCEIVABLE_ECOSYSTEM.md](reference/MODEL_PERCEIVABLE_ECOSYSTEM.md)
- [MODEL_PERCEIVABLE_ECOSYSTEM_PLAN.md](reference/MODEL_PERCEIVABLE_ECOSYSTEM_PLAN.md)
- [MODE_SYSTEM.md](reference/MODE_SYSTEM.md)
- [MULTI_AGENT_SYSTEM.md](reference/MULTI_AGENT_SYSTEM.md)
- [PHASE_6B_BUILDER_AUDIT.md](reference/PHASE_6B_BUILDER_AUDIT.md)
- [PLUGIN_SYSTEM.md](reference/PLUGIN_SYSTEM.md)
- [PROCESS_MANAGEMENT.md](reference/PROCESS_MANAGEMENT.md)
- [PRODUCT_TOPOLOGY.md](reference/PRODUCT_TOPOLOGY.md)
- [REDLINES.md](reference/REDLINES.md)
- [RELEASE.md](reference/RELEASE.md)
- [SANDBOX.md](reference/SANDBOX.md)
- [SECURITY.md](reference/SECURITY.md)
- [SERVER_DEVELOPMENT.md](reference/SERVER_DEVELOPMENT.md)
- [SESSION_KNOBS.md](reference/SESSION_KNOBS.md)
- [SESSION_SERVICE.md](reference/SESSION_SERVICE.md)
- [SKILL_MODEL_TAXONOMY.md](reference/SKILL_MODEL_TAXONOMY.md)
- [SKILL_TRIGGER_ENHANCEMENT.md](reference/SKILL_TRIGGER_ENHANCEMENT.md)
- [SKILL_TRIGGER_ENHANCEMENT_PLAN.md](reference/SKILL_TRIGGER_ENHANCEMENT_PLAN.md)
- [STATE_LAYER.md](reference/STATE_LAYER.md)
- [STYLE.md](reference/STYLE.md)
- [SUBSYSTEM_ROUTING.md](reference/SUBSYSTEM_ROUTING.md)
- [TERMINAL_RUNTIME.md](reference/TERMINAL_RUNTIME.md)
- [TOOL_SYSTEM.md](reference/TOOL_SYSTEM.md)
- [TRANSCRIPT_RENDERING.md](reference/TRANSCRIPT_RENDERING.md)
- [TUI_THEMES.md](reference/TUI_THEMES.md)
- [TWELVE_FACTOR_AUDIT.md](reference/TWELVE_FACTOR_AUDIT.md)
- [WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md](reference/WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md)
- [WHATSAPP_ARCHITECTURE_DESIGN.md](reference/WHATSAPP_ARCHITECTURE_DESIGN.md)
- [WINDOWS_RUNTIME.md](reference/WINDOWS_RUNTIME.md)
- [WORKFLOW_INTEROP.md](reference/WORKFLOW_INTEROP.md)
- [WORKFLOW_TEMPLATES.md](reference/WORKFLOW_TEMPLATES.md)
- [WORKSPACE.md](reference/WORKSPACE.md)
- [DREAM_DAEMON.md](reference/memory/DREAM_DAEMON.md)
- [EXTENSIONS.md](reference/memory/EXTENSIONS.md)
- [NOTES.md](reference/memory/NOTES.md)
- [RAW_MEMORY.md](reference/memory/RAW_MEMORY.md)
- [RETRIEVAL.md](reference/memory/RETRIEVAL.md)

## 入口

- 新人上手：[`README.md`](../README.md) · [`README_CN.md`](../README_CN.md)
- 宪法与红线：[`AGENTS.md`](../AGENTS.md) · [`CLAUDE.md`](../CLAUDE.md)
- 当前架构总览：[`docs/reference/ARCHITECTURE.md`](reference/ARCHITECTURE.md)
- 证据层契约：[`docs/reference/EVIDENCE_LAYER.md`](reference/EVIDENCE_LAYER.md)
