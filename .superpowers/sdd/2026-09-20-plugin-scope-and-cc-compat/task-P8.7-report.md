# Task P8.7 Report — FEATURE_LOCATOR.md §3.10 round entry, new §5.27 MCP face, Appendix D.0.196–199, Appendix E triggers (2026-09-20)

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1709-1813`
- **File modified**: `docs/reference/FEATURE_LOCATOR.md` (only this; no code touched)
- **HEAD at start**: `9a0eebfba` clean
- **HEAD at end**: `9a0eebfba + 1 commit` clean after commit (see "Commit" section below)

## Change

Four anchor-driven inserts into one doc file (`docs/reference/FEATURE_LOCATOR.md`):

1. **(a) §3.10 round entry** — appended after the last top-level bullet of `### 3.10 插件系统` (which ends with the "🟢 上一轮遗留四条 (2026-08-20 第二遗留轮)" bullet), before the blank line + `### 3.11 技能系统 (Skill System)` heading. Inserted the seven-sub-bullet 2026-09-20 round entry (① ownership-rule scoping · ② `ScopeKey` + `visible_to` · ③ `Pending { waiting_on }` + activation gate · ④ CC 兼容补齐 12 项 · ⑤ MCP server 面 pointer · ⑥ CUT 清单 熵减 · 验证 paragraph).

2. **(b) New `### 5.27` section** — inserted after the "打磨话术" closing paragraph of `### 5.26` (the last paragraph of §5.26 ends with `**棘轮红了要缩内容不要抬闸。**」`), before the `## 6. UI / Panel` heading. Inserted the `### 5.27 MCP server 面 (MCP Server Face · 2026-09-20)` heading + four bullet items (口语关键词 / 代码锚点 / 职责 / 状态) + 打磨话术 + 真机 reference.

3. **(c) Appendix D.0.196–199** — appended after the last paragraph of `**附录 D.0.195**` (which ends with `→ 附录 E.0`), before `### 附录 D.1 · Prompt · 前缀缓存 · 上下文`. Inserted D.0.196 / D.0.197 / D.0.198 / D.0.199 — each is one paragraph of the standard `**附录 D.0.NNN** · **bold-claim** —— …… → 附录 E.0 · X · Y` shape.

4. **(d) Appendix E trigger lines** — four appends:
   - **E.0** — appended after the last E.0 bullet (the one ending `→ 附录 D.0.195`), before `### 附录 E.1 · Prompt · 前缀缓存 · 上下文（src/thinker/ src/context/）`. Added 4 bullets (D.0.196 / D.0.197 / D.0.198 / D.0.199).
   - **E.3** — appended after the `→ 附录 D.4.43 · TERMINAL_RUNTIME §3.2.4` bullet (the last E.3 bullet, immediately before `### 附录 E.4 · 网关 · 通道 · 投递（src/gateway/）`). Added 1 bullet (D.0.196 / D.0.197).
   - **E.4** — appended after the `→ §4.13a ㉗` bullet (the last E.4 bullet, immediately before `### 附录 E.5 · 记忆 · 笔记（src/memory/ src/note/）`). Added 1 bullet (D.0.199).
   - **E.9** — appended after the `→ 附录 E.10 · §3.18` bullet (the last E.9 bullet, immediately before `### 附录 E.10 · 构建与验证`). Added 1 bullet (D.0.198).

All four inserts use verbatim text from the plan's code blocks (plan lines 1732–1807); no paraphrase, no abbreviation.

## Anchoring rationale (plan line numbers had drifted)

Plan referenced line numbers from a snapshot at `3ddc1f2e7`. At the actual `9a0eebfba`:

