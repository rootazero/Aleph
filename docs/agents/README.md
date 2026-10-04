# Agent Skills (Claude Code Plugin)

> **消费者**＝ `mattpocock-skills` plugin（`/to-issues` `/triage` `/to-prd` `/diagnose` `/tdd` `/grill-with-docs` `/code-review` 等）。它**不随本仓库分发**——代码装在各机器的 `~/.claude/plugins/`，启用声明在仓库的 `.claude/settings.json` → `enabledPlugins`。**新机器若未安装，下面三份配置零消费者、静默不生效**（判据 §7），跑 `/plugin` 装上即可。

## Index

| 主题 | 文档 |
|------|------|
| Issue tracker（GitHub via `gh` CLI） | [`docs/agents/issue-tracker.md`](issue-tracker.md) |
| Triage labels（5 个规范角色） | [`docs/agents/triage-labels.md`](triage-labels.md) |
| Domain docs（领域真源在 `docs/reference/`） | [`docs/agents/domain.md`](domain.md) |

## Issue tracker（要点）

Issues 在 GitHub（`rootazero/Aleph`），经 `gh` CLI 读写。详情 → [`docs/agents/issue-tracker.md`](issue-tracker.md)。

## Triage labels（要点）

五个规范角色直接用同名标签字符串（`needs-triage` / `needs-info` / `ready-for-agent` / `ready-for-human` / `wontfix`）；除 `wontfix` 外远端尚未创建，首次用到时 `gh label create`。详情 → [`docs/agents/triage-labels.md`](triage-labels.md)。

## Domain docs（要点）

Single-context 布局。领域真源是 `docs/reference/GLOSSARY.md` + [FEATURE_LOCATOR.md](../reference/FEATURE_LOCATOR.md)；`CONTEXT.md` 与 `docs/adr/` **尚未创建且这是预期状态**，由 `/grill-with-docs` 懒创建。详情 → [`docs/agents/domain.md`](domain.md)。
