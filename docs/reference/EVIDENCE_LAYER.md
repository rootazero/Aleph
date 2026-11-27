# 证据层契约（Evidence Layer Contract）

> **一句话**：Aleph 的文档分三层，**当前层说"现在是什么"，证据层说"当时发生了什么"**。
> 两者冲突时，**以代码为准**；证据层永远不能被当成现状引用。
> 文档地图见 [`docs/README.md`](../README.md)。

## 为什么需要这个契约

Aleph 在 2026 年经历了三次大改：CLI/TUI 分离、Webchat 从 TypeScript 全面改写为 Rust/Leptos、桌面宿主从 Tauri 迁到 NSH。每次改完，**旧文档不会自己失效**——它还躺在仓库里，还带着完整的命令、路径和结论。

一次真实的踩坑（本轮校对的起因）：`AGENTS.md` 与 `CLAUDE.md` 的技术栈行仍把桌面宿主写成 Tauri 独占，而实际桌面宿主已经进入 Tauri / NSH 双轨；更糟的是，**入口层是所有 agent 的第一份读物**——它错了，后面所有基于它的推理都跟着错。

如果只有一句规则，读者无法判断"我手上这份文档能不能用"。所以需要一份显式契约：把"当前层"和"证据层"在目录层面分开，并给每个证据文件标出快照时点。

## 三层定义

| 层 | 目录 | 语义 | 引用规则 |
|---|---|---|---|
| L1 入口层 | `AGENTS.md` `CLAUDE.md` `README.md` `README_CN.md` `CONTRIBUTING.md` `skills/architecture/SKILL.md` | 摘要与红线 | 可引用，但只到摘要精度；出现分歧时以下一层为准 |
| L2 当前层 | `docs/reference/` | 当前架构、契约、运维 | **现状的唯一文档来源**；与代码冲突时改文档或改代码，二选一，不留分歧 |
| L3 证据层 | `docs/superpowers/` `docs/reports/` `docs/engineering-reports/` `docs/plans/` `docs/research/` `docs/audits/` `docs/archive/` `docs/brainstorms/` `docs/designs/` `docs/specs/` `docs/implementation/` `docs/testing/` | 时点记录 | **不可作为现状引用**；只能引用为"当时我们这么决定/这么测过" |

L2 里也混着少量时点文档（文件名带日期的：例如 `docs/reference/2026-03-20-cli-tui-separation-design.md`）。这类文件在开头带**历史横幅**，并指向当前代码位置。

## 证据层的判定标准

一个文件属于证据层，当且仅当它满足下面任意一条：

1. 文件名带日期前缀（`YYYY-MM-DD-…`）或带日期后缀（`…-2026-04-06.md`）。
2. 开头有 `Status: Draft` / `状态：历史文档` / `Date:` / `**Status**:` 之类的时点字段。
3. 记录的是一次运行、一次审计、一次验收的结果（`docs/reports/`、`docs/superpowers/runs/`）。
4. 描述的是一份尚未实施或已被放弃的设计。

反过来讲：**当前层文件不得包含"待实施""草案"状态**。如果一份 `docs/reference/` 文档写的是"计划新增 X"，那它属于证据层，应移到 `docs/plans/` 或加历史横幅。

## 阅读证据层的正确姿势

```
读到一份证据文档
  │
  ├─ 它回答的是"当时"的问题吗？── 是 ──▶ 用它解释历史决策，不要照抄它的结论
  │
  └─ 它描述的能力/路径/默认值现在还在吗？
        │
        ├─ 去代码里核对：grep 关键符号 / 看 Cargo.toml / 看真实路径
        │     （例：文档说 src/gateway/interfaces/whatsapp/，实际运行时在 src/gateway/interfaces/whatsapp/wa_runtime/）
        │
        └─ 代码里已经变了 ──▶ 结论作废，改引 L2 或代码
```

**当前状态的三条取数路径**（按精度排序）：

1. `docs/reference/FEATURE_LOCATOR.md` — 功能 → 实现位置索引（6611 行，按模块分组）。
2. `docs/reference/SUBSYSTEM_ROUTING.md` — 子系统 → 该读哪份 reference 文档。
3. 代码本身 — 契约、默认值、真实路径以代码为准。

## 写作规范

### 在 L2 里引用证据

可以，但要带时点与出处：

```markdown
Phase 11 的门禁定义见 `docs/superpowers/plans/2026-11-21-phase11-nsh-cutover.md`（2026-11-21 快照）；
当前门禁状态以 `docs/reference/DESKTOP_SHELL.md` 与代码为准。
```

不要这样写（读者无法判断时效）：

```markdown
详见 docs/superpowers/plans/2026-11-21-phase11-nsh-cutover.md。
```

### 在 L3 里标注快照时点

证据文件开头固定给一行横幅（推荐统一措辞）：

```markdown
> **状态：历史文档（point-in-time）** — 本文记录的是 YYYY-MM-DD 当时的设计/结论，不代表当前架构。
> 当前状态以代码为准（<关键位置>）。
```

L2 里带日期的那几份已按此格式标注。

### 不要删除历史结论

证据层保留**内容**（结论、运行回执、失败原因），只改**表述**。删掉会让下一次重构重复踩坑；保留但标注时点，才既保留信息又不误导。

## 守卫

`scripts/doc_wiring_audit.py`（`just check-doc-wiring`，已并入 `just check-wiring`）把这些规则变成可执行检查：

| 检查 | 规则 |
|---|---|
| `HOST` | 入口层文档指向当前桌面宿主 `desktop/nsh-shell` / Entry docs point to the production desktop host |
| `SYNC` | `AGENTS.md` 与 `CLAUDE.md` 归一化后必须一致，只允许 `SYNC_ALLOWED_DIFFS` 列出的章节不同 —— 两份宪法不能各说一套 |
| `STALE` | 禁止单轨断言（把桌面宿主整体说成"只有 Tauri"的写法；具体字面量见 `scripts/doc_wiring_audit.py` 的 `CUT_PHRASES`） |
| `PATHS` | 文档里点名的仓库路径必须真实存在（带 baseline 棘轮：新出现的失效路径算失败） |
| `SYMBOLS` | 反引号里形似代码的符号（`a::b`、`f()`、带下划线的小写名、多驼峰名）必须在受跟踪源码中出现；标题含 Historical / Superseded / 历史记录 的章节豁免（同样是 baseline 棘轮） |
| `LINKS` | 本地 Markdown 链接必须可解析 |
| `CRATES` / `JUST` / `PORT` | 点名的 crate、`just` 目标、端口号必须与代码一致 |
| `MANGLED` / `ROOTS` | 路径拼写损坏（文件扩展名后粘上多余片段）与"缺 `src/` 前缀"要能被发现 |

证据层目录**不进入**上述检查的失败集合（它们的路径本来就是历史快照），只在 L1/L2 上强制。

## 相关

- 文档地图：[`docs/README.md`](../README.md)
- 当前架构：[`docs/reference/ARCHITECTURE.md`](ARCHITECTURE.md)
- 桌面宿主现状 / Desktop host：[`docs/reference/DESKTOP_SHELL.md`](DESKTOP_SHELL.md) · [`docs/reference/PRODUCT_TOPOLOGY.md`](PRODUCT_TOPOLOGY.md)
- 本轮校对计划：`docs/superpowers/plans/2026-11-23-doc-architecture-realignment.md`