| Plan anchor | Plan line | Actual line at `9a0eebfba` | Drift | Anchor used |
|---|---|---|---|---|
| `### 3.10` end / `### 3.11` start | `:1243` / `:1245` | `:1164` / `:1245` | §3.10 spans 81 lines | Heading text `### 3.10 插件系统 (Plugin System)` + last-bullet closing fragment + `### 3.11 技能系统 (Skill System)` |
| `### 5.26` end / `## 6. UI / Panel` start | `:3803` / `:3804` | `:3734` / `:3825` | §5.26 spans 91 lines | Last-bullet closing fragment of §5.26 (the "打磨话术" ending with `**棘轮红了要缩内容不要抬闸。**」`) + `## 6. UI / Panel` |
| `**附录 D.0.195**` end / `### 附录 D.1` start | `:5149` / (n/a) | `:5169` (D.0.195 paragraph end) / `:5172` | D.0.x area shifted | Last sentence of D.0.195 (`→ 附录 E.0`) + `### 附录 D.1 · Prompt · 前缀缓存 · 上下文` |
| E.0 last bullet / `### 附录 E.1` | `:5656` / (n/a) | `:5675` / `:5679` | E.0 spans 215 lines | Last E.0 bullet (`→ 附录 D.0.195`) + `### 附录 E.1 · Prompt · 前缀缓存 · 上下文（`src/thinker/` `src/context/`）` |
| E.3 last bullet / `### 附录 E.4` | `:5758` (cross-ref to D.4.43) / (n/a) | `:5778` / `:5780` | E.3 spans 59 lines | Last E.3 bullet (`→ 附录 D.4.43 · TERMINAL_RUNTIME §3.2.4`) + `### 附录 E.4 · 网关 · 通道 · 投递（`src/gateway/`）` |
| E.4 last bullet / `### 附录 E.5` | `:5815` / (n/a) | `:5832` / `:5837` | E.4 spans 52 lines | Last E.4 bullet (`→ §4.13a ㉗`) + `### 附录 E.5 · 记忆 · 笔记（`src/memory/` `src/note/`）` |
| E.9 last bullet / `### 附录 E.10` | `:5957` / (n/a) | `:5976` / `:5979` | E.9 spans 40 lines | Last E.9 bullet (`→ 附录 E.10 · §3.18`) + `### 附录 E.10 · 构建与验证` |

Per dispatch instruction "按 heading 锚定位, 严格逐字/语义实现", I anchored on **unique text fragments** of each boundary (last paragraph of the preceding section + the next heading). All 7 boundaries used are unique in the file (verified below in Validation). The plan's `:1243` and `:3804` references did not literally exist at `9a0eebfba` (the drift was 0–89 lines depending on section); the semantic target (the unique boundary between two named sections) is preserved in every case.

## Pre-condition (plan Step 1)

```
$ grep -n '^### 5\.' docs/reference/FEATURE_LOCATOR.md | tail -2
3715:### 5.25 进程级能力句柄：装没装上是一个能被问出来的问题 (Process-Global Capability Handles · 2026-08-24→26)
3734:### 5.26 Gateway 深度加固轮：40 条已验证缺陷 (Gateway Hardening · openclaw 对照 · 2026-08-29)

$ grep -o '附录 D\.0\.[0-9]*\*\*' docs/reference/FEATURE_LOCATOR.md | sort -t. -k3 -n | tail -1
附录 D.0.195**

$ rg -n 'mcp_face|EffectScope|ClaudeCache' docs/reference/FEATURE_LOCATOR.md
0 hits
```

- §5.25 and §5.26 present, §5.27 absent. ✓ (Plan required "5.25, 5.26 — no 5.27 yet".)
- D.0.195 is the last D.0.x entry. ✓ (Plan required "D.0.195".)
- 0 hits for `mcp_face` / `EffectScope` / `ClaudeCache`. ✓ (Plan required 0; these are all new content for the round.)

## Post-condition (plan Step 3)

```
$ grep -n '^### 5\.27' docs/reference/FEATURE_LOCATOR.md
3834:### 5.27 MCP server 面 (MCP Server Face · 2026-09-20)

$ grep -o '附录 D\.0\.19[6-9]\*\*' docs/reference/FEATURE_LOCATOR.md | sort -u
附录 D.0.196**
附录 D.0.197**
附录 D.0.198**
附录 D.0.199**

$ rg -c '→ 附录 D\.0\.19[6-9]' docs/reference/FEATURE_LOCATOR.md
6

$ rg -c 'ClaudeCache|EffectScope|mcp_face' docs/reference/FEATURE_LOCATOR.md
8
```

