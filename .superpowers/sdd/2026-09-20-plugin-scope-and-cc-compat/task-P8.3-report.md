# Task P8.3 report — PLUGIN_SYSTEM.md doc updates

Status: DONE (committed; commit hash see `git log -1`).
Worktree: `plugin-scope-round` @ `f7d1cbce3` (clean before; clean after).

## Files actually modified

Only `docs/reference/PLUGIN_SYSTEM.md`. No code, no harness, no config.

## Plan line numbers were stale (worked around by semantic anchors)

Plan P5P8-cut-and-docs.md targets `:149-171, 173-179, 334-350, 353-383, 570-578,
672, 683-706, 708, 719, 790-820`. On this commit those line numbers map to
different lines because P1/P2/P3/P4 code changes have grown the file. Per user
instruction I located every target by **semantic heading / anchor text**:

| Plan anchor | Semantic target | Location used |
|---|---|---|
| `:149-171` 插件状态 | `## 插件状态（plugins.list 的 status）` | heading match |
| `:173-179` Runtime 模型 | `## Runtime 模型` + 3-row table + `---` | anchor match |
| `:334-350` Scope 管理 | `## Scope 管理` (entire section) | heading match |
| `:353-383` 安装第三方 Claude Code 插件 | `## 安装第三方 Claude Code 插件` + table | heading match |
| `:570-578` MCP Runtime Wiring | `## MCP Runtime Wiring（已完成）` bullets | heading + bullet match |
| `:672` Manifest cache heading | `## Manifest 解析缓存（openclaw parity）` | heading match |
| `:688` `:699` Lazy Activation history | history block sentences | anchor match |
| `:708` Owner Trust heading | `## Owner Trust Policy（P3.5 — openclaw parity）` | heading match |
| `:719` Owner Trust cross-ref | "这对应 openclaw 的 …" sentence | anchor match |
| `:790-820` 进程级投影 | `## 进程级投影的单一咽喉（projection.rs）` | heading match |

All replacements used unique multi-line strings; `replace_once` asserts count == 1.
First (b) pass mistakenly replaced the `| "wasm" |` row by anchoring on the
trailing `---`; `git checkout` + script rewrite anchored on `wasm row + blank +
---` and **kept** the wasm row. Post-fix `git diff` confirms the wasm row is in
both old and new.

## Item-by-item

### (a) 插件状态
- `loaded` 含义: + `（PluginStatus::Loaded）`
- `pending` row **moved** to 2nd (loaded/pending/disabled/error/blocked), 含义
  rewritten as `PluginStatus::Pending { waiting_on }` + `mcp:manager` /
  `mcp:<server_id>` + 不因超时变 error rule (判据 §8)
- `disabled` 含义: `PluginStatus::Disabled` + cross-ref to (b) ClaudeCache
  paragraph. **Deviation**: kept existing 补救 cell content (Panel /
  `plugin_manage` 拒绝 model enable / claude_cache 行只能禁用) — plan's
  shortened `aleph plugin enable <name>` would drop that integration context.
- `error` 含义: `PluginStatus::Error("<step>: <reason>")` + `lifecycle.rs::write_failed_row`
  as single writer + 全有或全无 rule; preserved MCP-server-start-failure branch
- `blocked` 含义: `PluginStatus::Blocked(reason)`
- Blockquote (P3.1's text, with `overridden 从未有过生产者 / 于 2026-09 删除`
  history) **unchanged** + one appended sentence:
  `> activation_gate 认得的终态集合从枚举派生（守卫 G6），不是手写清单；Pending 计入 is_active()。`

### (b) Runtime 模型 — ClaudeCache paragraph appended (3-row table preserved)
Plan text appended after the `wasm` row, before `---`. Table intact.

### (c) Scope 管理 — section rewritten
6-row `ScopeKey` table (agent-level/local/project/user/claude-cache/bundled) +
新 `可见性` paragraph (ScopeKey / visible_to / VisibilityCtx / 5 face / hooks
project_scope_allows refactored) + fail-closed 行为变更 note + bash block
(`al` → `aleph` per plan).

