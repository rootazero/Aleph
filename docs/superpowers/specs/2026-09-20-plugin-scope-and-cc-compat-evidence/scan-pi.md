# pi extension system — scan for Aleph (pi → Aleph via MCP + CLI)

Scanned 2026-09-20. Sources: `/Volumes/TBU4/Github/pi` (`@earendil-works/pi-coding-agent` **0.85.1**, HEAD `71dca871` 2026-09-11) and `/Volumes/TBU4/Github/pi-cc-extensions` (0.8.70, peer `pi ^0.84.0`). All `file:line` cites are relative to `pi/packages/coding-agent/` unless prefixed. Read-only scan; nothing in either repo was modified.

> **Scope (revised 2026-09-20 by the lead)**: Aleph will **not** host pi extensions. Integration is pi → Aleph (pi mounts Aleph as an MCP server and/or calls `aleph-cli` from `bash`). §1–§6 are kept as background at reduced depth; §7 is a one-paragraph verdict; **§9 is the operative section** (§8 is intentionally absent — the lead's two follow-ups crossed; the MCP/CLI section was first written as §8 and renumbered).

> ⚠️ **Premise correction up front**: `pi-cc-extensions` is **not** a bridge that runs Claude Code plugins inside pi. It is a *Claude-Code-style TUI* suite (tool-call renderers, rich diffs, `/context`, `@session` references, two themes). It emulates CC's **look**, not CC's plugin contract. Details in §6. The team-lead's "existing bridge between the two plugin worlds" does not exist in that repo.

---

## 1. Extension package format

### 1.1 Discovery locations (in load order, first-seen path wins)

| # | Location | Scope | Trust gate | Where |
|---|---|---|---|---|
| 1 | `<cwd>/.pi/extensions/` | project | only if project is trusted (`settingsManager.isProjectTrusted()`) | `src/core/package-manager.ts:2398-2420`; `src/core/extensions/loader.ts:766-768` |
| 2 | `~/.pi/agent/extensions/` (`getAgentDir()`; `PI_CODING_AGENT_DIR` override; `CONFIG_DIR_NAME` from `package.json#piConfig.configDir`) | user | none | `src/config.ts:504,528`; `package-manager.ts:2470-2477` |
| 3 | `settings.json#extensions[]` — explicit file/dir paths (project + user) | either | project trust | `settings-manager.ts:132`; `loader.ts:773-789` |
| 4 | `settings.json#packages[]` — `npm:<spec>` / `git:` URL / local path, string or `{source, autoload?, extensions?, skills?, prompts?, themes?}` filter object | either | project trust | `settings-manager.ts:95-104,131`; `package-manager.ts:1251-1311` |
| 5 | `pi -e <path>` / `pi install <src>` temporary scope | temporary | — | `package-manager.ts:966-975,2066-2069` |

Per-directory discovery rule (**one level only, no recursion**): direct `*.ts`/`*.js` files → load; subdir with `package.json#pi.extensions` → load what it declares; else subdir with `index.ts`/`index.js` → load that. `loader.ts:680-745`.

### 1.2 Install layout for packages

| Source | Installed to (user scope) | Installed to (project scope) | How | Where |
|---|---|---|---|---|
| `npm:name[@range]` | `~/.pi/agent/npm/node_modules/<name>` (legacy fallback: global npm root) | `<cwd>/.pi/npm/node_modules/<name>` | `npm install` via configurable `settings.npmCommand` (npm/pnpm/bun) | `package-manager.ts:2066-2094,1446-1458` |
| git URL (`git:github.com/o/r`, `https://…`, `…#ref`) | `~/.pi/agent/git/<host>/<path>` | `<cwd>/.pi/git/<host>/<path>` | `git clone` → `git checkout <ref>` → **`npm install` inside the clone if `package.json` exists** | `package-manager.ts:1849-1856,1900-1925` |
| local path | used in place | used in place | no install | `package-manager.ts:1326-1352` |

Version drift: npm packages are re-installed when the installed version no longer satisfies the configured range (`package-manager.ts:1286-1293`). `PI_OFFLINE=1` disables all network (`package-manager.ts:51-55`).

### 1.3 Manifest: `package.json#pi`

```ts
interface PiManifest { extensions?: string[]; skills?: string[]; prompts?: string[]; themes?: string[] }   // src/core/pi-manifest.ts:4-9
```
- Paths are package-relative (`./extensions/index.ts`, `./skills`). A directory entry for `skills` is walked for `SKILL.md`. Non-string / non-array fields are dropped silently (`pi-manifest.ts:17-33`).
- Extra fields seen in the wild (`image`, `video`) are catalog metadata for pi.dev, ignored by the loader.
- **No manifest** → the loader falls back to conventional subdirs `extensions/`, `skills/`, `prompts/`, `themes/` at package root (`package-manager.ts:2188-2200`); if none exist, the whole dir is treated as one extension root (`index.ts`) (`package-manager.ts:1341-1347`).
- There is **no** `name`/`version`/`permissions`/`engines` semantics inside `pi` — identity comes from the npm/git source string; `peerDependencies` on `@earendil-works/pi-coding-agent` is convention only (not enforced).
- Ordering/priority: project-scope resource beats user-scope resource of the same path; `!pattern` / `+pattern` / `-pattern` overrides in `settings.json#extensions` etc. enable/disable individual files (`package-manager.ts:703-735`).

### 1.4 Entry point convention

- Entry = ES module whose **default export is a factory** `(pi: ExtensionAPI) => void | Promise<void>` (`loader.ts:498-503`, `types.ts` `ExtensionFactory`). Non-function default ⇒ "does not export a valid factory function" (`loader.ts:588-590`).
- **TypeScript is loaded directly at runtime via `jiti`** (`loader.ts:17,481-496`) — no build step. Compiled `.js` also accepted. In the Bun-compiled binary / Node SEA / bundled Node, `virtualModules` maps the framework imports to the embedded copies; in source mode it uses tsconfig paths; in unbundled-node mode it uses path aliases (`loader.ts:47-73,88-141`).
- Framework modules an extension may import **without installing them**: `typebox` (+`/compile`, `/value`), `@sinclair/typebox`, `@earendil-works/pi-agent-core`, `@earendil-works/pi-tui`, `@earendil-works/pi-ai` (+`/compat`,`/oauth`,`/providers/all`), `@earendil-works/pi-coding-agent`, plus the legacy `@mariozechner/*` names (`loader.ts:48-73`).
- Any **other** dependency must be in the package's own `node_modules` (installed at §1.2 time). Examples: `pi-hermes-memory` uses `better-sqlite3` (native addon!), `pi-web-access` uses `linkedom`/`undici`/`unpdf`, `pi-dynamic-workflows` uses `acorn`, `pi-cc-extensions` uses `jiti` + `@shikijs/cli` + `grok-mermaid`.

### 1.5 Real third-party shapes (survey of `/Volumes/TBU4/Github/pi-*`)

| Package | `pi` field | Entry form | Own deps | Notes |
|---|---|---|---|---|
| `pi-cc-extensions` 0.8.70 | `extensions:["./extensions/index.ts"], themes:[cc-dark.json, cc-light.json]` | TS source, one entry that installs ~10 features | jiti, shiki, grok-mermaid | `files` whitelist ships `extensions/` + `themes/` only |
| `@lanlance/pi-btw` | `extensions:["./extensions/btw.ts"]` | TS | none | pure peer-deps |
| `pi-ask-user` | `extensions:["./index.ts"], skills:["./skills"]` | TS | none | ships a skill dir too |
| `pi-dynamic-workflows` | `extensions:["extensions/workflow.ts"]` | TS (also has `main: dist/index.js` for library use) | acorn | |
| `pi-herdr-agents` | `extensions:["./pi-extension/subagents/index.ts"], skills:["./skills"]` | TS | none | ships `agents/`, `tools/` dirs consumed by its own code |
| `pi-hermes-memory` | `extensions:["./src/index.ts"]` | TS | better-sqlite3 (native), strip-ansi | native addon ⇒ needs real Node ABI |
| `pi-web-access` | `extensions:["./index.ts"]` | TS, ~40 flat files | 9 runtime deps (linkedom, undici, unpdf…) | heavy |
| `pi-agent-browser-native` | `extensions:["./dist/extensions/agent-browser/index.js"]` | **compiled JS** | cross-spawn | only one that ships JS |

Conclusion for a host: **7/8 real packages ship raw TypeScript as the entry** and rely on the host to transpile (`jiti`). All 8 rely on `peerDependencies` being satisfied by the host's virtual-module table. 3/8 need their own `node_modules` installed (1 with a native addon).

## 2. The `ExtensionAPI` surface (only what real extensions rely on)

Defined at `src/core/extensions/types.ts:1252-1506`, built in `loader.ts:255-476`. Two families: **registration** methods write into a per-extension record of Maps (`loader.ts:527-547`); **action** methods delegate to a shared runtime whose stubs throw until `ExtensionRunner.bindCore()` (`loader.ts:167-249`). UI lives on the handler **context** `ctx.ui`, not on `pi`. Guessed names that **do not exist**: `pi.setSystemPrompt` (return `{systemPrompt}` from `before_agent_start`), `pi.getSessionState` (use `ctx.sessionManager`, a 14-method read-only `Pick`, `session-manager.ts:190-206`), `pi.ui.*`.

Census of `pi.*` calls across the 8 real packages (`rg` over `pi-cc-extensions pi-btw pi-ask-user pi-dynamic-workflows pi-herdr-agents pi-hermes-memory pi-web-access pi-agent-browser-native`, non-test `.ts`):

| Method (types.ts line) | Uses | What it does |
|---|---|---|
| `on(event, handler)` (1257-1301) | 88 | subscribe; §3 |
| `registerTool(ToolDefinition)` (1308) | 23 | LLM-callable tool; TypeBox **or** plain JSON Schema params (§4.1) |
| `registerCommand(name, {handler(args, cmdCtx)})` (1317) | 26 | `/name` slash command; handler gets session-control context (`newSession/fork/switchSession/reload`) |
| `sendMessage({customType, content, display}, {triggerTurn?, deliverAs?})` (1365) | 15 | inject a `custom`-role message |
| `sendUserMessage(content, {deliverAs?, expandPromptTemplates?})` (1375) | 5 | inject a user message, always triggers a turn |
| `appendEntry(customType, data)` (1381) | 10 | persist extension state in the session file (never sent to LLM) |
| `events.on/emit(channel)` (1505; `event-bus.ts`) | 16 | process-wide bus shared by all extensions |
| `exec(cmd, args, opts)` (1397) | 7 | spawn without shell |
| `getThinkingLevel/setThinkingLevel/setModel` (1419-1428) | 6 | session knobs |
| `registerShortcut`, `registerMessageRenderer`, `registerEntryRenderer`, `registerMarkdownTransformer` (1320-1358) | 8 | **TUI-only** |
| `getAllTools/getActiveTools/setActiveTools` (1400-1406) | 5 | reshape the tool set (how plan-mode is built) |
| `registerProvider`, `registerFlag/getFlag`, `setSessionName`, `setLabel` | 1-2 | rare |

`ctx.ui` (`types.ts:133-284`) usage: `notify` 71, `setWidget` 8, `theme` 8, `custom` 7, `setStatus/setHeader/select/onTerminalInput` 3 each, `input`/`addAutocompleteProvider` 1. pi's own **RPC mode** already reduces this to a dialog subset (`select/confirm/input/editor/notify/setStatus/setTitle`) and stubs the component-factory calls — `custom()` returns `undefined` (`src/modes/rpc/rpc-mode.ts:163-231`), and well-behaved extensions branch on that (`pi-ask-user/index.ts:1963-1965`).

## 3. Event/hook list with payload shapes and return semantics

All types in `src/core/extensions/types.ts` (payloads 520-1010, result types 1119-1190, union 1086-1114). Dispatch semantics in `src/core/extensions/runner.ts` (`emit` 851-883 generic; specialised emitters 885-1286). Every handler gets `(event, ctx: ExtensionContext)`. Handlers run **sequentially, in extension load order, then handler registration order**; a thrown error is caught and reported via `emitError` **except in `tool_call`** where it propagates (`runner.ts:982-1003` has no try/catch — a throwing `tool_call` handler aborts the tool).

| Event | Payload (fields) | Return value / effect | Chaining rule | types.ts |
|---|---|---|---|---|
| `project_trust` | `{cwd}`; ctx is a reduced `ProjectTrustContext{cwd, mode, hasUI, ui:{select,confirm,input,notify}}` | `{trusted:"yes"\|"no"\|"undecided", remember?}` — decide project trust | first decisive answer | 520-543 |
| `resources_discover` | `{cwd, reason:"startup"\|"reload"}` | `{skillPaths?, promptPaths?, themePaths?}` — extension **contributes** resource paths | all results concatenated (`runner.ts:1197-1244`) | 546-556 |
| `session_start` | `{reason:"startup"\|"reload"\|"new"\|"resume"\|"fork", previousSessionFile?}` | none | | 564-570 |
| `session_info_changed` | `{name}` | none | | 573-577 |
| `session_before_switch` | `{reason:"new"\|"resume", targetSessionFile?}` | `{cancel?}` | first `cancel:true` short-circuits (`runner.ts:862-867`) | 580-584, 1162 |
| `session_before_fork` | `{entryId, position:"before"\|"at"}` | `{cancel?, skipConversationRestore?}` | same | 587-591, 1166 |
| `session_before_compact` | `{preparation: CompactionPreparation, branchEntries, customInstructions?, reason:"manual"\|"threshold"\|"overflow", willRetry, signal}` | `{cancel?, compaction?: CompactionResult}` — **extension can supply its own summary** | last non-cancel result wins; cancel short-circuits | 594-604, 1171 |
| `session_compact` / `session_compact_failed` | `{compactionEntry, fromExtension, reason, willRetry}` / `{reason, errorMessage?, aborted, willRetry, fromExtension}` | none | | 607-630 |
| `session_shutdown` | `{reason:"quit"\|"reload"\|"new"\|"resume"\|"fork", targetSessionFile?}` | none — **the only dispose hook** | | 633-638 |
| `session_before_tree` / `session_tree` | `{preparation: TreePreparation{targetId, oldLeafId, commonAncestorId, entriesToSummarize, userWantsSummary,…}, signal}` / `{newLeafId, oldLeafId, summaryEntry?}` | `{cancel?, summary?, customInstructions?, replaceInstructions?, label?}` | cancel short-circuits | 641-668, 1176-1189 |
| `context` | `{messages: AgentMessage[]}` (a `structuredClone`) | `{messages?}` — **replace the message list sent to the LLM** (prune/inject); not persisted | each handler sees previous handler's output (`runner.ts:1034-1064`) | 688-691, 1119 |
| `before_provider_request` | `{payload: unknown}` (provider wire payload) | any value replaces payload | chained | 694-697, 1123 |
| `before_provider_headers` | `{headers}` **mutate in place**; `null` deletes | return ignored | | 704-707 |
| `after_provider_response` | `{status, headers}` | none | | 710-714 |
| `before_agent_start` | `{prompt, images?, systemPrompt, systemPromptOptions: BuildSystemPromptOptions}` | `{message?: CustomMessage-like, systemPrompt?}` — **inject a message before the turn and/or replace the system prompt for this turn** | messages accumulate; systemPrompt chained; `ctx.getSystemPrompt()` reflects current chain (`runner.ts:1131-1195`) | 717-727, 1156-1159 |
| `agent_start` / `agent_end` / `agent_settled` | `{}` / `{messages}` / `{}` | none | | 730-743 |
| `ui_prompt_start` / `ui_prompt_end` | `{reason:"ui_prompt", kind:"select"\|"confirm"\|"input"\|"editor"\|"custom", title?}` | none | | 748-762 |
| `turn_start` / `turn_end` | `{turnIndex, timestamp}` / `{turnIndex, message, toolResults}` | none | | 764-776 |
| `message_start` / `message_update` / `message_end` | `{message}` / `{message, assistantMessageEvent}` (token-level) / `{message}` | `message_end` → `{message?}` replaces the finalized message (**same role enforced**, `runner.ts:885-925`) | chained | 779-795, 1151 |
| `tool_execution_start` / `_update` / `_end` | `{toolCallId, toolName, args}` / `+partialResult` / `{…, result, isError}` | none (observability) | | 798-824 |
| `model_select` / `thinking_level_select` | `{model, previousModel, source:"set"\|"cycle"\|"restore"}` / `{level, previousLevel}` | none | | 830-843 |
| `tool_call` | `{toolCallId, toolName, input}`; `input` is typed for builtins (`bash/powershell/read/edit/write/grep/find/ls`) else `Record<string,unknown>`; **`input` is mutable in place** = the way to patch args (no re-validation) | `{block?, reason?, terminate?}` — block execution; `terminate` hints agent stop after batch when all blocked results set it | first `block:true` short-circuits; else last result | 890-956, 1125-1135 |
| `tool_result` | `{toolCallId, toolName, input, content:(Text\|Image)[], isError, usage?, details}` | `{content?, details?, isError?, usage?}` — replace pieces of the result | field-wise chained (`runner.ts:927-980`) | 958-1020, 1144-1149 |
| `user_bash` | `{command, excludeFromContext, cwd}` (the `!cmd` / `!!cmd` editor prefix) | `{operations?: BashOperations, result?: BashResult}` — swap the executor (e.g. SSH) or fully handle | first non-undefined wins | 849-857, 1137-1142 |
| `input` | `{text, images?, source:"interactive"\|"rpc"\|"extension", streamingBehavior?}` | `{action:"continue"}` / `{action:"transform", text, images?}` / `{action:"handled"}` | transforms chain; `handled` short-circuits (`runner.ts:1246-1284`) | 867-891 |

**Not present** (team-lead guesses): no `model_select` *veto*, no `before_compact` under that name (it is `session_before_compact`), no `resource_load` (it is `resources_discover`), no generic "hook returning `{decision: allow/deny}`" — pi uses `block` / `cancel` / `handled` per event.

### 3.1 Which of these matter now
Only as **observers of what other pi extensions can do to Aleph's tool calls**: any installed extension can block/rewrite a `bash` call (`tool_call`, `input.command` mutable) or rewrite the MCP result text (`tool_result`). pi-mcp-adapter itself reports proxy calls as `toolName:"mcp"` and direct tools under their prefixed name (`direct-tools.ts:294`).

## 4. Tools, commands, prompt templates, skills

### 4.1 Registered tools (`pi.registerTool`) — `types.ts:451-500`

| Field | Type | Meaning |
|---|---|---|
| `name`, `label`, `description` | string | `name` is the LLM-facing id; `label` for UI |
| `parameters` | `TSchema` (TypeBox) **or any JSON-Schema object** | Loader only checks it is a non-array object (`loader.ts:288-292`). Validation in `pi-ai` accepts either: if the object lacks the TypeBox `Kind` symbol it goes through `coerceWithJsonSchema` first, then `Value.Check` (`packages/ai/src/utils/validation.ts:317-345`). ⇒ **a host does not need TypeBox on the wire; plain JSON Schema is a first-class input.** |
| `promptSnippet?`, `promptGuidelines?` | string / string[] | One-liner for the system prompt "Available tools" list + guideline bullets appended when active — this is pi's version of R9 "the tool owns its sentence" |
| `prepareArguments?` | `(unknown) => Static<TParams>` | pre-validation shim (`agent/src/agent-loop.ts:592-604`) |
| `executionMode?` | `"sequential"\|"parallel"` | per-tool concurrency override |
| `constrainedSampling?`, `renderShell?` | | provider-side constrained decoding; TUI framing |
| `execute(toolCallId, params, signal, onUpdate, ctx)` | → `Promise<AgentToolResult<TDetails>>` | `onUpdate(partial)` streams partial results (`tool_execution_update`); `ctx` is the full `ExtensionContext` (so tools can open dialogs, exec, read session) |
| `renderCall?` / `renderResult?` | `(args\|result, theme, ToolRenderContext) => Component` | TUI only |

`AgentToolResult<T> = {content: (Text|Image)[], details: T, usage?, addedToolNames?, terminate?}` (`packages/agent/src/types.ts:362-378`). Extension tools are wrapped so that if `execute` widened the active tool set via `setActiveTools`, the added names are reported in `addedToolNames` (`src/core/extensions/wrapper.ts:19-38`) — this is how "plan mode → build mode" tool unlocks travel with the transcript.

### 4.2 Slash commands — three sources, one namespace

`SlashCommandInfo.source ∈ {"extension","prompt","skill"}` (`src/core/slash-commands.ts:5-11`); 23 builtins listed at `:19-43`.

| Source | Discovered from | Name | Expansion |
|---|---|---|---|
| extension command | `pi.registerCommand(name, {handler})` | `/name` | handler runs in-process with `ExtensionCommandContext`; **not** sent to the LLM unless the handler does so |
| prompt template | `~/.pi/agent/prompts/*.md`, `.pi/prompts/*.md`, package `prompts/` or `pi.prompts`, settings `prompts[]`, `--prompt-template` | `/<filename-sans-.md>` | frontmatter `description`, `argument-hint`; body with `$1 $2 $@ $ARGUMENTS ${N:-default} ${@:N} ${@:N:L}` substitution (`src/core/prompt-templates.ts:59-103`); result becomes the user message |
| skill | `~/.pi/agent/skills/**/SKILL.md`, `.pi/skills/`, `~/.agents/skills/`, ancestor `.agents/skills/`, package `skills/` or `pi.skills`, plus `resources_discover` contributions | `/skill:<name>` | body wrapped as `<skill name="…" location="…">Refs relative to <baseDir> …</skill>` + args appended (`src/core/agent-session.ts:1362-1386`) |

### 4.3 Skills (`src/core/skills.ts`, `docs/skills.md`)

- Format = Agent Skills spec: YAML frontmatter `name` (≤64, `^[a-z0-9-]+$`, no leading/trailing/double hyphen — `skills.ts:96-117`), `description` (≤1024, required — `:122-132`), optional `license`, `compatibility`, `metadata`, `allowed-tools` (documented "experimental"; I found **no consumer** of it in `src/core` — treat as inert), `disable-model-invocation` (hide from prompt; only `/skill:name` invokes).
- pi deliberately **does not** require `name == parent dir` (`docs/skills.md:141`).
- Discovery walks dirs honoring `.gitignore`/`.ignore`/`.fdignore` (`skills.ts:16,49-66`); first `SKILL.md` wins per dir; name collisions keep the first found and emit a diagnostic (`docs/skills.md:189`).
- Prompt injection: `<available_skills><skill><name/><description/><location/></skill>…</available_skills>` appended to the system prompt **only when a `read` or `bash` tool is active** (`src/core/system-prompt.ts:46,66-67`; `skills.ts:355-383`). The model then `read`s the file itself — identical to Aleph's/CC's progressive disclosure.

### 4.4 Themes
`themes/*.json` or `pi.themes` — pure TUI concern, ignore for a host.

## 5. Isolation & lifecycle (short)

No scope/disposable model (nothing Cordis-like). An extension is a record of Maps (`loader.ts:527-547`); the factory returns nothing. Every factory **re-runs per session** (`/new`, `/resume`, `/fork` → `session_shutdown{reason}` → `runtime.invalidate()` so captured `pi`/`ctx` throw → re-run → `session_start`, `agent-session-runtime.ts:392-413`, `loader.ts:209-218`); `/reload` bumps a cache generation and re-imports TS from disk (`resource-loader.ts:388-393`). Only `session_shutdown` exists for cleanup; module-level state, timers, child processes and `process.on` listeners leak unless the extension cleans up (only `pi.events` subscriptions are tracked, `loader.ts:219-230`). Everything runs in-process with full Node; the sole gate is *project trust* (untrusted cwd ⇒ `.pi/` resources skipped, `package-manager.ts:2398-2420`). No API-version check: the loader aliases both `@earendil-works/*` and legacy `@mariozechner/*` names to the same embedded modules (`loader.ts:56-72`).

## 6. `pi-cc-extensions` specifically — what it is and is not

Repo: `/Volumes/TBU4/Github/pi-cc-extensions` (npm `pi-cc-extensions` 0.8.70, `package.json:1-72`). One entry `extensions/index.ts` (32 lines) that installs ~10 sub-features behind a JSON config at `~/.pi/agent/claude-code-style.json` (`README.en.md:53-96`).

**It emulates Claude Code's *presentation* and a few *conveniences*; it does not load, parse, or execute any Claude Code plugin artifact** (no `hooks.json`, no `settings.json`, no `.claude/commands`, no `.claude/agents`, no `plugin.json`, no `statusline` script, no MCP config). Grep for `.claude`, `hooks.json`, `plugin.json`, `PreToolUse` across the repo: **0 hits**.

| CC concept | Does pi-cc-extensions emulate it? | How (file) | pi API used |
|---|---|---|---|
| Tool-call card look (summaries, expand/collapse, rich edit/write diffs) | ✅ look only | `extensions/renderer/{default-mode,compact-mode}.ts`, `renderer/tool/diff/*` (adapted from `pi-tool-display`) | `pi.on(tool_execution_*)`, `pi.on(message_*)`, `registerEntryRenderer`, direct `pi-tui` components (37 files import `@earendil-works/pi-tui`) |
| Thinking title / collapsed thinking | ✅ look | `feature/compact-thinking.ts` | `message_update` |
| `/context` breakdown | ✅ (CC's `/context`) | `feature/context.ts` — re-computes system prompt / skills / tools / messages token estimate with `estimateTokens`, `formatSkillsForPrompt` from the coding-agent package | `registerCommand("context")`, `getAllTools`, `ctx.ui.custom` |
| `/clear`, `/exit` aliases | ✅ | `feature/shell/aliases.ts:13-26` → `ctx.newSession()` / `ctx.shutdown()` | `registerCommand` |
| Startup banner + tips; "Working…" footer with tokens/elapsed | ✅ look | `feature/shell/{startup-header,working-message}.ts` | `ctx.ui.setHeader`, `setWidget`, `setStatus` |
| `@agent` autocomplete + delegation hint | ⚠️ partial | `feature/reference/subagent.ts:18-25` reads `~/.pi/agent/agents/*.md` (frontmatter `display_name/description/model/thinking`) — that directory is **`@tintinweb/pi-subagents`'s format, not CC's `.claude/agents`** | `ctx.ui.addAutocompleteProvider` |
| `@session:` reference (inject prior session context) | pi-only feature | `feature/reference/session.ts` | `sendMessage`, `appendEntry` |
| Per-turn tool summary | pi-only | `feature/agent-summary/*` | `turn_end`, `registerMessageRenderer` |
| Mermaid / admonition markdown | pi-only | `renderer/markdown-enhance.ts` | `registerMarkdownTransformer` |
| Themes `cc-dark`/`cc-light` | ✅ look | `themes/*.json` | manifest `pi.themes` |
| **Hooks** (`PreToolUse`… as shell commands) | ❌ | — | — |
| **Commands** (`.claude/commands/*.md`) | ❌ (pi has its own `prompts/*.md`) | — | — |
| **Agents** (`.claude/agents/*.md`) | ❌ (see `@agent` row) | — | — |
| **Skills** (`SKILL.md`) | n/a — pi core already loads them natively (§4.3), same spec as CC | — | — |
| **settings.json / permissions / MCP** | ❌ | — | — |
| **statusline** | ❌ (its own footer widget) | — | — |

Its `pi.on` census: 38 subscriptions across 15 event names (`session_shutdown`, `session_start`, `before_agent_start`, `message_end`, `message_update`, `turn_start/end`, `tool_execution_start/end`, `agent_start/end`, `session_tree`, `session_compact`, `session_before_switch`, `resources_discover`) — i.e. it is a **pure observer + renderer**; it never blocks a tool, never edits context, registers exactly one tool.

**What this tells us about CC↔pi mapping cost**: nobody in the pi ecosystem has built the CC-plugin bridge; the closest artifacts are (a) pi core's native Agent-Skills loader (identical spec ⇒ free), (b) `prompts/*.md` ≈ `.claude/commands/*.md` (same `$ARGUMENTS`/`$1` substitution ⇒ near-free; frontmatter key names differ: `argument-hint` both, but CC's `allowed-tools`/`model` have no pi consumer), (c) `~/.pi/agent/agents/*.md` from `pi-subagents`/`pi-herdr-agents` ≈ `.claude/agents/*.md` (frontmatter `name/description/model/thinking/tools/system-prompt` — `pi-herdr-agents/agents/adversarial-reviewer.md:1-9`; CC uses `name/description/model/tools/color` ⇒ cheap adapter), (d) CC **hooks** ↔ pi `tool_call`/`tool_result`/`input`/`before_agent_start`/`session_*` events ⇒ semantically 1:1 (§3.1) but CC hooks are *shell commands with JSON on stdin*, pi hooks are *in-process JS closures* — that is the expensive direction (§7).

## 7. Hosting pi extensions in Rust — verdict (superseded, kept for the record)

Not pursued. If it ever is: the only R3-compatible route is a **Node sidecar** that reuses pi's exported `loadExtensions` + `ExtensionRunner.bindCore(fn-bags)` (`runner.ts:302-360`, `src/core/extensions/index.ts:7-21`) over JSON-RPC with pi's RPC-mode UI stubs copied verbatim; ~6/8 real packages would be materially useful, 7/8 ship raw TypeScript (needs `jiti`), 1/8 needs a native addon, and the TUI-only surface stays stubbed forever. Embedding a JS engine in `alephcore` violates R3, cannot load native addons, and would require a second copy of `runner.ts` chaining semantics. The declarative half (skills, prompt templates) is free either way — and is exactly what §9.2 uses.

## 9. pi as an MCP host + as a CLI caller

> Scope change 2026-09-20: Aleph will not host pi extensions. Integration direction is **pi → Aleph**: pi mounts Aleph as an MCP server and/or calls `aleph-cli` from its `bash` tool. This section is the contract Aleph must satisfy on both routes.

### 9.0 The single most important fact: pi core has **no MCP client**

`packages/coding-agent/README.md:499`: "**No MCP.** Build CLI tools with READMEs (see Skills), or build an extension that adds MCP support." `docs/usage.md:309` repeats it. `rg -i mcp packages/coding-agent/src` → 1 hit, a comment (`src/utils/tool-result-images.ts:15`). MCP in pi is delivered **exclusively** by the third-party extension **`pi-mcp-adapter`** (npm, v2.34.0 installed at `~/.pi/agent/npm/node_modules/pi-mcp-adapter`, 28,972 lines TS, deps `@modelcontextprotocol/client` 2.0.0 + `core` 2.0.0). It is the de-facto standard: the user's own `~/.pi/agent/settings.json` lists it, and pi-cc-extensions' README recommends it. **All of 9.1 describes pi-mcp-adapter, not pi.** A pi user without it cannot mount Aleph via MCP at all — the CLI route (9.3) is the only zero-prerequisite path.

### 9.1 How pi (via pi-mcp-adapter) consumes MCP servers

**Config files** (later wins; `README.md:60-75`, loader `config.ts`):

| # | File | Role |
|---|---|---|
| 1 | `~/.config/mcp/mcp.json` | user-global shared |
| 2 | `~/.agents/mcp.json`, 3 `~/.agents/mcp/mcp.json` | tool-agnostic compat inputs |
| 4 | `~/.pi/agent/mcp.json` (`$PI_CODING_AGENT_DIR/mcp.json`) | pi global override + host-config imports |
| 5 | `.mcp.json` (project) | **the file Aleph should document** — same shape Claude Code / Cursor use |
| 6 | `.pi/mcp.json` | pi project override (`/mcp enable|disable` writes only `disabled` here) |
| — | Claude plugin dirs via `claudePlugins:[{path, mcp:true, skills:true}]` | reads only the plugin root `.mcp.json` + `skills/**/SKILL.md`; expands `${CLAUDE_PLUGIN_ROOT}`; **never runs plugin hooks** (`README.md:145-160`, `claude-plugin-loader.ts:1-40`) |
| — | pi package manifest `package.json#pi.mcp: "./mcp.json"` | servers auto-prefixed `<sanitized-pkg>__<server>` (`README.md:164-176`) |
| — | Cursor / Claude Code / Codex host configs | detected by `/mcp setup`; **not loaded** unless `settings.hostConfigDiscovery:"on"` |

**Server entry schema** (`README.md:277-330`, types `types.ts:~440-560`):

```jsonc
{ "mcpServers": { "aleph": {
    "command": "aleph-mcp", "args": ["--stdio"], "env": {"ALEPH_TOKEN": "${ALEPH_TOKEN}"}, "cwd": "~",   // stdio
    // or: "url": "http://127.0.0.1:8787/mcp", "headers": {"Authorization": "Bearer ${ALEPH_TOKEN}"},   // StreamableHTTP, SSE fallback
    // or: "socket": "/tmp/aleph.sock"                                                                   // rmcp-mux unix socket
    "auth": "bearer" | "oauth", "bearerToken" | "bearerTokenEnv" | "bearerTokenStore", "oauth": {...},
    "lifecycle": "lazy" | "eager" | "keep-alive" | "lazy-keep-alive",   // default lazy: connects on first tool call
    "idleTimeout": 10, "requestTimeoutMs": 30000, "protocolVersion": "legacy" | "auto" | "2026-07-28",
    "exposeResources": true, "directTools": true | ["name"] | false | "search", "toolPrefix": "server"|"short"|"none"|"mcp",
    "includeTools": [...], "excludeTools": [...], "approveTools": [...], "searchKeywords": {"tool": ["kw"]},
    "inheritEnv": true, "disabled": false, "debug": false, "trace": false } },
  "settings": { "toolPrefix": "server", "directTools": false, "sampling": true, "elicitation": true, "outputGuard": true, ... } }
```
Env/header values support `${VAR}`, `$env:VAR`, `~`, and a leading `!cmd` that is executed at connect time (10 s, 1 MiB, `README.md:335-337`).

**How the server's surface reaches the model** (this is where pi differs from Claude Code / Aleph):

| MCP thing | Default exposure | Where |
|---|---|---|
| tools | **not** registered individually. One proxy tool `mcp` (~200 tokens) with params `{tool, args, connect, describe, instructions, search, regex, includeSchemas, limit, offset, server, action, url, target}`; the model must `mcp({search})` → `mcp({describe})` → `mcp({tool, args})` | `index.ts:1648-1680`; description text `direct-tool-surface.ts:buildProxyDescription` (lists `Servers: a, b` + usage lines) |
| tools, opt-in | `directTools: true|[names]` registers them as first-class pi tools (schema passed through, `strictDirectToolArguments` optional); `"search"` registers them inactive until a search hit | `README.md:627-742`; `direct-tools.ts` |
| tool names | `<prefix>_<tool>` with `.`→`_` in the tool name; prefix = server name with chars outside `[A-Za-z0-9_-]` hex-escaped (`_2e_`, `_20_`); `short` strips `-mcp`; `mcp` mode → `mcp__<server>_<tool>`. **Measured**: `aleph`+`take.screenshot` → `aleph_take_screenshot`; `aleph-mcp` short → `aleph_take_screenshot`; `Aleph Core` → `Aleph_20_Core_take_screenshot` | `types.ts:787-812` (README's `chrome_devtools_…` example is stale vs. code — hyphens are preserved) |
| resources | exposed as **tools** `read_<sanitized-resource-name>` (lower-cased, non-alnum→`_`) unless `exposeResources:false` | `resource-tools.ts:2-14`; `direct-tool-surface.ts:123-161` |
| prompts | registered as pi slash commands `/mcp__<server>__<prompt>` with positional / `key=value` args; result flattened to one user message | `README.md:609-619`; `types.ts:868-880` |
| `instructions` (from `initialize`) | **not** injected into the system prompt. Shown only on `mcp({server})` list (300-char preview) or `mcp({instructions:"aleph"})` | `server-manager.ts:974-985`; `proxy-modes.ts:30,835-841` |
| search results | name + 50-char description (compact) or full description + TS-shape of `inputSchema` (`includeSchemas`, default true) | `proxy-modes.ts:780-800` |
| results | `content` text blocks → model; **output guard 50 KiB / 2,000 lines** (same as bash), overflow spilled to a temp file whose path is returned; images pass through; `structuredContent` kept in `details.mcpResult` (≤16 KiB) | `README.md:544-560`; `mcp-output-guard.ts` |
| `isError` | direct tools: mapped to pi tool error; proxy: text with error marker | `direct-tools.ts:322` |
| schema sanitization (direct tools) | `inputSchema` passed through minus `$schema` and `additionalProperties`; a non-object schema becomes `{type:"object", properties:{}}`; args accepted as object **or** JSON string (`normalizeToolArguments`), optional strict validation via `strictDirectToolArguments`; proxy mode does **no** validation ("MCP server validates arguments, not the adapter", `README.md:906`) | `utils.ts:337-380` |
| reconnect / backoff | `lazy`: connect on first call, idle-disconnect after `idleTimeout` (10 min), reconnect on next use; a failed connect enters a **60 s failure backoff** (`FAILURE_BACKOFF_MS`) during which calls fail fast; `eager`: no auto-reconnect; `keep-alive`/`lazy-keep-alive`: health check every **30 s** (`tools/list` or `ping`, 5 s cap, 10 servers concurrently), retry backoff 30 s → 5 min, full reconnect only when the HTTP session is proven expired; reconnect skipped while OAuth is pending | `failure-backoff.ts:3`; `lifecycle.ts:14-16,93-119,170-252`; `server-manager.ts:220`; `README.md:437-447` |

**Protocol / capabilities**:
- Default `protocolVersion:"legacy"` = classic `initialize` via MCP SDK v2 core: `LATEST_PROTOCOL_VERSION "2025-11-25"`, `DEFAULT_NEGOTIATED "2025-03-26"`, supported `["2025-11-25","2025-06-18","2025-03-26","2024-11-05","2024-10-07"]` (`@modelcontextprotocol/core/dist/auth-*.mjs:4-12`). `"auto"` probes 2026-07-28 (`server/discover`) with fallback; `"2026-07-28"` pins (`README.md:318-326`, `server-manager.ts:128-142`).
- Client capabilities advertised: `sampling:{}` (if enabled), `elicitation:{form:{}, url?:{}}`, extension `io.modelcontextprotocol/ui` (`server-manager.ts:1128-1145`). **No `roots`** (README: "adapter-level roots support … not yet implemented").
- **Sampling**: supported, text-only, routed through pi's model registry honoring `modelPreferences.hints`; needs `settings.samplingAutoApprove` in headless sessions (`sampling-handler.ts:1-40`, `README.md:911-915`).
- **Elicitation**: form mode via pi `select/input` dialogs; url mode TUI-only with consent (`elicitation-handler.ts`, `README.md:621-626`).
- **Tool approval**: `approveTools` globs → Allow once / for session / Deny; headless ⇒ fail-closed `approval_required`; other extensions can claim via `pi.events.on("pi-mcp-adapter:tool-approval-request")` (`README.md:509-543`).
- **No MCP "hooks"** concept exists on either side; pi's own `tool_call`/`tool_result` events (§3) fire with `toolName:"mcp"` for proxy calls and with the prefixed name for direct tools (`direct-tools.ts:294`).
- Lifecycle: lazy connect on first call, idle disconnect after 10 min, metadata cached on disk so `search/describe` work offline; each pi session spawns its **own** stdio server process (no cross-session sharing, `README.md:912`).

**What Aleph's MCP server must therefore do to be a good citizen in pi**: (1) keep tool `description`s front-loaded — only the first 50 chars survive compact search; (2) put usage guidance in tool descriptions, **not** in `instructions` (never auto-shown); (3) keep results ≤ 50 KiB or expect spill-to-file; (4) `tools/list` is cached on disk between sessions; advertise `tools.listChanged` and send `notifications/tools/list_changed` for dynamic sets — the client subscribes to tools/resources/prompts changes (`server-manager.ts:1157-1172`) and refreshes keep-alive catalogs on health checks (`README.md:907`); (5) speak 2025-03-26/2025-06-18 legacy handshake by default; (6) name the server `aleph` (yields `aleph_<tool>` / `/mcp__aleph__<prompt>`); (7) do not rely on `roots` or on the client honoring `_meta` progress; (8) the client identifies as `{name:"pi-mcp-<server>", version:"1.0.0"}` (`server-manager.ts:1151-1152`) — Aleph can key pi-specific behaviour on that `clientInfo.name` prefix.

### 9.2 Skills, prompt templates, AGENTS.md — the teaching surface

Full detail in §4.2-4.3; the parts relevant to shipping an "how to call aleph-cli" skill:

| Surface | Discovery (in precedence order) | Format | Reaches the model how |
|---|---|---|---|
| **Skill** | `.pi/skills/**/SKILL.md` (trusted project), ancestor `.agents/skills/`, `~/.pi/agent/skills/`, `~/.agents/skills/`, package `skills/` or `pi.skills`, `resources_discover` from extensions (`package-manager.ts:2398-2510`) | Agent-Skills frontmatter: `name` (`^[a-z0-9-]+$`, ≤64), `description` (≤1024, required), `disable-model-invocation` (bool); `license/compatibility/metadata/allowed-tools` parsed-but-inert (`skills.ts:96-132`) | `<available_skills><skill><name/><description/><location/></skill></available_skills>` appended to the system prompt **only when `read` or `bash` is active** (`system-prompt.ts:46,66-67`, `skills.ts:355-383`); model `read`s the file itself; user can force with `/skill:<name> args` (`agent-session.ts:1362-1386`) |
| **Prompt template** | `.pi/prompts/*.md`, `~/.pi/agent/prompts/*.md`, package `prompts/`, settings `prompts[]`, `--prompt-template` | frontmatter `description`, `argument-hint`; body with `$1 $@ $ARGUMENTS ${N:-def} ${@:N} ${@:N:L}` (`prompt-templates.ts:59-103`) | becomes the **user** message when typed as `/<name> args`; not visible otherwise |
| **Context file** | one per dir, first match of `AGENTS.override.md, AGENTS.md, AGENTS.MD, CLAUDE.md, CLAUDE.MD` in `~/.pi/agent/` then every ancestor of cwd root→cwd (worktree-shadow aware) (`resource-loader.ts:71-156`) | plain markdown | `<project_context><project_instructions path="…">…</project_instructions></project_context>` in the system prompt (`system-prompt.ts:151-158`) |
| **Package** | `pi install npm:<pkg>` / `git:` → `settings.packages[]`; a package with only `skills/` + `prompts/` and **no `extensions`** is legal (`package-manager.ts:2188-2200`) | see §1.3 | |

Cheapest Aleph deliverable: an npm/git package `pi-aleph` with `"pi": {"skills": ["./skills"], "prompts": ["./prompts"]}` (no JS), containing `skills/aleph-cli/SKILL.md` (description ≤1024 chars saying *when* to reach for `aleph-cli`) and optional `prompts/aleph.md`. Because a skill's `<location>` is an absolute path the model reads on demand, the SKILL.md can be long (progressive disclosure identical to CC/Aleph). Note the frontmatter `name` need not match the directory (`docs/skills.md:141`) and collisions keep the first found (`:189`). If Aleph also ships MCP, add `"mcp": "./mcp.json"` to the same manifest — pi-mcp-adapter reads it and prefixes the server `pi_aleph__aleph` (hyphen→underscore, `types.ts:527-539`; `README.md:164-176`), or instruct users to put it in `.mcp.json` to keep the bare `aleph` prefix.

### 9.3 What pi's `bash` tool expects from a well-behaved CLI

`src/core/tools/bash.ts` (shell selection `src/utils/shell.ts:67-135`, limits `src/core/tools/truncate.ts:11-12`):

| Aspect | pi behaviour | Consequence for `aleph-cli` | Where |
|---|---|---|---|
| Invocation | `spawn(shell, ["-c", command])`; shell = `settings.shellPath` → bash/zsh from `$SHELL` → `sh`; Windows: Git Bash / WSL (`-s` via stdin); optional `settings.shellCommandPrefix` prepended | one-shot, non-login `-c` — don't rely on interactive rc files or aliases | `bash.ts:93-98`; `shell.ts:21,67-119` |
| stdin | `stdio: ["ignore", "pipe", "pipe"]` — **stdin is closed** | **never prompt**; detect `!isatty(0)` / EOF and fail fast with a usage error instead of hanging; no confirmations | `bash.ts:98` |
| TTY | pipes, no pty | `isatty(1)` is false — **disable colour/spinners/progress bars**; ANSI in stdout reaches the model **verbatim** (only the TUI strips it, `render-utils.ts:48`); output-accumulator does no sanitising | `bash.ts:120-122` |
| stdout/stderr | both piped into **one** accumulator in arrival order — no separation, no labels | put machine output on stdout and keep stderr short; a chatty stderr interleaves into the JSON the model reads | `bash.ts:121-122` |
| Exit code | `!= 0` ⇒ `throw Error(output + "\n\nCommand exited with code N")` ⇒ tool result `isError:true` **but the captured output is still delivered** | use non-zero for real failures only; the model reads the text either way | `bash.ts:361-364` |
| Timeout | **none by default** ("no default timeout", `bash.ts:39`); model may pass `timeout` seconds (max 2^31-1 ms); on expiry the **whole process tree** is killed and text ends with `Command timed out after N seconds`; abort (Esc) ⇒ `Command aborted` | long operations must stream progress lines (see next row) or be resumable; a silent 10-minute job looks hung to the user | `bash.ts:21-34,107-137,349-357` |
| Streaming | stdout chunks are pushed to the TUI throttled by `BASH_UPDATE_THROTTLE_MS` | line-buffered progress on stdout is visible live; block-buffered output appears only at exit | `bash.ts:261-292`; `renderers/bash.ts` |
| Truncation | keeps the **last** 2,000 lines / 50 KiB (whichever first); full output written to `$TMPDIR/pi-bash-*`; footer `[Showing lines A-B of N. Full output: <path>]`; a partial last line is trimmed at a byte boundary | **head matters less than tail** (opposite of Claude Code); summaries/status belong at the **end**; keep default output well under 50 KiB, offer `--json`/`--limit` | `truncate.ts:1-12`; `bash.ts:317-333`; `output-accumulator.ts` |
| JSON detection | **none** — output is plain text; no schema, no auto-parse | JSON is fine but must be the only thing on stdout; pretty-print (line-oriented) so tail-truncation still yields valid-looking fragments | — |
| Environment | inherits shell env **plus** `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL` (default on; `exposeSessionEnvironment`) | `aleph-cli` can **detect it is inside pi** and which model/session is driving — useful for provenance / rate decisions | `bash.ts:165-190` |
| cwd | session cwd (`ctx.cwd`) | resolve paths relative to `$PWD`; print absolute paths in output | `bash.ts:247-253` |
| Hooks | `tool_call` event fires with `toolName:"bash"`, `input.command` mutable in place; any installed extension may block or rewrite it | nothing to do; be greppable (`aleph-cli <verb>` prefix) so allow/deny globs in third-party gates can match | §3 |
| Model guidance | system prompt only says "Execute bash commands (ls, grep, find, etc.)" + "inspect PI_* env vars" | the model will not know `aleph-cli` exists unless a skill/AGENTS.md says so (9.2) | `bash.ts:42-45` |

`!cmd` / `!!cmd` typed by the **user** in the editor goes through the same executor via the `user_bash` event (`types.ts:849-857`) — same constraints, plus `!!` output is excluded from the LLM context.

## 10. One-screen summary for the lead

1. **pi core has no MCP client** (`README.md:499`). MCP = `pi-mcp-adapter` (third-party, 29k LOC, installed on this machine). Its default exposes **one proxy tool `mcp`** — Aleph's tools are found via `mcp({search})` and called via `mcp({tool, args})` unless the user sets `directTools`. Server `instructions` are **never** auto-injected; tool descriptions are the only guidance channel (first 50 chars in compact search).
2. **Config** = standard `.mcp.json` / `~/.config/mcp/mcp.json` (`mcpServers.<name>.{command,args,env|url,headers|socket, auth, lifecycle, directTools, toolPrefix,…}`); pi overrides in `~/.pi/agent/mcp.json` and `.pi/mcp.json`. Tool names `<server>_<tool>`, prompts `/mcp__<server>__<prompt>`, resources `read_<name>`. Protocol: SDK v2, default legacy handshake (2025-03-26 negotiated, 2025-11-25 latest), `"auto"`/`"2026-07-28"` opt-in; sampling (text-only) + form elicitation + `listChanged` supported; no `roots`. Client name `pi-mcp-<server>`.
3. **Teaching surface** = a JS-free pi package `{"pi":{"skills":["./skills"],"prompts":["./prompts"],"mcp":"./mcp.json"}}`; SKILL.md is Agent-Skills spec, listed in the system prompt only when `read`/`bash` is active, read on demand; AGENTS.md/CLAUDE.md up the ancestor chain go into `<project_context>`.
4. **`bash` contract**: `sh -c`, **stdin closed**, no TTY, stdout+stderr **merged**, no default timeout, non-zero exit = error but output still delivered, **tail-truncation** 2,000 lines / 50 KiB with spill file, ANSI reaches the model verbatim, no JSON detection, `PI_SESSION_ID/PI_MODEL/…` env vars mark "inside pi".
5. `pi-cc-extensions` is a Claude-Code-*look* suite, not a CC-plugin bridge (§6). pi extensions themselves: TS factory modules loaded by `jiti`, 40 typed events, no isolation (§1–§5).

### Not done / caveats
- No pi or adapter code was executed except one `node -e` call to confirm tool-name formatting (§9.1); everything else is source reading. Line numbers: pi 0.85.1 (`71dca871`), pi-cc-extensions 0.8.70, pi-mcp-adapter 2.34.0 (as installed under `~/.pi/agent/npm/node_modules`, not a git checkout — its `README.md`/`*.ts` cites are to that copy). The user's running pi is 0.86.0 (`settings.json#lastChangelogVersion`); I did not diff 0.85.1→0.86.0.
- pi-mcp-adapter's `config.ts` (1,467 lines) schema was read via README + `types.ts`, not exhaustively; `claudePlugins`/`agentPluginPaths` loaders were skimmed for shape only.
- `allowed-tools` skill frontmatter: 0 consumers in `packages/coding-agent/src/` — documentation-only.
- Did not test how `mcp({search})` ranks Aleph-style tool names; `searchKeywords` per-server config exists if ranking is poor.
- Windows shell path (Git Bash / WSL `-s` stdin transport) not exercised; stdin-closed claim verified only for the `argv` transport (`bash.ts:98`).