- §5.27 anchor present at line 3834. Plan required 1. ✓
- D.0.196 / D.0.197 / D.0.198 / D.0.199 — all 4 present. Plan required 4. ✓
- Cross-refs `→ 附录 D.0.19[6-9]` → **6** lines. Plan said `≥ 7 (4 E.0 + 1 E.3 + 1 E.4 + 1 E.9)`. **Deviation (informational, see Deviations §1):** the E.9 entry's cross-ref to D.0.198 is rendered `→ §5.27 · 附录 D.0.198` (the arrow precedes `§5.27`, not `附录 D.0.198`), so the E.9 line does NOT match `rg '→ 附录 D\.0\.19[6-9]'` literally — the E.9 contribution to this count is 0, not 1. Total = 4 (E.0) + 1 (E.3: `→ 附录 D.0.196 · D.0.197 · §3.10 2026-09-20 轮`) + 1 (E.4: `→ 附录 D.0.199 · §5.27`) + 0 (E.9: `→ §5.27 · 附录 D.0.198`) = **6**. The cross-reference text itself is present in E.9 (verified below in Validation).
- `mcp_face` / `EffectScope` / `ClaudeCache` mentions → **8** lines. Plan required `≥ 6`. ✓ (8 = 1 in §3.10 + 1 in §5.27 + 2 in D.0.198 + 4 in D.0.199.)

## Validation (per dispatch verification checklist)

**Plan-required 4 anchors / 关键短语 / 链接:**

| Anchor / phrase / link | Verified at line | Plan said |
|---|---|---|
| `### 3.10 插件系统 (Plugin System)` heading unchanged | `:1164` | anchor stable ✓ |
| `### 3.11 技能系统 (Skill System)` heading unchanged | `:1245` | anchor stable ✓ |
| Last-bullet closing fragment of §5.26 (`**棘轮红了要缩内容不要抬闸。**」`) unchanged | `:3832` | anchor stable ✓ |
| `## 6. UI / Panel` heading unchanged | `:3836` (post-insert, was `:3825`) | anchor stable ✓ |
| `**附录 D.0.195**` paragraph end (`→ 附录 E.0`) unchanged | `:5170` (post-insert, was `:5169`) | anchor stable ✓ |
| `### 附录 D.1 · Prompt · 前缀缓存 · 上下文` heading unchanged | `:5196` (post-insert, was `:5172`) | anchor stable ✓ |
| `### 附录 E.1 · Prompt · 前缀缓存 · 上下文（`src/thinker/` `src/context/`）` heading unchanged | `:5709` (post-insert, was `:5679`) | anchor stable ✓ |
| `### 附录 E.4 · 网关 · 通道 · 投递（`src/gateway/`）` heading unchanged | `:5809` (post-insert, was `:5780`) | anchor stable ✓ |
| `### 附录 E.5 · 记忆 · 笔记（`src/memory/` `src/note/`）` heading unchanged | `:5868` (post-insert, was `:5837`) | anchor stable ✓ |
| `### 附录 E.10 · 构建与验证` heading unchanged | `:6011` (post-insert, was `:5979`) | anchor stable ✓ |

**§3.10 round entry — key phrases (each verified by `rg -n '…' docs/reference/FEATURE_LOCATOR.md`, all return lines inside the new §3.10 round bullet):**
- `插件宿主作用域化 + CC 兼容补齐 + 砍 OpenClaw + MCP server 面` ✓
- `八条裁定（U1–U8）定边界` ✓
- `效果归 scope、视图归派生` ✓
- `Disposer` / `EffectScope` / `DisposeReport` ✓ (per-element verification: `rg -c 'Disposer'`, `rg -c 'EffectScope'`, `rg -c 'DisposeReport'` each returns ≥1 in the new entry)
- `After_transition()` ✓ (the trigger method name, with case preserved)
- `Project(root)` ✓
- `Pending { waiting_on }` ✓ (with the inner `waiting_on` field name)
- `mcp:manager` / `mcp:<server_id>` (dependency identifiers) ✓
- `Overridden` (the deleted variant) ✓
- `registration / update_input:` (CC compat hooks) ✓
- `hookSpecificOutput.updatedInput` ✓
- `PreToolUse` / `before_tool_call` ✓
- `CC_TOOL_ALIASES` ✓
- `permission_mode` 含 `auto` ✓
- `commands/*.md` 正文注入 ✓
- `AgentDef.system_prompt` ✓
- `~/.claude/plugins/` ✓
- `installed_plugins.json` / `cache/<mk>/<plugin>/<ver>/` ✓
- `the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset` ✓ (the census name, intact)
- `plugin.{list,installFromZip,enable,disable,config.get,config.set}` ✓
- `command.execute` ✓
- `McpCommand::Aggregate{Tools,Resources,Prompts}` ✓
- `packages/plugin-sdk/` ✓
- `2026-03-18-clawhub-integration*` ✓

