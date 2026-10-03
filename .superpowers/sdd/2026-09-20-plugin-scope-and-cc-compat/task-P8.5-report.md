# Task P8.5 — GATEWAY.md MCP-face section

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1619-1680`
- **File modified**: `docs/reference/GATEWAY.md` (only this; no code touched)
- **HEAD at start**: `dbea7aa7b` clean, `git diff dbea7aa7b -- src/harness/` empty
- **HEAD at end**: `dbea7aa7b` clean after commit (see below)

## Change

Inserted `### MCP 面（`src/gateway/mcp_face/`，2026-09-20）` as a `###` subsection under `## HTTP Server`,
between `### Channel webhook ingestion` and `## See Also`. Section text inserted verbatim from the
plan's code block (plan lines 1631-1666), plus a trailing `---` separator that the plan itself
includes. Insertion point anchored on the existing `---` immediately before `## See Also`.

```
$ rg -n '^### Channel webhook ingestion|^### MCP 面|^## See Also' docs/reference/GATEWAY.md
1284:### Channel webhook ingestion
1362:### MCP 面（`src/gateway/mcp_face/`，2026-09-20）
1397:## See Also
```

`git diff --check` clean; `+35` insertions, 0 deletions (the section is one continuous block, all
new lines).

## Process note — first attempt's tool failure

The first attempt in this session used the harness `edit` tool. The tool rejected the call with
`edits.0.newText: must be string` on a payload whose JSON it had pre-processed into a structured
object (`newText.token.String.McpFace.server.tool.$text` …) — the JSON pre-processing interpreted
backtick-heavy inline code spans and `$`-prefixed keys as nested fields. No file change resulted.
On retry the file was modified via a Node.js script (`node /tmp/p85-insert.mjs`, written to `/tmp`,
not in the repo) that performed an in-memory anchor-replace, leaving the rest of `GATEWAY.md`
byte-identical. The script's anchor (`"reported it dead.\n\n---\n\n## See Also"`) was verified
unique before write.

This report file is staged with `git add -f` because `.superpowers/sdd/` is not tracked by
default; the directory is per-task scratchpad, not source.

## Post-conditions (plan Step 3)

All four checks pass against the post-insertion file:

| Check | Plan target | Observed |
|---|---|---|
| `rg -n 'mcp_face' docs/reference/GATEWAY.md \| wc -l` | ≥ 3 | 4 |
| `rg -n '2026-07-28' docs/reference/GATEWAY.md` | exactly 1, on the line that says it is NOT spoken | 1 hit, line 1375, on `2026-07-28 是**无握手、无 session** 的方言 … 这一面**不说它**` |
| `rg -c 'SUPPORTED_PROTOCOL_VERSIONS' docs/reference/GATEWAY.md` | 1 | 1 |
| `rg -n 'packages/pi-aleph/README.md' docs/reference/GATEWAY.md` | 1 | 1 (line 1365) |
| `rg -n 'MCP_REMOTE_POSTS_PER_MINUTE\|DEFAULT_EXPOSE_EXCLUDES\|LIVE_SUBSECTIONS' \| wc -l` | 3 | 3 (lines 1377, 1378) |

`git diff --check docs/reference/GATEWAY.md` reports no whitespace/conflict errors.

## Implementation references verified at `dbea7aa7b`

Every claim in the inserted section was checked against the tree before insertion. Path : line for
each load-bearing claim:

| Plan claim | Path : line |
|---|---|
| `MCP_LEGACY_PROTOCOL_VERSION = "2025-03-26"` | `src/mcp/protocol.rs:658` |
| `MCP_MODERN_PROTOCOL_VERSION = "2026-07-28"` (not spoken) | `src/mcp/modern/mod.rs:42` |
| `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25","2025-06-18", MCP_LEGACY_PROTOCOL_VERSION]` | `src/gateway/mcp_face/protocol.rs:26` |
| `MCP_REMOTE_POSTS_PER_MINUTE: u32 = 120` | `src/gateway/mcp_face/http.rs:57` |
| `DEFAULT_EXPOSE_EXCLUDES` = `tool_usage` / `config_audit` / `node_list` / `user_profile` | `src/gateway/mcp_face/config.rs:44-65` |
| 36 pinned default names | `src/gateway/mcp_face/config.rs` `PINNED_DEFAULT` (36 entries) |
| `try_mcp_face() -> Option<&'static Arc<McpFace>>` | `src/gateway/mcp_face/mod.rs:395` |
| `notify_tools_list_changed` | `src/gateway/mcp_face/mod.rs:322` |
| `MCP_APPROVAL_HINT` | `src/gateway/mcp_face/mod.rs:49` |
| `MCP_PATH = "/mcp"` | `src/gateway/mcp_face/http.rs:48` |
| 401 + `WWW-Authenticate: Bearer realm="aleph"` | `src/gateway/mcp_face/http.rs:172` |
| 426 (insecure remote) / 403 (origin) / 429 (rate limit) / 404 (unknown session) / 400 (no session header) | `src/gateway/mcp_face/http.rs:144, 153, 196, 210, 214, 344, 349` |
| `Retry-After` header on 429 | `src/gateway/mcp_face/http.rs:197` |
| `Mcp-Session-Id` minted by `initialize` | `src/gateway/mcp_face/session.rs`, header wired in `src/gateway/mcp_face/http.rs:7` |
| `McpClient { client_name }` principal | `src/gateway/mcp_face/session.rs:44` |
| `lifecycle.rs::after_transition` → `try_mcp_face().notify_tools_list_changed()` | `src/extension/lifecycle.rs:1055-1056` |
| `GatewayServer::operator_presence_probe()` | `src/gateway/server/mod.rs:736` |
| `qa/mcp_face/run.sh` stages `{handshake,tools,auth,list_changed,deny}` | `qa/mcp_face/run.sh` header, `qa/README.md:664-672` |