### (d) 安装第三方 Claude Code 插件 — table + DEVIATION/CONNECT
- agents row: `frontmatter → AgentDef，正文 → AgentDef.system_prompt（2026-09-20 起）`,
  permissionMode 解析不应用 (DEVIATION 3), color 忽略
- commands row: short `SkillTemplate` / `allowed-tools` / `argument-hint` /
  `disable-model-invocation` cell per plan text. **Deviation**: detail from old
  long cell (`!cmd` 同意清单 / 沙箱外 / env inheritance) not repeated in this
  row — those semantics live at length in the `环境变量` section below
- `hooks/hooks.json` `timeout` row inserted: CC 600 s / Aleph 300 s default +
  cap, `MAX_HOOK_TIMEOUT_SECS`, 180 s tool budget, 钳 + warn
- DEVIATION block (5 items per R5.3): timeout · skills CC-only fields · agent
  permissionMode · `~/.claude/settings.json` not read · `~/.aleph/hooks.json`
- CONNECT block (5 items per U-b): exit-2=block + updatedInput + hook_event_name
  spelling (U-b) + `CC_TOOL_ALIASES` matcher + permission_mode from ExecTier +
  allowed-tools 双语义 + no-project-only-Global cross-ref to (c)

### (e) MCP Runtime Wiring — replaced 注册编排 + 卸载清理 bullets
- 注册编排 → `mount(id)` calls `add_transient_server`, `Disposer` stored in
  `EffectScope` (step `"mcp_server"`), `unmount(id)` reverse-dispose = `remove_transient_server`.
  Notes 旧 `set_plugin_enabled(true)` asymmetry deleted.
- 卸载清理 → no longer "capture server ids and tear down"; server id lives in
  disposer closure, dispose IS teardown.
- Other two bullets (`transient 通道`, `list_servers`) untouched.

### (f) Manifest 解析缓存
Heading: `## Manifest 解析缓存（openclaw parity）` → `## Manifest 解析缓存`.
Sentence: openclaw reference → "key 里带 dev/ino 是为了对抗硬链接替换
（canonicalize 关不掉硬链接——附录 E.3）".