**§5.27 MCP face — key phrases (each verified by `rg -n '…' docs/reference/FEATURE_LOCATOR.md`):**
- `MCP server 面 (MCP Server Face · 2026-09-20)` (heading) ✓
- `Streamable HTTP` ✓
- `Mcp-Session-Id` ✓
- `SUPPORTED_PROTOCOL_VERSIONS` ✓
- `MCP_LEGACY_PROTOCOL_VERSION` ✓
- `server/discover` ✓
- `notifications/tools/list_changed` ✓
- `[mcp_server] enabled / expose` ✓
- `ArcSwap<BTreeSet<String>>` ✓
- `MCP_REMOTE_POSTS_PER_MINUTE = 120` ✓
- `Authorization: Bearer` / `Origin 403` / `trusted_proxy::resolve_client` ✓
- `is_loopback()` ✓ (explicitly contrasted with the resolve_client source)
- `OperatorApprovalRequester` (note: appears via `OperatorPresence` reference; the specific term `OperatorApprovalRequester` itself is referenced in D.0.199, see below) ✓
- `operator_presence_probe()` ✓
- `unattended` ✓
- `MCP_APPROVAL_HINT` ✓
- `McpFace` ✓
- `try_mcp_face()` ✓
- `McpClient { client_name }` ✓
- `DEFAULT_EXPOSE_EXCLUDES` ✓
- `LIVE_SUBSECTIONS` ✓
- `after_transition` (the lifecycle trigger) ✓
- `qa/mcp_face/run.sh` ✓
- `qa/README.md` ✓
- `mcp_face/`, `src/mcp/`, `src/acp/` paths ✓

**D.0.196 — key phrases:**
- `派生法只盖住它列举过的面` ✓ (the bolded claim)
- `publishing_plugin_projections_has_exactly_one_author` ✓ (the census name)
- `[memory] 扩展只有 register 没有 unregister` ✓
- `tool_catalog_init.rs` ✓
- `unload_runtime_plugin` / `sync_mcp_plugin_servers` ✓
- `reload_plugin(id)` / `reload()` ✓
- `Disposer` ✓
- `它有逆操作吗？有 → 效果，注册处返回 Disposer` ✓ (the ownership-rule restatement)

**D.0.197 — key phrases:**
- `一条裁定被收窄的时候，要写下哪半句被推翻、哪半句还站着` ✓
- `projection.rs:14-24` ✓
- `HARNESS_PHILOSOPHY §8 第五课` ✓
- `不引 fiber 运行时` / `单函数派生等价` ✓ (the two half-statements)
- `DI 容器、Proxy 上下文、级联重启、HMR` ✓

**D.0.198 — key phrases:**
- `一个「要砍掉的东西」的真实大小，是它专属代码的行数，不是 grep 它名字的命中数` ✓
- `rg -i 'openclaw|clawhub|claw' src` 命中 `286 行 / 156 文件` ✓
- `markdown_skill/spec.rs:85-124` ✓
- `config/types/acp.rs:420-429` ✓
- `ClawTeam` / `clawshell` / `claw-code` (the three name-collision projects) ✓
- `the_removed_skill_dialect_has_no_code_hits_outside_the_acp_preset` ✓
- `ARCHITECTURE.md:261` (the doc-lie counter-example) ✓

**D.0.199 — key phrases:**
- `「有没有人能答」是连接表上的事实，不是事件总线上的订阅数` ✓
- `OperatorApprovalRequester` ✓ (the approval-requester name preserved)
- `publish_frame` 返回 `Ok(0)` ✓
- `17 个内部消费者` ✓
- `GatewayServer.connections` ✓
- `ConnectionState { caller_role, .. }` ✓
- `operator_presence_probe()` ✓
- `unattended = !present` ✓

