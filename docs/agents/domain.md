# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root, or
- **`CONTEXT-MAP.md`** at the repo root if it exists — it points at one `CONTEXT.md` per context. Read each one relevant to the topic.
- **`docs/adr/`** — read ADRs that touch the area you're about to work in. In multi-context repos, also check `src/<context>/docs/adr/` for context-scoped decisions.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The producer skill (`/grill-with-docs`) creates them lazily when terms or decisions actually get resolved.

## File structure

This repo is **single-context**:

```
/
├── CONTEXT.md          ← 尚未创建（预期状态）
├── docs/adr/           ← 尚未创建（预期状态）
└── src/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_

---

## Aleph 的既有真源（本仓库补充）

这个仓库在本技能安装前就已有领域文档。**它们是真源，不要复述进 `CONTEXT.md`：**

| 你想找 | 去哪儿 |
|---|---|
| 领域词汇 / 术语 | `docs/reference/GLOSSARY.md` · `docs/reference/DOMAIN_MODELING.md` |
| 「这个功能现在长什么样」 | `docs/reference/FEATURE_LOCATOR.md`（reference 全库总索引，按 §编号组织） |
| 架构红线 R1–R10 / 设计原则 P1–P8 | 根 `CLAUDE.md`（Tier-1） |
| 改某目录前必读什么 | 根 `CLAUDE.md` 的「📍 子系统路由」表 |
| 工程判据（踩过的坑）与验证纪律 | `FEATURE_LOCATOR.md` 附录 C（验证纪律）/ D（判据全文）/ E（触发器，§0–§10） |
| 子系统级上下文 | `src/harness/CLAUDE.md` · `src/gateway/CLAUDE.md` |

⚠️ **判据 §1「同一事实的两份表述」**：往 `CONTEXT.md` 里抄一份 GLOSSARY 的词条，改一份就是静默说谎。`CONTEXT.md` 只收 GLOSSARY 里**没有**的新术语，或干脆写成指向 GLOSSARY 的指针。

⚠️ 同理，本仓库**没有**采用 `CONTEXT-MAP.md`：根 `CLAUDE.md` 的「子系统路由」表已经在干那件事，再建一张就是它的第二份表述。

将来 `/grill-with-docs` 落地一个架构决策时，新 ADR 写进 `docs/adr/`，**不要**在 `docs/reference/` 里另起第二套决策记录。