### (g) Lazy Activation Planner history
Two `openclaw activation-planner.ts` references inside the history block
rephrased as "参考实现的 activation planner". Heading + surrounding paragraphs
untouched (per do-not-disturb directive on other tasks' legitimate paragraphs).

### (h) Owner Trust Policy
Heading: `## Owner Trust Policy（P3.5 — openclaw parity）` → `## Owner Trust
Policy（P3.5）`. Sentence: "这对应 openclaw 的 passesManifestOwnerBasePolicy +
bundled 短路。" → "Bundled / Config origin 短路、其余按 allowlist——这是 Aleph
自己的规则，不再标注出处。"

Body of Owner Trust section **unchanged** (`[trust] enforce` / `plugins.toml`
paragraph, `plugin_manage` 4 trust verbs, `qa/plugins/run.sh trust` coverage
note, "Bundled/Config 至今没有生产者" reasoning, "为什么技能目录没有对应的闸"
subsection) — user flagged as do-not-touch.

### (i) 生命周期四原语 section inserted before `## 进程级投影的单一咽喉`
- 4 primitives table (mount / unmount / reload_plugin / reload) with result types
  and semantics
- 6 step labels table (registry_row / wasm_module / mcp_server / service /
  memory_extension / slash_command) with register ↔ unregister pairs, **2026-09-20
  新增** annotations on memory_extension and slash_command
- "规则只有一句" paragraph (effect vs. view rule)
- `after_transition()` paragraph (single trigger point, three syncs, load_guard
  serialization, 4-entry API surface, `String` not newtype)
- G1 census / G2 round-trip / G3 single-author guard list

### (j) Cordis paragraph in `## 进程级投影的单一咽喉`
Replaced 5-line Cordis paragraph with 6-line "列举法只对了一半" paragraph:
effect-not-return-value observation + 2026-08-16 answer + 2026-09-20 局限性
(memory extension / slash / MCP / reload_plugin 四处漏) + 效果归 EffectScope /
视图归派生 (cross-ref to (i)) + `lifecycle.rs::after_transition` single trigger.
Following "它替换掉的缺陷" paragraph + 2-author table **unchanged** — historical
defect still relevant.

## Pre-condition vs post-condition greps

Pre (plan Step 1):
```
openclaw: 6 hits (:672 heading, :677 sentence, :748/:759 history, :768 heading, :779 sentence)
overridden: 2 hits (:159, :165 — both inside "于 2026-09 删除" blockquote)
ClaudeCache|visible_to|ScopeKey: 3 hits (:247 ClaudeCache pre-existing, :253 visible_to pre-existing P2 ref, :775 ClaudeCache pre-existing trust section)
```

Post (plan Step 3):
```
$ rg -n 'openclaw' docs/reference/PLUGIN_SYSTEM.md
(no output)

$ rg -n 'overridden|Overridden' docs/reference/PLUGIN_SYSTEM.md
159:> **2026-08-16 之前只有前两个是真的。** `Overridden` / `Error` 是**零生产者**的枚举变体：
165:> 现在 `error` / `blocked` 有了 registry 行 + `status_detail`。**`overridden` 从未有过生产者**：

$ rg -c 'after_transition|EffectScope|visible_to|ClaudeCache|system_prompt|CC_TOOL_ALIASES' docs/reference/PLUGIN_SYSTEM.md
15
```

Plan thresholds satisfied:
- openclaw = 0 ✓ (was 6)
- overridden stays inside "于 2026-09 删除" blockquote (2 hits, both legitimate) ✓
- contract names count >= 8 ✓ (15)

The "ClaudeCache/visible_to/ScopeKey 全局 0" was correctly read as a
no-requirement (the very contract of (b)(c) is to introduce them, so 0 globally
is the wrong goal). The 3 pre-existing hits (lines 247, 253, 775) are legitimate
P2 / Trust policy references from other tasks' work — **not P8.3 noise**.

## Harness-diff check

```
$ git diff f7d1cbce3 -- src/harness/ | wc -l
       0
```

0 lines. No code touched. R10 intact.

## Markdown / link integrity

```
$ git diff --check docs/reference/PLUGIN_SYSTEM.md
(no output)
```

No whitespace / conflict-marker warnings. No new external/relative links.

## Headings uniqueness

`grep -n '^## ' docs/reference/PLUGIN_SYSTEM.md` returns all unique H2 headings
(verified by `sort | uniq -c` — no duplicates).

## Diff stat

```
$ git diff --stat docs/reference/PLUGIN_SYSTEM.md
 docs/reference/PLUGIN_SYSTEM.md | 137 ++++++++++++++++++++++++++++++----------
 1 file changed, 104 insertions(+), 33 deletions(-)
```

Growth dominated by (i) 新 section (~35 行) + (d) DEVIATION/CONNECT (~25 行) +
(c) section rewrite (~14 行 net) + (b) ClaudeCache (~8 行) + (j) paragraph swap.

## Deviations from plan text

| # | Plan item | Deviation | Why |
|---|---|---|---|
| 1 | (a) `disabled` 含义 / 补救 | 保留原 补救 cell（Panel / plugin_manage 拒绝 / claude_cache 行 model can't enable）; 加一句跨参到 (b) ClaudeCache 段 | plan 短版会丢集成信息 |
| 2 | (d) `commands` 行 | plan 短 cell 替换长 cell; 长 cell 的 `!cmd` 同意清单 / 沙箱外 / env 继承细节在本行不再重复 | 这些语义在「环境变量」节有完整版; plan call for 短版 |
| 3 | (a) blockquote 末尾 | 追加 plan 指定的 activation_gate 句 | plan 显式要求 append (not replace) |

No other deviations. enable/disable 耐久块 + Owner Trust 主体未动。

## Self-review

- **Completeness**: (a)–(j) 全部按 plan 实现; commit 已落 (见 git log)
- **质量**: post-condition 命中; harness-diff 空; markdown 干净; heading 唯一
- **不越界**: 未读 `~/.claude/settings.json`, 未写 `~/.aleph/`, 未启动
  `aleph-server`, 未跑 cargo, 未用 `cargo fmt` / `--no-verify` / `git add -A` /
  `stash`, 不动 enable/disable 耐久块, 不动 Owner Trust 主体
- **诚实**: 所有 grep 输出是实际执行的真实输出, 未编造验证