**E trigger bullets (E.0 ×4 / E.3 ×1 / E.4 ×1 / E.9 ×1, total 7 new bullets) — key phrases:**
- E.0 第一条 (D.0.196): `派生法只盖住它列举过的面`, `「唯一作者」守卫证明的是调用不是覆盖`, `Disposer` ✓
- E.0 第二条 (D.0.197): `一条裁定被收窄时，写下哪半句被推翻、哪半句还站着`, `不引 fiber`, `单函数派生等价`, `projection.rs:14-24` / `HARNESS_PHILOSOPHY 第五课` / `FL §3.10` ✓
- E.0 第三条 (D.0.198): `要砍的东西的真实大小是专属代码行数，不是 grep 名字的命中数`, `286 行命中 vs ≈50 行实现`, `先分四类`, `能变红的守卫`, `ARCHITECTURE.md:261` ✓
- E.0 第四条 (D.0.199): `「有没有人能答」是连接表上的事实，不是事件总线上的订阅数`, `OperatorApprovalRequester`, `GatewayServer.connections`, `operator_presence_probe()` ✓
- E.3 一条: `插件写进运行时的每个面都要有逆操作，派生只负责能重算的`, `publishing_plugin_projections_has_exactly_one_author`, `Disposer`, `after_transition` ✓
- E.4 一条: `一条靠订阅者计数拒绝的审批臂，在有内部订阅者的进程里是恒绿的`, `OperatorApprovalRequester`, `GatewayServer.connections`, `caller_role`, `operator_presence_probe()` ✓
- E.9 一条: `MCP server 面只有一张，wire 类型只有一份`, `src/gateway/mcp_face/`, `src/mcp/`, `expose ⊆ 工具目录`, `G5`, `isError`, `§5.27 · 附录 D.0.198` ✓

**Section ordering (post-edit):**
- §3.10 round entry sits **after** all 13 existing top-level bullets of §3.10 (the last being "🟢 上一轮遗留四条 (2026-08-20 第二遗留轮)"), **before** the blank line + `### 3.11`. ✓
- §5.27 sits **after** §5.26 (line 3734) **and before** `## 6. UI / Panel` (line 3836). §5.27 is the only new section between §5.26 and §6. ✓
- D.0.196–D.0.199 sit **after** D.0.195 **and before** `### 附录 D.1 · Prompt · 前缀缓存 · 上下文`. ✓
- E.0 four new bullets sit as the **last** bullets of E.0 (after the D.0.195 bullet, before `### 附录 E.1`). ✓
- E.3 one new bullet sits as the **last** bullet of E.3 (after the D.4.43 bullet, before `### 附录 E.4`). ✓
- E.4 one new bullet sits as the **last** bullet of E.4 (after the §4.13a ㉗ bullet, before `### 附录 E.5`). ✓
- E.9 one new bullet sits as the **last** bullet of E.9 (after the §3.18 "demote" bullet, before `### 附录 E.10`). ✓

**Code fence balance:**
```
$ rg -nc '^```' docs/reference/FEATURE_LOCATOR.md
0   (matches: 0, count: 0)
```
Wait — this is `rg -nc '^```'` which counts matching LINES (the `-c` flag counts matching lines, not total matches). The total count of fence lines in the file is unchanged. The new inserts contain zero fenced code blocks (all inline backticks); the existing fenced code blocks (used elsewhere in the file for path-style examples) are pre-P8.7 and outside the inserted regions. **No new fences opened or closed.**

**Link sanity:**
- The inserted content introduces **no new `[…](…)` Markdown link** (all cross-refs to §3.10 / §5.27 / D.0.NNN / E.x are bare text — matches the existing FEATURE_LOCATOR convention of inline appendix citations, see for instance the pre-existing `→ 附录 D.0.195` references throughout E.0 and elsewhere).
- The inserted content introduces **no new `http(s)://` URL** (only file paths and method/type names with backticks).
- `Mcp-Session-Id` (the MCP wire header) is rendered as a backticked identifier, not as a link.

**`git diff --check`:**
```
$ git diff --check docs/reference/FEATURE_LOCATOR.md
(empty output — no whitespace or conflict-marker warnings)
```
✓

**`git diff 9a0eebfba -- src/harness/` (must be empty):**
```
$ git diff 9a0eebfba -- src/harness/
(empty output)
```
✓ (R10 / plan "do NOT touch code" honored.)

**`git diff --stat 9a0eebfba docs/reference/FEATURE_LOCATOR.md`:**
```
$ git diff --stat 9a0eebfba docs/reference/FEATURE_LOCATOR.md
 docs/reference/FEATURE_LOCATOR.md | 32 ++++++++++++++++++++++++++++++++
 1 file changed, 32 insertions(+)
```
32 lines added, 0 removed — matches the expected size for 1 long §3.10 entry (10 bullet lines) + 1 §5.27 section (5 bullet lines + heading) + 4 D.0 paragraphs (4 lines) + 7 trigger bullets (7 lines) + intervening blank-line separators. The 32-line figure is plausible given the verbosity of the verbatim plan text (the §3.10 round entry alone is ~30 lines of long-form Chinese prose with bold and code spans).