## Deviations from plan (documented per AGENTS.md "State the Negative")

### D1 — Pre-condition `rg -n 'mcp_face|/mcp\b' docs/reference/GATEWAY.md → 0` does not hold

Before this commit the file already had a one-line MCP summary inside the `## HTTP Server`
bullet list (lines 1166 + 1171), so the pre-condition count was 2, not 0. This summary is a
legitimate TL;DR; the new `###` section is a deeper expansion that re-states the same point at
full detail. No existing text was removed.

### D2 — `packages/pi-aleph/` does not exist at `dbea7aa7b` (P6.8 not yet landed)

The plan's first paragraph says "客户端侧的接法只写一份：`packages/pi-aleph/README.md`". Per the
plan's own "lead addendum" (plan lines 2073 + plan-task body line 1624) the pointer sentence
must live in P8.5's section because P6.8 "cannot write into it". P6.8 has not landed at this
HEAD, so the link is currently dangling; it will resolve when P6.8 creates the package. P8.5
is not the right task to fix this (out of scope: "不碰代码", "不提前 P8.6+").

### D3 — `mcp_server.expose` not in `LIVE_SUBSECTIONS` (P6.9 not yet landed)

The plan's row "**`expose` 改动即时生效**（`mcp_server.expose` 在 `LIVE_SUBSECTIONS` 里 …）"
claims that `LIVE_SUBSECTIONS` contains `mcp_server.expose`. At `dbea7aa7b`:

- `src/config/reload_impact.rs:108` declares `LIVE_SUBSECTIONS = &["policies.spend", "policies.terminal"]`
  only. `mcp_server.expose` is not in it.
- `src/config/live_apply.rs:233-242`'s `every_live_section_has_an_apply_arm` test lists the
  same six arms (`route`, `execution`, `behavior`, `policies.spend`, `policies.terminal`,
  `search`) and would fail loudly if `mcp_server.expose` were added without an arm.
- `apply_expose` (referenced in `src/gateway/mcp_face/mod.rs:61`'s docstring) does not exist
  anywhere in `src/`.

The plan text is consistent with the source's own docstrings (`src/config/structs.rs:176`,
`src/gateway/mcp_face/config.rs:19` both say "Applies live"). In other words, the doc claim and
the code's stated intent agree, but the runtime const list and the apply arm have not been
written yet — that is the P6.9 hole. P8.5's section reflects the planned/intended state
verbatim per the plan; the runtime will match once P6.9 lands. Code fix is out of scope for P8.5.

### D4 — Spec version-literal header in section title

The plan title says `### MCP 面（`src/gateway/mcp_face/`，2026-09-20）`. The `2026-09-20`
marker is the design/ruling date used throughout the `2026-09-20-plugin-scope-and-cc-compat`
plan family; the implementation lives at `dbea7aa7b` which is dated 2026-09-26+. The title
date is the *plan* date, not the commit date — kept as the plan dictates.

## What was NOT done (state-the-negative)

- No source file under `src/` was modified.
- No `cargo fmt`, no `--no-verify`, no `git add -A`, no `stash`.
- No `~/.claude/settings.json` read, no `~/.claude/.aleph` written, no server started.
- Did not advance to P8.6 or any later task.
- Did not run the qa fixture (`qa/mcp_face/run.sh`) — that's a runtime check, P8.5 is docs only.
- The brief one-line MCP summary in `## HTTP Server`'s bullet list (lines 1166 + 1171) was
  intentionally **left in place**: the plan instructs to "insert" the new section, not to
  remove or rewrite the existing bullet. The new section duplicates the high-level fact for
  self-containedness; the bullet survives as a TL;DR.
- Post-insertion, the new section's claim that `expose` is live-applied is **currently
  inaccurate at runtime** until P6.9 lands (D3). This is a documented divergence, not a hidden
  one — any operator reading P8.5's section will see the docs/code tension resolved only by
  landing P6.9's const + arm.

## Commit

`docs(gateway): document the MCP face`
