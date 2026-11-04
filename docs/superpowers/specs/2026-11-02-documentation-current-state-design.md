# 文档现状校准设计 / Documentation Current-State Reconciliation Design

**日期 / Date:** 2026-11-02
**范围 / Scope:** `FEATURE_LOCATOR.md` 与四份直接相关 reference 文档
**状态 / Status:** 已批准并实施 / Approved and implemented

## 意图 / Intent

把 Aleph 当前的 capability、provider lifecycle、MemoryRuntime 与 deferred 边界写成一个稳定的现状入口，避免把研究方案、历史轮次和当前生产接线混读。此次工作服务于维护者、审查者和后续 agent：他们应能从 `FEATURE_LOCATOR.md` 先找到当前权威，再沿链接进入拥有该事实的 reference 文档或源码。

Make the current capability, provider-lifecycle, MemoryRuntime, and deferred boundaries easy to locate without conflating research proposals, historical rounds, and current production wiring. The entry point is for maintainers, reviewers, and future agents; it should lead to the owning reference or source evidence.

## 目标 / Goals

- 在 `docs/reference/FEATURE_LOCATOR.md` 顶部增加一个 current-state entry，覆盖 Tool capability projection、provider lifecycle、MemoryRuntime、provider/cache 语义和明确 deferred 项。
- 修正与当前边界直接相关的 `docs/reference/MEMORY_SYSTEM.md`、`PLUGIN_SYSTEM.md`、`ARCHITECTURE.md`、`TOOL_SYSTEM.md` 表述，使单一权威、projection-only、生命周期与可见性分离、raw Memory Off 语义一致。
- 保留历史轮次作为证据索引，但明确历史段落不能覆盖 current-state entry。
- 使用源码锚点和已有 run/research 文档链接，不把测试结果或研究方案写成生产接线。
- 保持双语文档惯例；英文和中文描述表达同一当前事实。

## 非目标 / Non-goals

- 不逐条重写 6374 行 `FEATURE_LOCATOR.md` 历史词典。
- 不修改生产代码、Capability 契约、provider lifecycle、MemoryRuntime 或生成器。
- 不把所有 externally composable 子系统声称已经接入 Zahir；当前 Tool/Plugin、Gateway bounded read/lease、Terminal、Memory 仍分别保持各自 authority。
- 不把 MemoryRuntime 写成 universal scheduler、所有 Memory writer 的停止闸、进程隔离、native unload、RSS 回收或 exactly-once 外部效果。
- 不将 deepseek-harness 的 Cordis 机制复制进 Aleph；它只作为文档结构和 capability seam 的比较材料。

## 约束 / Constraints

- 文档必须描述当前源码事实；设计和历史证据放在 research、spec、run 或历史小节。
- 每个事实保留一个主要归属点，其他文档通过链接引用。
- `ToolHandlerRegistry`、`OwnershipTree`、`ProviderHandle`、`ExtensionManager`、`ChannelRegistry`、`MemoryRuntime` 等 authority 不得在文档中互相混称。
- 当前 provider lifecycle 的 production wiring 只写 bounded tool-only WASM；Feishu → Zahir、MCP lazy、其他 Capability kind、EffectClaim producer、Safe Replay、universal scheduler 和 external exactly-once 保持 deferred/NO-GO。
- 文档修改不触发 Cargo；使用 Markdown link、diff、结构化脚本和现有文档检查验证。

## 方案 / Approaches

### 方案 A：有限校准（采用） / Bounded reconciliation (selected)

在 `FEATURE_LOCATOR.md` 增加 current-state entry，并同步四份直接相关 reference 文档的短段落或链接。历史条目继续保留，但当前入口声明覆盖关系。此方案最小化历史证据重写和锚点漂移，同时给当前架构判断提供唯一入口。

Add a current-state entry to `FEATURE_LOCATOR.md` and synchronize short sections or links in the four directly related reference documents. Historical entries remain, but the current entry states which text is authoritative. This minimizes historical rewrite and anchor drift while providing one current entry for architecture decisions.

### 方案 B：全表重写 / Full dictionary rewrite

逐条重新核对并改写整个功能词典。它能最大化表面一致性，但会混合历史证据与当前契约，且扩大无法一次验证的范围，因此不采用。

### 方案 C：独立现状文档 / Separate current-state document

新增一份现状表并只在 `FEATURE_LOCATOR.md` 顶部链接。它隔离了旧文档风险，但会产生两个维护入口，因此不采用。

## 计划中的文档形状 / Planned document shape

`FEATURE_LOCATOR.md` 的 current-state entry 将按以下顺序组织：

1. 单一权威与 projection 规则；
2. Tool/Plugin capability 当前 production slice；
3. provider lifecycle 与 `ExtensionManager` / `RuntimeOwner` 的边界；
4. MemoryRuntime 的 derived-resource、raw archival、admission 和 stop 语义；
5. Gateway/Terminal/Memory/Feishu 等仍独立或 deferred 的边界；
6. 当前事实的源码、reference、run-record 链接。

四份 reference 文档只保留各自拥有的事实：`MEMORY_SYSTEM.md` 拥有 MemoryRuntime 语义，`PLUGIN_SYSTEM.md` 拥有插件 discovery/activation，`ARCHITECTURE.md` 拥有跨子系统分层和 capability plane，`TOOL_SYSTEM.md` 拥有 callable Tool registry/dispatch。重复内容改为短链接或一致的边界说明。

## 待实现 / Remaining work

- Documentation changes described here have been implemented in the current-state locator and four owning reference documents.
- Structural, local-target, bilingual-entry, and `git diff --check` validation has passed; the final fresh review and run-record update remain part of the delivery gate.
- Cargo, native execution, production runtime tests, and full workspace checks are intentionally not run because this phase changes documentation only.