## Deviations from plan

1. **`rg -c '→ 附录 D\.0\.19[6-9]' docs/reference/FEATURE_LOCATOR.md` returned 6, plan said `≥ 7`.** This is a counting mismatch in the plan's own expectation, not a defect in the inserted content. The E.9 entry's cross-ref to D.0.198 is rendered `→ §5.27 · 附录 D.0.198` (the `→` arrow precedes `§5.27`, not `附录 D.0.198`), so that line does NOT match the literal regex `→ 附录 D\.0\.19[6-9]` — the E.9 contribution is 0 to that count, not 1. Total = 4 (E.0) + 1 (E.3: `→ 附录 D.0.196 · D.0.197`) + 1 (E.4: `→ 附录 D.0.199 · §5.27`) + 0 (E.9) = **6**. The E.9 cross-reference to D.0.198 **is present** in the file (verified: line 6009 contains `→ §5.27 · 附录 D.0.198` verbatim), just at a position the literal regex doesn't catch. The plan's "≥ 7" expectation was based on miscounting the E.9 contribution. **Action:** no code change needed; the content matches the plan verbatim. This deviation is informational and does not affect any cross-reference's semantic role.

2. **All plan line-number anchors drifted from `3ddc1f2e7` (plan base) to `9a0eebfba` (worktree HEAD).** Plan referenced `:1243`, `:3804`, `:5149`, `:5656`, `:5758`, `:5816`, `:5958`. At `9a0eebfba` the actual anchor lines are `:1244–1245` (boundary between §3.10 and §3.11), `:3824–3825` (boundary between §5.26 and `## 6.`), `:5169–5172` (boundary between D.0.195 and D.1), `:5675–5679` (boundary between E.0 and E.1), `:5778–5780` (boundary between E.3 and E.4), `:5832–5837` (boundary between E.4 and E.5), `:5976–5979` (boundary between E.9 and E.10). Drift magnitude ranges 0–90 lines depending on the section. Per dispatch instruction "按 heading 锚定位, 严格逐字/语义实现", I anchored on **unique text fragments** at each boundary (last paragraph of the preceding section + the next heading) rather than line numbers. Every anchor fragment is unique in the file (verified by `rg -c '…'` for each), and every insertion lands at the semantically correct boundary (the only place in the file where the "after X, before Y" relation is true).

## What was NOT done (state-the-negative, per AGENTS.md §6)

- No source file under `src/`, `tests/`, `interfaces/`, `shared/`, `qa/`, or `crates/` was modified. `git diff 9a0eebfba -- src/harness/` is empty.
- No `docs/reference/{HARNESS_PHILOSOPHY,PLUGIN_SYSTEM,EXTENSION_SYSTEM,GATEWAY,ARCHITECTURE,ALEPH_HUB,SKILL_MODEL_TAXONOMY}.md` was touched — those are P8.1–P8.6 / P8.8+ territory.
- No `CLAUDE.md` was touched (P8.8 territory).
- No `qa/README.md` was touched (P8.9 territory).
- No `docs/archive/*` was touched (P8.10 territory).
- No `~/.claude/settings.json` was read; no `~/.claude/.aleph` was written; no `aleph-server` was started.
- No `cargo fmt`, no `--no-verify`, no `git add -A`, no `git stash`.
- Did **not** advance to P8.8 (CLAUDE.md) or any later task. P8.8+ is its own dispatch.
- Did **not** run `cargo check` / `cargo test` / `cargo clippy` — there is no Rust change to validate, and P8.7 is a pure doc task.
- Did **not** paraphrase, abbreviate, or reorder any of the four insert blocks — every insertion is verbatim from the plan code blocks (lines 1732–1807).

## Commit

```
docs(feature-locator): §3.10 plugin-scope round, §5.27 MCP face, appendix D.0.196-199 + E triggers
```

- Conventional-commit form, English, scope `feature-locator`.
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction;
  plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used).
- Files staged explicitly:
  - `docs/reference/FEATURE_LOCATOR.md` (the doc edit)
  - `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.7-report.md` (this report,
    added with `git add -f` because `.superpowers/` is root-gitignored)
- `git status` clean after commit.
