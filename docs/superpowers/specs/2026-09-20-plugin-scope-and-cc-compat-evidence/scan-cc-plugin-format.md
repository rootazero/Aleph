# Claude Code Plugin Format — Ground Truth Scan

Sources used (cited inline per claim):
- **[disk:plugin-dev-skill]** = `~/.claude/plugins/cache/claude-plugins-official/plugin-dev/c447c3207a42/skills/{plugin-structure,hook-development,command-development,agent-development,mcp-integration,plugin-settings,skill-development}/SKILL.md` — official first-party skill docs, installed version `c447c3207a42` (a git-commit-sha-as-version pseudo-tag), lastUpdated 2026-09-20 per `installed_plugins.json`.
- **[disk:installed]** = real installed plugin files under `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/` on this machine.
- **[disk:settings]** = `~/.claude/settings.json` (this user's real config, live).
- **[disk:marketplace]** = `~/.claude/plugins/marketplaces/claude-plugins-official/.claude-plugin/marketplace.json` (live official marketplace index) and per-plugin `.claude-plugin/marketplace.json` (dev-marketplace self-listing found inside superpowers).
- **[web:docs]** = fetched from `code.claude.com/docs/en/...` (cited with exact URL when used).

---

## 1. Directory layout & discovery

**Plugin root layout** (canonical, from `plugin-structure/SKILL.md` [disk:plugin-dev-skill]):

```
plugin-name/
├── .claude-plugin/
│   └── plugin.json          # Required: Plugin manifest
├── commands/                 # Slash commands (.md files)
├── agents/                   # Subagent definitions (.md files)
├── skills/                   # Agent skills (subdirectories)
│   └── skill-name/
│       └── SKILL.md         # Required for each skill
├── hooks/
│   └── hooks.json           # Event handler configuration
├── .mcp.json                # MCP server definitions
└── scripts/                 # Helper scripts and utilities
```

Critical rules stated verbatim in the skill doc:
1. `plugin.json` **must** live in `.claude-plugin/` (a dotdir at plugin root).
2. All component dirs (`commands/`, `agents/`, `skills/`, `hooks/`) **must** be at plugin root level, **not** nested inside `.claude-plugin/`.
3. Only create dirs for components actually used (all optional except the manifest with at least `name`).
4. kebab-case naming convention for all dirs/files.

Confirmed on disk for real installed plugins [disk:installed]:
- `superpowers@claude-plugins-official` v6.3.0: has `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json` (self-listing, see §1 marketplace notes below), `commands/` (none actually — superpowers ships no commands dir in the listing captured, mainly `skills/`), `skills/<name>/SKILL.md` per skill (e.g. `skills/using-git-worktrees`, `skills/test-driven-development`, `skills/systematic-debugging`, `skills/using-superpowers`), `hooks/hooks.json`, `hooks/scripts/` — note this plugin ALSO ships non-Claude-Code integration dirs side by side: `.kimi-plugin/`, `.cursor-plugin/`, `.opencode/`, `.devin-plugin/`, `.hermes-plugin/`, `.codex-plugin/`, `.pi/`, `.agents/`, `AGENTS.md`, `GEMINI.md`, `CLAUDE.md` — i.e. one repo cross-publishes to multiple agent-harness plugin ecosystems; the Claude Code–relevant subset is only `.claude-plugin/` + `commands/`/`agents/`/`skills/`/`hooks/`/`.mcp.json`.
- `plugin-dev@claude-plugins-official` (version pinned to git sha `c447c3207a42`): `.claude-plugin/plugin.json`, `agents/*.md` (3 files: `agent-creator.md`, `skill-reviewer.md`, `plugin-validator.md`), `commands/create-plugin.md`, `skills/<name>/{SKILL.md,references/,examples/,scripts/,README.md}` (7 skills: `plugin-structure`, `hook-development`, `command-development`, `agent-development`, `mcp-integration`, `plugin-settings`, `skill-development`).
- `code-review@claude-plugins-official`: minimal plugin — `.claude-plugin/plugin.json` (only `name`, `description`, `author` — no `version` field present, confirming version is optional and can be tracked purely by the installer's git-sha pseudo-version instead), `commands/code-review.md`. No agents/skills/hooks/mcp — proves those dirs are fully optional.

**Version pseudo-tag note**: several officially-marketplace-distributed plugins (`plugin-dev`, `context7`, `feature-dev`, `code-review`) are installed with `"version": "c447c3207a42"` in `installed_plugins.json` — a 12-char git commit SHA prefix, not semver — while others use real semver (`superpowers` → `6.3.0`, `chrome-devtools-mcp` → `1.9.0`, LSP plugins → `1.0.0`). This means the installer accepts **either** a semver string **or** a git-sha string as the "version" identifying a cache directory; it is used as an opaque cache-key/directory-name, not strictly parsed as semver by the install-path logic. [disk:installed]

### `.claude-plugin/plugin.json` full field list

From `plugin-structure/SKILL.md` [disk:plugin-dev-skill], cross-checked against real files:

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | **yes** | kebab-case, unique across installed plugins, no spaces/special chars |
| `version` | string | no | "semantic versioning" recommended but see pseudo-tag note above — real files show git-sha strings accepted too |
| `description` | string | no | recommended |
| `author` | object `{name, email, url}` | no | `email`/`url` optional sub-fields |
| `homepage` | string (URL) | no | |
| `repository` | string (URL) | no | in real files sometimes a bare URL string (`superpowers`: `"repository": "https://github.com/obra/superpowers"`) |
| `license` | string | no | SPDX-style short string, e.g. `"MIT"` |
| `keywords` | string[] | no | discovery/categorization |
| `commands` | string \| string[] | no | custom path(s), supplements (does not replace) default `commands/` |
| `agents` | string \| string[] | no | custom path(s), supplements default `agents/` |
| `skills` | — | no | (skill-structure doc shows `commands`/`agents`/`hooks`/`mcpServers` as the explicitly-documented override keys; marketplace.json separately supports a plugin-level `skills` array of subpaths — see §1 marketplace notes) |
| `hooks` | string (path) | no | custom path to a hooks JSON file, supplements default `hooks/hooks.json` |
| `mcpServers` | string (path) | no | custom path to an MCP config file, supplements default `.mcp.json` |

Path-value rules for the above override fields (verbatim): must be relative to plugin root, must start with `./`, cannot be absolute, arrays supported for multiple locations. Custom paths **supplement** defaults — both load.

Real `plugin.json` examples on disk:
```json
// superpowers (has version, author object, homepage, repository as bare string, license, keywords)
{
  "name": "superpowers",
  "description": "Core skills library for Claude Code: TDD, debugging, collaboration patterns, and proven techniques",
  "version": "6.3.0",
  "author": {"name": "Jesse Vincent", "email": "jesse@fsck.com"},
  "homepage": "https://github.com/obra/superpowers",
  "repository": "https://github.com/obra/superpowers",
  "license": "MIT",
  "keywords": ["skills","tdd","debugging","collaboration","best-practices","workflows"]
}

// code-review (minimal — no version, no homepage/repository/license/keywords)
{
  "name": "code-review",
  "description": "Automated code review for pull requests using multiple specialized agents with confidence-based scoring",
  "author": {"name": "Anthropic", "email": "support@anthropic.com"}
}
```

### `${CLAUDE_PLUGIN_ROOT}` semantics [disk:plugin-dev-skill]

- An environment variable available in hook commands, MCP server command/args, and referenceable in prose inside command/agent/skill markdown bodies.
- Purpose: portable absolute path to the plugin's install directory, because install location varies by installation method (marketplace/local/npm), OS, and user prefs.
- Must be used instead of: hardcoded absolute paths, relative paths from cwd (`./scripts/...`), or `~/`-relative paths.
- Confirmed in real files: superpowers `hooks/hooks.json` uses `"\"${CLAUDE_PLUGIN_ROOT}/hooks/run-hook.cmd\""` as the command string [disk:installed].

### Marketplace (`.claude-plugin/marketplace.json`) schema

Two real examples captured — a **live full marketplace index** and a **plugin's self-describing dev-marketplace**:

**A. Plugin-internal dev marketplace** (superpowers ships its own, presumably for local/dev installs) [disk:installed]:
```json
{
  "name": "superpowers-dev",
  "description": "Development marketplace for Superpowers core skills library",
  "owner": {"name": "Jesse Vincent", "email": "jesse@fsck.com"},
  "plugins": [
    {
      "name": "superpowers",
      "description": "...",
      "version": "6.3.0",
      "source": "./",
      "author": {"name": "Jesse Vincent", "email": "jesse@fsck.com"}
    }
  ]
}
```
Here `source: "./"` = plugin lives at the marketplace repo root (relative-path source variant).

**B. Live official marketplace** `claude-plugins-official` root [disk:marketplace] — top-level fields: `$schema` (`https://anthropic.com/claude-code/marketplace.schema.json`), `name`, `description`, `owner {name,email}`, `renames` (object mapping old-name → new-name, used for plugin rename/redirect history, e.g. `"adlc": "agentforce-adlc"`), `plugins[]`.

Per-plugin entry fields observed across dozens of real entries: `name` (required), `description`, `author {name[,email]}` (optional — several entries omit it entirely, e.g. `"ai-plugins"`, `"aikido"`), `category` (free string, e.g. `"security"`, `"design"`, `"development"`, `"database"`, `"productivity"`, `"monitoring"`, `"location"`), `homepage`, `displayName` (optional human-friendly override, seen on `airwallex-agentos`), `strict` (boolean, seen `false` on `amd-skills` — controls whether unknown fields are tolerated, see §8), `skills` (array of subpaths, e.g. `amd-skills` → `["./local-ai-use","./local-ai-app-integration","./serving-llms-on-instinct","./tracelens-analysis-orchestrator"]` — lets ONE marketplace `source` repo register multiple independently-named skill bundles under one plugin listing), and **`source`**, which has multiple shapes:
  - bare string, relative path into the marketplace's own repo: `"source": "./plugins/agent-sdk-dev"`, `"source": "./external_plugins/asana"` — i.e. source can literally be a subdirectory path when the marketplace repo itself vendors the plugin.
  - object, `{"source": "git-subdir", "url": "...", "path": "plugins/x", "ref": "v1.5.5", "sha": "<full 40-hex commit sha>"}` — pull a subdirectory of an external repo, pinned to a ref/tag AND a resolved commit sha (both present together in every observed `git-subdir` entry — `ref` is the human-readable pointer, `sha` is what's actually fetched/pinned).
  - object, `{"source": "url", "url": "...", "sha": "<40-hex sha>"}` — whole external repo (no `path`/no `ref` field observed on this variant; only `url`+`sha`).
  - (implied by `known_marketplaces.json` shape below, not seen inside `plugins[]` entries) `{"source": "github", "repo": "owner/name"}` — used for marketplace-level sources, not yet confirmed as a **plugin**-level source variant inside `marketplace.json`; flag as unconfirmed.

### `known_marketplaces.json` shape [disk:installed] (this is Claude Code's own tracking file, not part of the distributed plugin/marketplace format — infra, not spec)
```json
{
  "<marketplace-name>": {
    "source": {"source": "github", "repo": "<owner>/<repo>"},
    "installLocation": "/Users/.../plugins/marketplaces/<marketplace-name>",
    "lastUpdated": "<ISO8601>"
  }
}
```
All 5 real entries on this machine use `{"source": "github", "repo": ...}` — no git-URL or npm variant observed here (npm as a marketplace/plugin source is unconfirmed on-disk; flag for docs cross-check).

### `installed_plugins.json` shape [disk:installed] (schema `"version": 2` at file root)
```json
{
  "version": 2,
  "plugins": {
    "<plugin-name>@<marketplace-name>": [
      {
        "scope": "user",
        "installPath": "/Users/.../plugins/cache/<marketplace>/<plugin>/<version-or-sha>",
        "version": "<semver-or-git-sha>",
        "installedAt": "<ISO8601>",
        "lastUpdated": "<ISO8601>",
        "gitCommitSha": "<40-hex, only present when marketplace itself is git-tracked>"
      }
    ]
  }
}
```
Key = `"<plugin-name>@<marketplace-name>"` (composite identity — plugin names are only unique **within** a marketplace, matching the "must be unique across installed plugins" note in the skill doc being about the *combined* key in practice, since e.g. nothing here proves cross-marketplace collision handling). Value is an **array** — suggesting multiple scope-entries (e.g. user + project) could coexist per key, though every real entry here has exactly 1 element with `"scope": "user"`. `gitCommitSha` present only on marketplace-tracked (git-based) plugins, absent on... (actually present on all here since all are git-sourced; unconfirmed what a non-git install looks like).

### `lspServers` — marketplace-inline and plugin-level manifest field (not covered by plugin-dev skill docs, found by direct inspection)

Three real Anthropic-published plugins (`clangd-lsp`, `rust-analyzer-lsp`, `swift-lsp`, all v1.0.0, `strict: false`) declare their entire config **inline in the marketplace.json `plugins[]` entry** rather than shipping a `.claude-plugin/plugin.json` in the install cache at all — their installed cache dirs contain only `README.md` + `.in_use` marker, no manifest. This proves:
1. `lspServers` is a legitimate top-level key (confirmed present alongside `name`/`description`/`version`/`author`/`source`/`category`/`strict` in the marketplace entry), shape: `{"<server-id>": {"command": "<binary-name>", "args": ["<flag>", ...]?, "extensionToLanguage": {".<ext>": "<language-id>", ...}}}`.
2. A marketplace entry can carry enough config that **no separate plugin repo/cache manifest is required** for a pure-LSP plugin — the marketplace.json entry IS the plugin.json for these three. This is a second, undocumented (by the skill docs) route to defining a plugin, alongside the standard `.claude-plugin/plugin.json`-in-a-directory route.
3. `strict: false` on the marketplace entry is the same flag noted in §1's `amd-skills` entry — likely controls whether Claude Code tolerates fields/shapes it doesn't recognize for that plugin (unconfirmed semantics — inferred from field name and co-occurrence with unusual/extension entries only, not from any prose doc found).

### `enabledPlugins` in settings.json [disk:settings]
```json
"enabledPlugins": {
  "<plugin-name>@<marketplace-name>": true,
  ...
}
```
Flat boolean map, composite key identical format to `installed_plugins.json`. All 15 real entries on this machine are `true`; `false` (explicit disable without uninstall) is inferred supported but not observed. Also present in the same settings.json: `extraKnownMarketplaces` (object keyed by marketplace name, value `{"source": {...}}` — same source-object shape as `known_marketplaces.json`) — this is how a **user's own settings.json** (not the central plugins dir) can register additional marketplaces, presumably for portability of a settings.json across machines.

---

## 2. Commands (`commands/*.md`)

Source: `command-development/SKILL.md` [disk:plugin-dev-skill], cross-checked against real files (`code-review/commands/code-review.md`, `plugin-dev/commands/create-plugin.md`) [disk:installed].

**Important doc note (verbatim from skill):** `.claude/commands/` is called out as a **legacy format**; the skill explicitly recommends the `skills/<name>/SKILL.md` directory format for *new* work, stating "Both are loaded identically — the only difference is file layout." Implication for a compat layer: commands and skills are two file-layout variants of conceptually-similar-but-not-identical loading (commands are always-available slash-invoked; skills are autonomously triggered by description-matching — see §4). A host claiming compatibility should support commands as first-class, not just as a deprecated alias.

**Core semantic point (verbatim, called "Critical"):** Command markdown body content becomes literally Claude's instructions when invoked — written as directives TO the agent, not as user-facing descriptive text ("Review this code for X" not "This command will review your code for X").

### Locations & scope labels (shown in `/help`)
| Scope | Path | `/help` label |
|---|---|---|
| Project | `.claude/commands/` | `(project)` |
| Personal | `~/.claude/commands/` | `(user)` |
| Plugin | `plugin-name/commands/` | `(plugin-name)` |

### YAML frontmatter fields
| Field | Type | Default | Notes |
|---|---|---|---|
| `description` | string | first line of prompt body | shown in `/help`; best practice <60 chars |
| `allowed-tools` | string or array | inherits from conversation | patterns: exact list `Read, Write, Edit`; scoped-bash `Bash(git:*)`; wildcard `*` (rarely needed). Real file `code-review.md` uses a **single string** with multiple `Bash(gh ...:*)` scoped entries comma-joined, confirming string form with comma-separated scoped-bash entries is valid syntax, not just a plain array. |
| `model` | string enum | inherits | `sonnet`\|`opus`\|`haiku` (no `inherit` value documented for commands, unlike agents where `inherit` is a valid/recommended enum value) |
| `argument-hint` | string | none | shown for autocomplete, e.g. `[pr-number] [priority] [assignee]` |
| `disable-model-invocation` | boolean | `false` | prevents the SlashCommand tool from programmatically/model-invoking this command — real file `code-review.md` sets this explicitly to `false` (i.e. explicitly allowing model-invocation, which is also the default) |

Real `code-review.md` frontmatter observed:
```yaml
---
allowed-tools: Bash(gh issue view:*), Bash(gh search:*), Bash(gh issue list:*), Bash(gh pr comment:*), Bash(gh pr diff:*), Bash(gh pr view:*), Bash(gh pr list:*)
description: Code review a pull request
disable-model-invocation: false
---
```
Real `create-plugin.md` frontmatter observed — `allowed-tools` given as a YAML block-array (not a comma string) here, confirming **both syntaxes are valid**:
```yaml
---
description: Guided end-to-end plugin creation workflow with component design, implementation, and validation
argument-hint: Optional plugin description
allowed-tools:
  ["Read","Write","Grep","Glob","Bash","TodoWrite","AskUserQuestion","Skill","Task"]
---
```

### Dynamic content
- `$ARGUMENTS` — all args as one string.
- `$1`, `$2`, `$3`, … — positional args, combinable with `$ARGUMENTS` remainder-style usage (mixing shown in docs, not confirmed on a real installed file).
- `@path/to/file` — statically or dynamically (`@$1`) includes file contents; Claude reads the file before processing the command.
- `` !`shell command` `` — inline bash execution, runs before Claude processes the command, output substituted into the prompt; requires `Bash` (or scoped `Bash(...)`) in `allowed-tools`. Confirmed pattern used in doc examples: `` Files changed: !`git diff --name-only` ``.

### Namespacing
Subdirectories under `commands/` produce namespaced commands, shown in `/help` as e.g. `/helper (plugin:plugin-name:utils)` for `commands/utils/helper.md`. For project/personal namespaced commands the doc shows label format `(project:ci)` for `commands/ci/build.md`.

### Integration with other components
Commands can: instruct Claude to launch a plugin agent (agent "must exist in `plugin/agents/` directory", launched via the Task tool), mention a plugin skill by name to trigger it (skill "must exist in `plugin/skills/` directory"), and reference `${CLAUDE_PLUGIN_ROOT}`-rooted scripts/templates/config via `!` bash execution or `@` file reference.

---

## 3. Agents (`agents/*.md`)

Source: `agent-development/SKILL.md` [disk:plugin-dev-skill], cross-checked against real file `plugin-dev/agents/plugin-validator.md` [disk:installed].

**Conceptual distinction (verbatim):** "Agents are FOR autonomous work, commands are FOR user-initiated actions."

### Frontmatter fields
| Field | Required | Format | Notes |
|---|---|---|---|
| `name` | **yes** | lowercase, numbers, hyphens only; 3–50 chars; must start/end alphanumeric | e.g. `code-reviewer`; rejects `helper` (too generic — a style rule not a syntax rule), `-agent-` (edge hyphens), `my_agent` (underscore), `ag` (too short) |
| `description` | **yes** | prose, must state triggering conditions + a short prose summary of 2–4 example scenarios + a pointer to a body "When to invoke" section | doc calls this "the most critical field... loaded into context whenever the agent is registered". Real file uses YAML block-scalar (`description: \|`) containing full `<example>`/`<commentary>` blocks — i.e. the doc's own template (bullet-prose "When to invoke") and the real shipped agent's actual style (structured `<example>` XML-ish blocks reproduced inline in frontmatter) **diverge**: real file embeds full example transcripts directly in the YAML description value, not just a short summary + pointer. Flag this divergence explicitly — a compat layer must accept arbitrarily long multi-line YAML block-scalar descriptions containing embedded pseudo-XML example blocks, not just short prose. |
| `model` | doc says **yes**; real file has it | `inherit`\|`sonnet`\|`opus`\|`haiku` | `inherit` = same model as parent, recommended default. Real file: `model: inherit`. |
| `color` | doc says **yes**; real file has it | one of `blue`,`cyan`,`green`,`yellow`,`magenta`,`red` | real file: `color: yellow`. UI visual identifier only. |
| `tools` | no | array of tool-name strings | omit = full tool access (also `["*"]` for explicit full access). Real file: `tools: ["Read", "Grep", "Glob", "Bash"]`. |

### Body = system prompt
Second-person voice in the **doc's own prescriptive template** ("You are...", "You will..."), structured as: role/domain sentence → "Your Core Responsibilities" numbered list → "Analysis Process" steps → "Quality Standards" → "Output Format" → "Edge Cases". Length guidance: 500–3,000 chars typical, hard ceiling "under 10,000 characters" per doc. Real file (`plugin-validator.md`) matches this shape closely: opens "You are an expert plugin validator specializing in...", then "**Your Core Responsibilities:**" numbered list, then "**Validation Process:**" numbered/nested steps.

### Validation rules (from doc, not independently machine-verified against a live install path)
- name: 3–50 chars regex-shaped as above.
- description: 10–5,000 chars, best practice 200–1,000 with 2–4 examples.
- system prompt body: 20–10,000 chars, best practice 500–3,000.

### Namespacing
Single plugin → bare `agent-name`; with subdirectories → `plugin:subdir:agent-name` (same pattern family as command namespacing in §2).

---

## 4. Skills (`skills/<name>/SKILL.md`)

Source: `skill-development/SKILL.md` [disk:plugin-dev-skill], cross-checked against real user-level skills at `~/.claude/skills/*/SKILL.md` [disk:installed] (these are personal-scope skills, not plugin-bundled, but same file format per the doc's claim that plugin skills use "a simpler manual structure" of the identical `SKILL.md` contract).

### Anatomy
```
skill-name/
├── SKILL.md (required)        — YAML frontmatter (name, description — both required) + markdown instructions
└── (optional bundled resources)
    ├── scripts/     — executable code; deterministic/repeated tasks; may run WITHOUT being loaded into context
    ├── references/  — docs loaded into context on demand (keep SKILL.md lean by moving detail here)
    └── assets/      — files used in OUTPUT, not loaded into context (templates, logos, fonts, boilerplate)
```

### Frontmatter — REQUIRED fields only
| Field | Required | Notes |
|---|---|---|
| `name` | yes | plugin-dev doc's own template shows `Skill Name` (title-case example) but real installed skills (both plugin-bundled and personal) uniformly use kebab-case matching the directory name (e.g. `name: borges-style-writing`), matching `plugin-structure/SKILL.md`'s own frontmatter (`name: plugin-structure`). Treat kebab-case-matches-dirname as the real convention; the doc's "Skill Name" example is inconsistent with actual practice and every real file sampled. |
| `description` | yes | **must be third-person** ("This skill should be used when the user asks to...") **not** second-person ("Use this skill when you..."). Must contain concrete, quoted trigger phrases the user would literally say. This is explicitly graded: doc gives worked "Bad" examples (`"Use this skill when working with hooks."` — wrong person, vague; `"Load when user needs hook help."` — not third person; `"Provides hook guidance."` — no triggers) vs "Good" (full trigger-phrase list). |
| `version` | no | optional, seen in plugin-dev's own SKILL.md files (`version: 0.1.0`/`0.2.0`) but **absent** from every personal/user-level skill sampled — so it's a plugin-dev-team convention, not a hard requirement. |

No other frontmatter fields documented for skills (no `allowed-tools`/`disable-model-invocation`/`user-invocable` field found in either the skill-development doc's own examples or any real SKILL.md sampled — flag as **unconfirmed/likely does not exist** at the plugin-SKILL.md layer, though the task's search terms hypothesized these; do not assume `allowed-tools` on skills without further evidence).

### Progressive disclosure — the core design principle (verbatim 3-level model)
1. **Metadata (`name`+`description`)** — always resident in context (~100 words) for every installed skill, whether or not triggered.
2. **SKILL.md body** — loaded only when the skill triggers (target 1,500–2,000 words, hard-ish ceiling ~3,000/"<5k max").
3. **Bundled resources** (`references/`,`examples/`,`scripts/`) — loaded/executed only as needed, "Unlimited" because scripts can run without being read into context at all.

### Writing style requirement — imperative/infinitive, not second person
Explicit contrast given: "Start by reading the configuration file." (correct) vs "You should start by reading the configuration file." (incorrect). This applies to the **body**; the **frontmatter description** is prescribed **third person** (a *third* distinct voice register from the body's imperative — i.e. SKILL.md deliberately uses two different grammatical persons in two different parts of the same file, which a naive compat-checker might flag as inconsistent but is actually per-spec).

### Discovery mechanism (same as §1): scans `skills/` for subdirectories containing `SKILL.md`; loads metadata always, body on trigger, resources on demand. Distribution: **no packaging/zipping** — skills ship as plain files inside the plugin, available immediately on plugin install.

### Testing hook
`cc --plugin-dir /path/to/plugin` — local dev-mode plugin loading flag, confirmed only in doc prose (not independently verified against real CLI help output in this scan).

---

## 5. Hooks — full event list, schemas, contracts

Source: `hook-development/SKILL.md` [disk:plugin-dev-skill] (primary), cross-checked against real `superpowers/hooks/hooks.json` [disk:installed] and this user's own live `~/.claude/settings.json` `hooks` block [disk:settings] (which is itself a **currently-executing real example** of the settings-format).

### Two wrapper formats — this is a real, load-bearing distinction, not a doc nuance
| Context | Format | Real example |
|---|---|---|
| Plugin `hooks/hooks.json` | `{"description": "...", "hooks": {"<Event>": [...]}}` — event map nested one level under a required `hooks` key | `superpowers/hooks/hooks.json` [disk:installed]: `{"hooks": {"SessionStart": [{"matcher": "startup\|clear\|compact", "hooks": [{"type":"command","command":"\"${CLAUDE_PLUGIN_ROOT}/hooks/run-hook.cmd\" session-start","shell":"bash","async":false}]}]}}` |
| User `.claude/settings.json` | `{"<Event>": [...]}` — events directly at the top level of the `hooks` object, **no** wrapper, **no** `description` field | this user's live settings.json [disk:settings]: `"hooks": {"PreToolUse": [...], "Notification": [...], "Stop": [...]}` |

Additional fields observed on real hook entries **not mentioned in the SKILL.md's own JSON examples**: `"shell": "bash"` (explicit shell selection for the command) and `"async": false` (explicit sync/async execution flag) on the superpowers `SessionStart` hook, and `"statusMessage": "cargo serialize gate"` on this user's own `PreToolUse` hook for `cargo-gate.sh`. Neither `shell`, `async`, nor `statusMessage` appear in the skill doc's hook-entry JSON snippets — these are **additional real-world fields the doc doesn't document**; flag for the compat layer as fields to tolerate/pass through even if unimplemented (per §8 unknown-field tolerance).

### Full event list (9 events, doc-confirmed)
`PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreCompact`, `Notification`. **No `PostToolUseFailure` or `PermissionRequest` event found anywhere** in the skill doc, in real hooks.json files, or in this user's live settings.json — treat both as **unconfirmed/likely do not exist** in the current format (the task prompt hypothesized them; evidence says no).

### Hook *type* — two kinds, not one
1. **`command`** — executes a bash command. Fields: `type: "command"`, `command` (string, should use `${CLAUDE_PLUGIN_ROOT}`), `timeout` (seconds, default **60s** for command hooks). Also real-world-only fields `shell`, `async` (see above).
2. **`prompt`** (doc calls this "Recommended" for most cases) — LLM-driven decision. Fields: `type: "prompt"`, `prompt` (string, may reference `$TOOL_INPUT`/`$TOOL_RESULT`/`$USER_PROMPT` etc.), `timeout` (default **30s** for prompt hooks). Supported ONLY on events: `Stop`, `SubagentStop`, `UserPromptSubmit`, `PreToolUse` (doc's explicit "Supported events" list for prompt-type — NOT all 9 events support prompt hooks, only command hooks are universal).

### `matcher` field
- Exact tool name: `"Write"`.
- Alternation: `"Read|Write|Edit"`.
- Wildcard: `"*"` (all tools/all events depending on context).
- Regex, explicitly including MCP-tool patterns: `"mcp__.*__delete.*"`, `"mcp__.*"` (all MCP tools), `"mcp__plugin_asana_.*"` (one plugin's MCP tools scoped by the `mcp__plugin_<name>_...` naming convention — ties directly to §6's MCP tool-naming scheme). **Matchers are case-sensitive** (explicit doc statement).

### Hook INPUT contract (stdin JSON) — common envelope + event-specific fields
Common fields on every hook invocation: `session_id`, `transcript_path`, `cwd`, `permission_mode` (`"ask"|"allow"`), `hook_event_name`.
Event-specific additions: `PreToolUse`/`PostToolUse` → `tool_name`, `tool_input`, `tool_result` (doc names it `tool_result`, NOT `tool_response` — flag vs. the task prompt's hypothesized `tool_response` field name: doc evidence says `tool_result`, but this is the doc's prose description, not a captured real stdin payload — **treat the exact field name as needing live-capture verification**, since I did not capture a real hook stdin payload in this scan). `UserPromptSubmit` → `user_prompt`. `Stop`/`SubagentStop` → `reason`. Prompt-hooks reference these as shell-style variables (`$TOOL_INPUT`, `$TOOL_RESULT`, `$USER_PROMPT`) inside the `prompt` string, implying the harness does variable-substitution into the prompt text before sending it to the LLM judge — this is a distinct mechanism from the command-hook stdin-JSON contract.

### Hook OUTPUT contract
**Universal envelope** (all hook types/events):
```json
{"continue": true, "suppressOutput": false, "systemMessage": "Message for Claude"}
```
`continue` (bool, default true — false halts processing), `suppressOutput` (bool, default false — hides stdout from transcript), `systemMessage` (string shown to Claude).

**PreToolUse-specific output:**
```json
{"hookSpecificOutput": {"permissionDecision": "allow|deny|ask", "updatedInput": {"field": "modified_value"}}, "systemMessage": "..."}
```
— confirms `hookSpecificOutput.permissionDecision` enum `allow|deny|ask` and `updatedInput` (object, lets a hook rewrite the tool call's input) exist exactly as the task prompt hypothesized. Doc does **not** show `hookSpecificOutput.hookEventName` or `permissionDecisionReason` or `additionalContext` fields anywhere in its examples — those are **unconfirmed** by this source (task prompt hypothesized them; not found in [disk:plugin-dev-skill]; would need [web:docs] cross-check, not performed in this pass).

**Stop/SubagentStop-specific output:**
```json
{"decision": "approve|block", "reason": "Explanation", "systemMessage": "Additional context"}
```
— confirms `decision: approve|block` enum (task prompt's hypothesis used lowercase `approve|block` too — matches).

**Exit codes** (doc states this applies generally, and separately for PostToolUse specifically):
- `0` = success, stdout shown in transcript (PostToolUse: "stdout shown in transcript").
- `2` = blocking error, stderr fed back to Claude (PostToolUse: "stderr fed back to Claude" — matches task prompt's hypothesis exactly).
- other = non-blocking error (doc's general Exit Codes section; PostToolUse subsection doesn't separately restate this third case but the general section is presented as applying to "All Hooks").

### Environment variables available to command hooks
`$CLAUDE_PROJECT_DIR` (project root), `$CLAUDE_PLUGIN_ROOT` (plugin dir — the portable-path variable from §1), `$CLAUDE_ENV_FILE` (SessionStart-only — append `export FOO=bar` lines to persist env vars into the session), `$CLAUDE_CODE_REMOTE` (set when running in a remote context).

### Execution model
- **Plugin hooks merge with user's hooks** (verbatim) — i.e. a plugin's `hooks/hooks.json` entries and the user's `settings.json` entries for the same event both fire; not a replace/override relationship.
- **All matching hooks for a given matcher run in parallel** — explicit doc statement, with stated design implications: "Hooks don't see each other's output," "Non-deterministic ordering," "Design for independence." This directly contradicts any assumption of sequential/ordered hook execution within one event+matcher group.
- **Hooks load at session start only** — editing `hooks/hooks.json` or hook scripts does NOT affect the current session; requires exiting and restarting `claude`. Invalid JSON in hooks.json causes a startup **loading failure** (not silently ignored); missing referenced scripts cause warnings (not hard failures) at startup, visible via `claude --debug`. `/hooks` command reviews currently-loaded hooks in-session.

### Real observed additional fields (from this user's live global settings.json, not from the skill doc)
`timeout` (integer seconds) present on real hook entries same as documented. No `once` field observed anywhere on disk in this scan (task prompt hypothesized `once`; **unconfirmed, not found**).

---

## 6. MCP (`.mcp.json`)

Source: `mcp-integration/SKILL.md` [disk:plugin-dev-skill], cross-checked against real `context7/.mcp.json` [disk:installed] (the only real MCP-bundling plugin's config file found in cache — `chrome-devtools-mcp` had no `.mcp.json` file findable in its cache dir despite the name, likely because it bundles an actual MCP *server implementation* rather than a client config, or the config lives elsewhere not captured in this pass — flag as a gap).

### Two ways to declare, both documented, real files use way 1
1. **`.mcp.json` at plugin root** (recommended per doc) — top-level key `mcpServers`, object of `{"<server-name>": {...server-config...}}`.
2. **Inline `mcpServers` field in `plugin.json`** — same shape, embedded in the manifest instead of a separate file.

Real `context7/.mcp.json` [disk:installed] (identical across all 4 cached versions of this plugin):
```json
{
  "mcpServers": {
    "context7": {
      "type": "http",
      "url": "https://mcp.context7.com/mcp?client=claude-code-plugin",
      "headers": {"Authorization": "${CONTEXT7_API_KEY:-}"}
    }
  }
}
```
Notably this confirms **shell-style default-value env-var expansion syntax** `${VAR:-}` works inside `.mcp.json` string values (falls back to empty string if `CONTEXT7_API_KEY` unset) — a specific syntax detail not spelled out in the skill doc's own generic `${API_TOKEN}` examples (doc never shows the `:-` default-value bash-parameter-expansion form explicitly, but the real file proves it's supported).

### Server-config shape by `type`
| `type` | Required fields | Notes |
|---|---|---|
| *(omitted)* = stdio | `command` (string), optional `args` (array), optional `env` (object) | default when no `type` key present; spawns a local child process, Claude Code owns its lifecycle (start/stdio-comms/terminate-on-exit) |
| `"sse"` | `url` | Server-Sent Events; OAuth handled automatically by Claude Code, user authenticates in-browser on first use, no manual token config |
| `"http"` | `url`, optional `headers` (object) | REST/token-auth; confirmed real-world via context7 example above |
| `"ws"` | `url`, optional `headers` (object) | WebSocket; doc example uses `wss://` scheme |

`${CLAUDE_PLUGIN_ROOT}` usable in `command`/`args` for stdio-type portability, same as hooks.

### MCP tool naming convention — confirmed format
`mcp__plugin_<plugin-name>_<server-name>__<tool-name>` — worked example given: plugin `asana`, server `asana`, tool `create_task` → full name `mcp__plugin_asana_asana__asana_create_task` (note the doubled `asana_asana` segment — plugin-name and server-name are NOT deduplicated even when identical, and the tool's own name is ALSO prefixed with `asana_` again inside `create_task`→`asana_create_task`, i.e. triple repetition of the identifier in the worked example — this is confirmed exactly as documented, and is directly relevant to any compat layer that needs to construct/parse these tool names, since a naive `mcp__plugin_<name>__<tool>` two-segment scheme would be **wrong**: it's actually `mcp__plugin_<plugin>_<server>__<tool>`, three components before the double-underscore).

Referenced in `allowed-tools` frontmatter (commands) using this exact naming, either pre-allowing specific tools (recommended) or wildcards like `"mcp__plugin_asana_asana__*"` (doc explicitly discourages wildcards for security — "Pre-allow specific tools, not wildcards").

### Lifecycle
Servers start automatically when the plugin enables; for stdio, connection happens before first tool use per doc ("Lazy Loading" section says "Not all servers connect at startup... First tool use triggers connection" — this appears to **contradict** the "Automatic startup... MCP servers start when plugin enables" line earlier in the same doc; flag as an internal inconsistency in the source doc itself, likely meaning "registered/configured" at plugin-enable time vs. "connection actually opened" lazily on first tool call). Configuration changes require restart (same restart-required pattern as hooks and skill-settings, §5/§7). `/mcp` slash command lists all servers including plugin-provided ones, for in-session inspection.

---

## 7. Settings & misc

### Plugin-scoped user settings pattern — `.claude/<plugin-name>.local.md`
Source: `plugin-settings/SKILL.md` [disk:plugin-dev-skill]. This is **not** a `settings.json`-shaped file — it's a convention (not enforced by any schema the doc shows) of YAML-frontmatter + markdown-body files living at `.claude/<plugin-name>.local.md` in a **project** directory, read by the plugin's own hooks/commands/agents (via `sed`/`grep`/`awk` parsing shown in bash examples — there is no built-in first-party parser; each plugin hand-rolls frontmatter extraction). Purpose: per-project plugin configuration/state, explicitly recommended to be **git-ignored** (`.claude/*.local.md` in `.gitignore`) and **not** committed. Real-world named examples cited in the doc (not independently verified on this disk — these are doc-cited, not disk-confirmed): `multi-agent-swarm` plugin's `.claude/multi-agent-swarm.local.md` (fields: `agent_name`, `task_number`, `pr_number`, `coordinator_session`, `enabled`, `dependencies`), `ralph-loop` plugin's `.claude/ralph-loop.local.md` (fields: `iteration`, `max_iterations`, `completion_promise`, body = prompt text fed back into the loop).

Key mechanical fact: **changes to this file require a Claude Code restart to take effect** if consumed by a hook (hooks load at session start, §5) — same restart constraint family as hooks.json and (implicitly) MCP config changes.

### `outputStyles`, `lspServers`, statusline, `.lsp.json` — direct search results
- **`outputStyles`**: searched all cached `plugin.json` files on this machine — **zero hits**. Not found in any skill doc either. Unconfirmed/no evidence found in this pass (would need [web:docs] to confirm the field exists at all as a plugin.json key).
- **`lspServers`**: confirmed real (see §1) — but observed **only at the marketplace.json plugin-entry level**, never inside an actual `.claude-plugin/plugin.json` cache file in this scan (the three real LSP plugins ship with no local manifest at all). Whether `lspServers` is also a legal key *inside* `.claude-plugin/plugin.json` (as the task hypothesized) is **unconfirmed** — only the marketplace-entry-level placement is evidenced here.
- **`.lsp.json`** (a separate file, analogous to `.mcp.json`): **no evidence found** — not present in any cached plugin dir, not mentioned in any skill doc searched. Unconfirmed/likely does not exist as a convention; `lspServers` appears to be a manifest/marketplace-entry key, not a separate file format.
- **statusline**: not found in any plugin.json; IS found in this user's own root `settings.json` [disk:settings] as a **user-level, non-plugin** setting: `"statusLine": {"type": "command", "command": "npx -y ccstatusline@latest | ...", "padding": 0, "refreshInterval": 10}` — this confirms the `statusLine` schema shape (`type`, `command`, `padding`, `refreshInterval`) exists at the user-settings layer, but no evidence it's a plugin-manifest key.
- **CLAUDE.md/memory conventions relevant to plugins**: no explicit doc coverage found in the 7 skill docs read; out of scope for this pass (not chased further).

### Other real `settings.json` top-level keys observed (context, not plugin-specific, but relevant to "settings & misc" scope) [disk:settings]
`cleanupPeriodDays`, `env` (object of env vars set for every session, e.g. `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`), `permissions.allow` (array), `hooks` (see §5), `statusLine` (above), `enabledPlugins` (see §1), `extraKnownMarketplaces` (see §1), `effortLevel`, `tui`, `skipDangerousModePermissionPrompt`, `skipWorkflowUsageWarning`, `agentPushNotifEnabled`, `skipAutoPermissionPrompt` — several of these (`effortLevel`, `tui`, the `skip*` flags) are **not** documented anywhere in the 7 plugin-dev skill docs; they are host-level Claude Code settings orthogonal to the plugin format itself, included here only because they were visible in the same file and might matter to a settings-compat story, not a plugin-compat story.


---

## 8. Version/compat notes — the plugin-dev skill doc is measurably behind the live docs

**Critical finding for a compatibility layer:** the installed `plugin-dev` skill (pinned `c447c3207a42`, itself installed/updated 2026-09-20 per `installed_plugins.json` — i.e. *current as of today*) is **significantly simplified/outdated** relative to the live docs at `code.claude.com/docs/en/hooks` and `.../plugins-reference` [web:docs, fetched 2026-09-20]. This is not a minor gap — it changes what "支持 Claude Code plugins" must mean. Concrete deltas:

### Hook events: 9 documented on disk vs **32** on the live docs page
[disk:plugin-dev-skill] documents only: `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreCompact`, `Notification`.

[web:docs `https://code.claude.com/docs/en/hooks`] lists 32: `SessionStart`, `Setup`, `UserPromptSubmit`, `UserPromptExpansion`, `PreToolUse`, **`PermissionRequest`**, `PermissionDenied`, `PostToolUse`, **`PostToolUseFailure`**, `PostToolBatch`, `Notification`, `MessageDisplay`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `Stop`, `StopFailure`, `TeammateIdle`, `InstructionsLoaded`, `ConfigChange`, `CwdChanged`, `DirectoryAdded`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `PreModelSwitch`, `PostModelSwitch`, `Elicitation`, `ElicitationResult`.

This **confirms** the task prompt's hypothesized `PostToolUseFailure` and `PermissionRequest` events exist — they were simply absent from the (current, actively-shipping) plugin-dev skill's simplified event list. **Implication for a compat host**: do not treat the 9-event list as the ceiling. A host claiming plugin compatibility should at minimum wire the original 9 plus `PermissionRequest`/`PostToolUseFailure` (directly relevant to a security/approval-gate-shaped host like Aleph), and should structurally tolerate/no-op unknown future events rather than erroring, given this list has evidently grown a lot and will likely keep growing (§8 "unknown-field/event tolerance").

### PreToolUse stdin schema — richer than the skill doc, and now versioned to session concepts the skill doc never mentions
[web:docs] gives the current PreToolUse stdin shape:
```json
{
  "session_id": "string",
  "prompt_id": "uuid (absent until first user input)",
  "transcript_path": "string",
  "cwd": "string",
  "scratchpad_dir": "string (absent if no scratchpad)",
  "permission_mode": "default|plan|acceptEdits|auto|dontAsk|bypassPermissions",
  "effort": {"level": "low|medium|high|xhigh|max"},
  "hook_event_name": "PreToolUse",
  "agent_id": "string (subagent only)",
  "agent_type": "string (subagent or --agent)",
  "tool_name": "string",
  "tool_input": {"...": "..."},
  "tool_use_id": "string"
}
```
This both confirms [disk:plugin-dev-skill]'s common-envelope fields (`session_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`) and adds fields the skill doc never mentions: `prompt_id`, `scratchpad_dir`, `effort.level`, `agent_id`, `agent_type`, `tool_use_id`. Note `permission_mode`'s enum is **6 values** (`default|plan|acceptEdits|auto|dontAsk|bypassPermissions`), not the 2-value `ask|allow` the skill doc showed — the skill doc's `ask|allow` is flatly wrong/stale against the live enum.

**`tool_result` vs `tool_response` — still genuinely unresolved.** [disk:plugin-dev-skill] prose says `tool_result`. The web fetch of the hooks page explicitly could not surface the PostToolUse-specific stdin schema in the fetched content and flagged the exact field name as **not shown** in what it retrieved. This item needs a live-captured real PostToolUse hook invocation (e.g. `cat > /tmp/x; echo hi` piped through a debug hook) to settle — **not resolved in this pass**; do not guess which name is correct.

### `hookSpecificOutput` — richer than the skill doc, confirms the task's hypothesized fields
[web:docs] output schema:
```json
{
  "hookSpecificOutput": {
    "hookEventName": "string",
    "permissionDecision": "allow|deny|block",
    "permissionDecisionReason": "string",
    "additionalContext": "string",
    "updatedInput": {"...": "..."},
    "systemMessage": "string",
    "terminalSequence": "string",
    "retry": true
  }
}
```
This **confirms** `hookEventName`, `permissionDecisionReason`, and `additionalContext` all exist (all three were unconfirmed-by-disk-source in §5) — but note the enum given here is `allow|deny|block`, whereas [disk:plugin-dev-skill]'s own PreToolUse example literally says `"permissionDecision": "allow|deny|ask"` (three values, third one `ask` not `block`). **This is a real three-way disagreement worth flagging rather than silently picking one**: disk skill doc says `allow|deny|ask`; the freshly-fetched web page says `allow|deny|block`. A compat layer should probably accept `ask` as an alias/degrade-path input from third-party plugins written against the (still currently-shipping) skill-doc's documented enum, while treating `allow|deny|block` as the authoritative current output the host itself should honor — but this is inference, not a confirmed resolution; flag as open. Also new: `terminalSequence`, `retry` — undocumented anywhere on disk, meaning present-day sinceunknown-date additions.

### Exit code semantics — one real nuance the skill doc omits entirely
[web:docs]: exit 0 → stdout parsed as JSON **only if it starts with `{` and ends with `}`**; exit 2 → blocking, message from `permissionDecisionReason` or stderr, **"cannot be overridden by JSON"**; other codes → non-blocking on most events, **except** `WorktreeCreate`/`WorktreeRemove` where any nonzero code fails the operation (an explicit named exception the skill doc has no way to know about, since it doesn't know those events exist).

### Timeouts — per-hook-type table, not the skill doc's flat "60s/30s"
[web:docs]: `command`/`http`/`mcp_tool` types default **600s** (not 60s as [disk:plugin-dev-skill] states for command hooks!), except **30s** specifically on `UserPromptSubmit`, `PreModelSwitch`, `PostModelSwitch`, `MessageDisplay`. `prompt` type stays 30s (agrees with disk). New `agent` type at 60s (undocumented on disk — implies a hook `type` beyond just `command`/`prompt`, another disk-vs-live gap). `SessionEnd` has a distinct "1.5-second shared budget (raised to match per-hook timeout up to 60 seconds)" rule found nowhere on disk. **This is a materially different number (600s vs 60s) that a compat layer's default timeout must match against the live behavior, not the disk skill doc's stated default**, if the goal is real interop rather than doc-fidelity.

### Env vars — 3 more than the skill doc
[disk:plugin-dev-skill]: `CLAUDE_PROJECT_DIR`, `CLAUDE_PLUGIN_ROOT`, `CLAUDE_ENV_FILE` (SessionStart-only), `CLAUDE_CODE_REMOTE`.
[web:docs] adds: `CLAUDE_PLUGIN_DATA` (plugin persistent data directory — a **new concept**, a plugin-owned persistent storage dir distinct from the read-only `CLAUDE_PLUGIN_ROOT` install dir), `CLAUDE_CODE_BRIDGE_SESSION_ID` (Remote Control session id, "v2.1.199+" — confirms the product has a version-gated feature history worth being defensive about), `CLAUDE_EFFORT` (current effort level — ties to the `effort.level` stdin field above), `CLAUDE_PLUGIN_OPTION_*` (a **whole family** of env vars, one per `userConfig` field the plugin declares, e.g. `$CLAUDE_PLUGIN_OPTION_WEBHOOK_URL` — this only makes sense in light of the `userConfig` manifest field documented below, itself entirely absent from the disk skill doc). Also noted: hooks inherit the parent environment **minus `OTEL_*` exporter vars**, and additionally scrubbed when `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` is set — an env-hygiene detail with no disk-doc counterpart at all.

### `plugin.json` — the live schema is much larger than [disk:plugin-dev-skill]'s list
[web:docs `.../plugins-reference`] gives a materially longer field list than §1's disk-derived table. New fields not in the disk skill doc: `displayName`, `metadata` (free-form, "Claude Code ignores at runtime"), `defaultEnabled` (boolean, default `true`), `$schema`, `workflows` (path, **replaces** default `workflows/`), `outputStyles` (path, **replaces** default `output-styles/`), `lspServers` (now confirmed as a **plugin.json-level** field too, not just marketplace-entry-level as observed on disk in §1 — resolves that earlier "unconfirmed" flag), `userConfig` (object — schema: per-field `type` ∈ `string|number|boolean|directory|file`, required `title`+`description`, optional `sensitive` (mask+secure-store), `required`, `default`, `options` (string-type only, "requires v2.1.271+" — another version-gate), `multiple`, `min`/`max`), `channels` (array of `{server, userConfig}`, `server` must match an `mcpServers` key), `dependencies` (array of plugin-name strings or `{name, version-semver-constraint}` objects — **plugin-to-plugin dependency declarations**, entirely unmentioned on disk), `experimental.{themes,monitors,evals}` (each following the same path-array convention as other component fields).

**Supplements-vs-replaces is field-specific, and the disk skill doc's blanket claim is wrong for several fields.** [disk:plugin-dev-skill] states custom paths "supplement (do not replace)" defaults, as a general rule. [web:docs] instead gives a **per-field table**: `commands`, `agents`, `workflows`, `outputStyles`, `experimental.themes`, `experimental.monitors` all **replace** the default directory scan when set; only `skills` **adds to** the default `skills/` scan (with a further carve-out: "marketplace entries with `source` resolving to marketplace root: declaring specific subdirectories replaces the default `skills/` scan"). A compat layer built on the disk skill doc's "always supplements" rule would misbehave for every field except `skills`.

**Unknown-field tolerance — now explicit, and load-bearing for a compat claim.** [web:docs]: "Claude Code ignores top-level fields it doesn't recognize" in `plugin.json`, explicitly so a manifest "can double as VS Code/Cursor/npm manifests" (directly explains the `superpowers` plugin's cohabiting `.cursor-plugin/`, `.kimi-plugin/`, `.opencode/`, etc. dirs observed on disk in §1 — those are siblings at plugin ROOT, not inside `.claude-plugin/`, so they don't even need the unknown-field tolerance; but the same tolerance principle applies to unknown *keys inside* `plugin.json` itself). `claude plugin validate` reports unrecognized fields as **warnings**, escalated to errors only with `--strict`. **Type mismatches are NOT tolerated the same way**: most fields fail plugin load entirely on a type mismatch (e.g. `keywords` given as a string instead of array); only `experimental` and `metadata` degrade gracefully (ignored + warning) on a non-object value. This asymmetry (unknown *keys* = safe warning; wrong *type* on a known key = hard failure) is the single most important tolerance rule for a compat layer to get right, and is the direct authoritative resolution of the task's own "unknown-field tolerance expectations" ask.

### `marketplace.json` `source` — a genuine, unresolved conflict between real disk files and the live docs
This is the most important discrepancy found in this whole scan and must **not** be silently resolved by picking one side.

**Real, currently-loaded marketplace.json on disk** [disk:marketplace] uses the inner discriminator key **`source`** (the same word, reused, one level down):
```json
"source": {
  "source": "git-subdir",
  "url": "https://github.com/42Crunch-AI/claude-plugins.git",
  "path": "plugins/api-security-testing",
  "ref": "v1.5.5",
  "sha": "30287f5e3f122a646d1ac5ca3ab96e130c52a3ad"
}
```
(also observed: `{"source": "url", "url": "...", "sha": "..."}`, and bare-string `"source": "./plugins/agent-sdk-dev"`.)

**Live docs** [web:docs `.../plugins-reference`] instead describe the discriminator key as **`type`**:
```json
"source": {"type": "github", "repo": "owner/repo", "ref": "..."}
"source": {"type": "git", "url": "https://..."}
"source": {"type": "git-subdir", "url": "https://...", "path": "..."}
"source": {"type": "npm", "package": "@scope/name", "version": "semver-range"}
"source": {"type": "local", "path": "/abs/or/~/path"}
"source": {"type": "command", "command": "...", "mode": "copy|link"}
"source": {"type": "archive", "url": "...", "format": "tar.gz|tar.bz2|zip", "strip": 0, "headersHelper": "..."}
"source": {"type": "synced"}
```
i.e. this is a real, live, shipping-today marketplace file using field name `source` as its own type-discriminator, while the documentation page describes the discriminator as `type`. **A compat parser must accept BOTH `{"source": "<type>", ...}` and `{"type": "<type>", ...}` shapes** for a marketplace-entry `source` object — this is not a hypothetical edge case, it is the literal shape of `anthropics/claude-plugins-official`'s live marketplace.json as installed on this machine today. (Separately, `known_marketplaces.json` — Claude Code's own local tracking file, not a distributed format — wraps a *marketplace's own* source the same doubled way: `{"source": {"source": "github", "repo": "..."}}`, i.e. outer key `source` holding an object whose own discriminator key is again literally `source`. This is consistent with the disk-marketplace.json convention, not the docs' `type` convention, reinforcing that the real/shipping convention is `source`-as-discriminator and the docs may be describing a newer, not-yet-fully-migrated, or simply misdocumented alternate spelling.)

The **`npm`** source variant is now confirmed to exist (web docs only — not seen on any real disk file in this scan): `{"type"|"source": "npm", "package": "@scope/name", "version": "semver-range"}`, with a note to use `npm-shrinkwrap.json` for reproducible distribution. Also newly confirmed **`local`** (absolute or `~`-relative path — note this directly contradicts §1's plugin.json-level path rule that "cannot use absolute paths"; that rule applies to `plugin.json`'s own custom-path fields like `commands`/`agents`, not to a marketplace `source`, which is a different context — **don't conflate the two path-rule contexts**), **`command`** (execute-and-capture, with `copy`|`link` mode — a supply-chain-relevant source type: arbitrary command execution to produce a plugin's file tree, worth flagging for anyone doing security review of the format), and **`archive`** (tarball/zip fetch with optional `headersHelper` command for auth headers).

### Skills frontmatter — the live docs confirm a rich field set absent from [disk:plugin-dev-skill], AND a dual-spec split
[web:docs `.../skills`] gives a SKILL.md frontmatter field list far exceeding the plugin-dev skill's "just name + description" (§4): `when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`, `allowed-tools`, `disallowed-tools`, `model`, `effort`, `context` (`fork` = run in a forked subagent), `agent` (subagent type when `context: fork`), `background` (`false` = wait synchronously for forked-subagent result), `hooks` (hooks that register only while the skill is active — a **skill-scoped hooks mechanism** entirely absent from [disk:plugin-dev-skill] and from §5's plugin-level hooks discussion), `paths` (glob patterns gating activation), `shell` (`bash`|`powershell` for injected commands), `metadata`, `license`, `compatibility`.

Crucially, **`allowed-tools` on a skill does NOT restrict tool access** — it only pre-grants permission for the listed tools for the turn that invokes the skill (quoted verbatim: "It does not restrict which tools are available: every tool remains callable, and your permission settings still govern tools that are not listed."). This directly contradicts a plausible compat-layer assumption (and contradicts how `allowed-tools` behaves on **commands**, §2, where it does gate which tools the command may reach for — same field name, different semantics depending on whether it's on a command or a skill). `disallowed-tools` is the one that actually removes tools from the pool while a skill is active.

**Dual-spec split, directly relevant to any host wanting broader-than-Claude-Code compatibility**: only `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools` are part of the portable **Agent Skills spec** (usable outside Claude Code — claude.ai, Skills API, cross-tool packaging). Every other field listed above (`when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`, `disallowed-tools`, `model`, `effort`, `context`, `agent`, `background`, `hooks`, `paths`, `shell`) is a **Claude-Code-specific extension** that "will cause a hard error if included in skills uploaded to claude.ai or packaged for external distribution." A host aiming for the broader Agent-Skills-spec target and a host aiming for full Claude-Code-plugin fidelity have **different, only-partially-overlapping** field sets to support — this split should be a first-class distinction in any compat checklist, not folded into one undifferentiated list.


---

## Compatibility checklist

Numbered, flat, tagged `[manifest]`/`[command]`/`[agent]`/`[skill]`/`[hook]`/`[mcp]`/`[settings]`/`[marketplace]`. Each item cites its evidence tier: **(disk)** = directly observed real file, **(skill)** = plugin-dev skill doc prose/example, **(web)** = live docs fetch 2026-09-20, **(conflict)** = sources disagree, host must choose deliberately.

1. `[manifest]` Manifest file MUST live at `<plugin-root>/.claude-plugin/plugin.json`; never at plugin root itself. (disk+skill)
2. `[manifest]` Component directories (`commands/`, `agents/`, `skills/`, `hooks/`, `workflows/`, `output-styles/`, `themes/`, `monitors/`) MUST sit at plugin root, sibling to `.claude-plugin/`, never nested inside it. (skill+web)
3. `[manifest]` Only `name` is a required field on `plugin.json`; everything else, including the manifest file's own presence for LSP-only plugins defined entirely via marketplace.json, is optional. (disk)
4. `[manifest]` Accept `version` as either semver OR an opaque string such as a 12-char git-sha prefix — do not hard-validate semver format. (disk)
5. `[manifest]` Support the full field set: `name`, `displayName`, `version`, `description`, `author{name,email,url}`, `homepage`, `repository`, `license`, `keywords[]`, `metadata{}`, `defaultEnabled` (bool, default true), `$schema`, `skills`, `commands`, `agents`, `workflows`, `hooks`, `mcpServers`, `outputStyles`, `lspServers`, `userConfig{}`, `channels[]`, `dependencies[]`, `experimental.{themes,monitors,evals}`. (web)
6. `[manifest]` Ignore unrecognized top-level keys in `plugin.json` as a warning, not a load failure — this is load-bearing because real plugins (e.g. `superpowers`) cohabit `.claude-plugin/` config with unrelated tool manifests. (web)
7. `[manifest]` On a **type mismatch** for a known field (e.g. `keywords` given as a string), fail plugin load — except `experimental` and `metadata`, which degrade to ignored+warning on wrong type. Do not apply the same leniency to every field. (web)
8. `[manifest]` For override-path fields, honor the per-field supplements-vs-replaces split: `skills` **adds to** default `skills/`; `commands`, `agents`, `workflows`, `outputStyles`, `experimental.themes`, `experimental.monitors` **replace** their defaults when set. (web, corrects skill doc's blanket "always supplements" claim)
9. `[manifest]` Override-path values must be relative, start with `./`, and support arrays for multiple locations (this rule applies to `plugin.json`'s own path fields — do not conflate with `marketplace.json` `source.path`, which may be absolute or `~`-relative for `local`-type sources). (skill; conflict-adjacent, see #9)
10. `[manifest]` Support `${CLAUDE_PLUGIN_ROOT}` expansion in every path-bearing field (hook commands, MCP server command/args, LSP command/args) and in prose references inside command/agent/skill markdown bodies. (disk+skill)
11. `[manifest]` Support `${CLAUDE_PLUGIN_DATA}` as a distinct, writable, persistent-storage directory separate from the read-only `${CLAUDE_PLUGIN_ROOT}` install directory. (web)
12. `[manifest]` `userConfig` fields: support `type` ∈ `string|number|boolean|directory|file`, required `title`+`description`, optional `sensitive` (mask input, store securely), `required`, `default`, `options[]` (string type only), `multiple` (bool), `min`/`max` (number type). Each declared field must surface as `$CLAUDE_PLUGIN_OPTION_<NAME>` in hook/process environments. (web)
13. `[manifest]` `channels[]` entries reference an `mcpServers` key by name via a `server` field and carry their own `userConfig`. (web)
14. `[manifest]` `dependencies[]` accepts either a bare plugin-name string or `{name, version: "<semver-constraint>"}`. (web)
15. `[manifest]` `lspServers` is valid both as a `plugin.json` top-level key AND inline in a `marketplace.json` plugin entry (three real Anthropic LSP plugins ship with NO local manifest at all — the marketplace entry is sufficient). Shape: `{"<server-id>": {"command": str, "args": [str]?, "extensionToLanguage": {".ext": "lang", ...}}}`. (disk+web)
16. `[command]` Commands are `.md` files under `commands/`; body content is an instruction TO the agent, not descriptive text for the user — a compat host's rendering/execution must treat the body as a system-turn-style prompt injection, not a chat message to display verbatim. (skill)
17. `[command]` Three scopes with distinct `/help` labels: project `.claude/commands/` → `(project)`; personal `~/.claude/commands/` → `(user)`; plugin `plugin/commands/` → `(plugin-name)`. (skill)
18. `[command]` Frontmatter: `description` (string), `allowed-tools` (accept BOTH a comma-joined string like `Bash(git:*), Read` AND a YAML array — both forms are real, confirmed on two different shipped files), `model` (`sonnet|opus|haiku`, no `inherit` value), `argument-hint` (string), `disable-model-invocation` (bool, default false). (disk+skill)
19. `[command]` `allowed-tools` on a **command** DOES restrict/gate tool access for that command's execution — semantically different from the same field name on a **skill** (see #34), which only pre-grants permission without restricting. Do not implement one code path for both. (skill+web, conflict-adjacent)
20. `[command]` Support `$ARGUMENTS` (all args as one string) and positional `$1`, `$2`, … . (skill)
21. `[command]` Support `@path` (and `@$1` with argument substitution) as an eager file-content inclusion directive, resolved before the prompt is sent. (skill)
22. `[command]` Support `` !`shell command` `` inline execution, output substituted into the prompt text before Claude sees it; requires `Bash`/scoped-`Bash` in `allowed-tools`. (skill)
23. `[command]` Subdirectories under `commands/` produce namespaced invocation labels (`plugin:subdir:name` for plugins, `project:subdir` for project commands) shown in `/help`. (skill)
24. `[agent]` Agent files are `.md` under `agents/`; frontmatter requires `name` (3–50 chars, lowercase/digits/hyphens, must start/end alphanumeric), `description` (trigger-condition prose), `model` (`inherit|sonnet|opus|haiku`), `color` (`blue|cyan|green|yellow|magenta|red`); `tools` (array) is optional, omit = full access. (skill, disk-confirmed on real file)
25. `[agent]` Accept `description` as a YAML block-scalar containing embedded multi-line `<example>`/`<commentary>` pseudo-XML blocks — real shipped agents use this, not just short prose+pointer as the doc's own template suggests. Do not truncate or reject long block-scalar descriptions. (disk)
26. `[agent]` Agent body = system prompt, second-person voice, target 500–3,000 chars, treat >10,000 chars as a soft-fail/warn threshold, not hard reject (doc's own stated ceiling, not independently confirmed as enforced). (skill)
27. `[agent]` Namespacing: bare `agent-name` for a top-level plugin agent, `plugin:subdir:agent-name` for a nested one. (skill)
28. `[skill]` Skill = directory `skills/<name>/SKILL.md` (required) plus optional `scripts/`, `references/`, `assets/`. Discovery scans for the literal filename `SKILL.md`, case-sensitive, not `README.md` or any alias. (skill)
29. `[skill]` Only `name`+`description` are required in SKILL.md frontmatter; real files use kebab-case `name` matching the directory name (not the doc-template's Title-Case example). (skill+disk)
30. `[skill]` `description` MUST be validated/authored in third person with concrete quoted trigger phrases — this is a content-quality convention, not a machine-enforced syntax rule; a compat host should not hard-reject second-person descriptions, only a linter/reviewer tool would flag it. (skill)
31. `[skill]` Support the full Claude-Code-extension frontmatter set beyond name/description: `when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`, `allowed-tools`, `disallowed-tools`, `model`, `effort`, `context` (`fork`), `agent`, `background`, `hooks`, `paths`, `shell`, `metadata`, `license`, `compatibility`. (web)
32. `[skill]` Implement the 3-level progressive-disclosure loading model precisely: (a) name+description always resident; (b) SKILL.md body loaded only on trigger; (c) `scripts/`/`references/`/`assets/` loaded/executed only on demand, with scripts specifically executable WITHOUT ever being read into the context window. (skill)
33. `[skill]` `context: fork` + `agent: <type>` + `background: false|true` define a skill-scoped subagent-dispatch mechanism (fork to a named subagent type, optionally block for its result) — distinct from the plugin-level `agents/` directory. (web)
34. `[skill]` `allowed-tools` on a skill only pre-grants permission (no prompt) for the invoking turn; it does NOT restrict the available tool set. `disallowed-tools` is the field that actually removes tools from the pool while the skill is active. (web) — contrast with #19.
35. `[skill]` `hooks` field on a skill registers hooks scoped to only-while-that-skill-is-active — a skill-local hook mechanism, separate from and additive to plugin-level `hooks/hooks.json`. (web)
36. `[skill]` For any "publish to claude.ai / Skills API / cross-tool packaging" path, restrict emitted frontmatter to the Agent-Skills-spec subset: `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools` only — including any other field is a documented hard error on that path. Keep this as a SEPARATE compliance target from full Claude-Code-plugin-format compatibility. (web)
37. `[hook]` Plugin `hooks/hooks.json` format wraps events under a required `hooks` key, with an optional sibling `description`: `{"description"?, "hooks": {"<Event>": [...]}}`. User `settings.json` format has NO wrapper — events sit directly at top level. Treat these as two distinct serializations of the same event-map, not two different feature sets. (skill, disk-confirmed both real files)
38. `[hook]` Support hook-entry fields beyond the doc's own JSON examples, since real shipped hooks use them: `shell` (explicit shell selector), `async` (bool), `statusMessage` (string, shown while the hook runs). (disk)
39. `[hook]` Minimum viable event set for a compat claim: `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreCompact`, `Notification` (skill-doc baseline) PLUS `PermissionRequest`, `PostToolUseFailure` (confirmed real, security/approval-relevant). Full live list has 32 named events; unknown/future events should no-op rather than error. (skill+web)
40. `[hook]` Two hook `type`s minimum: `command` (bash execution) and `prompt` (LLM-judged decision, restricted to `Stop`/`SubagentStop`/`UserPromptSubmit`/`PreToolUse` per the skill doc). Live docs additionally reference `http`, `mcp_tool`, and `agent` hook types by name in the timeout table — their configuration shape was not captured in this pass; flag as a follow-up, do not assume they're absent. (skill+web, partial)
41. `[hook]` `matcher` is a regex-capable string: exact tool name, `|`-alternation, `*` wildcard, and arbitrary regex including MCP-scoped patterns like `mcp__.*__delete.*`. Matching is case-sensitive. (skill)
42. `[hook]` Command-hook stdin envelope (common fields, all events): `session_id`, `transcript_path`, `cwd`, `hook_event_name`; live docs add `prompt_id`, `scratchpad_dir`, `permission_mode` (6-value enum: `default|plan|acceptEdits|auto|dontAsk|bypassPermissions` — NOT the skill-doc's stale 2-value `ask|allow`), `effort.level`, `agent_id`, `agent_type`. (skill+web, web supersedes skill on `permission_mode`)
43. `[hook]` PreToolUse/PostToolUse add `tool_name`, `tool_input`, `tool_use_id`. **The exact field name for the post-execution result payload (`tool_result` vs `tool_response`) is UNRESOLVED by this scan** — skill doc prose says `tool_result`; live-docs fetch could not surface the field; do not hardcode either name without a live-captured sample. (skill, unresolved)
44. `[hook]` UserPromptSubmit adds `user_prompt`; Stop/SubagentStop add `reason`. (skill)
45. `[hook]` Prompt-type hooks receive stdin-derived values as shell-style template variables inside the `prompt` string itself (`$TOOL_INPUT`, `$TOOL_RESULT`, `$USER_PROMPT`), a distinct substitution mechanism from the command-hook JSON-stdin contract. (skill)
46. `[hook]` Universal stdout envelope: `{"continue": bool (default true), "suppressOutput": bool (default false), "systemMessage": string}`. (skill)
47. `[hook]` PreToolUse-specific stdout: `hookSpecificOutput: {permissionDecision, updatedInput, hookEventName, permissionDecisionReason, additionalContext, systemMessage, terminalSequence, retry}`. `permissionDecision` enum is `allow|deny|ask` per the currently-shipping skill doc but `allow|deny|block` per the live docs page fetched today — **genuine three-way conflict, host should accept all of `allow|deny|ask|block` on input and pick one canonical output value deliberately, not silently.** (skill+web, conflict — see #47)
48. `[hook]` Stop/SubagentStop-specific stdout: `{"decision": "approve|block", "reason": string, "systemMessage": string}`. (skill)
49. `[hook]` Exit code semantics: `0` = success, stdout parsed as JSON only if it starts with `{` and ends with `}`; `2` = blocking, reason from `hookSpecificOutput.permissionDecisionReason` or stderr, NOT overridable by other JSON fields; other nonzero = non-blocking error almost everywhere, EXCEPT `WorktreeCreate`/`WorktreeRemove` where any nonzero fails the operation. (web, more precise than skill doc)
50. `[hook]` Default timeouts: `command`/`http`/`mcp_tool` = 600s, except 30s specifically on `UserPromptSubmit`/`PreModelSwitch`/`PostModelSwitch`/`MessageDisplay`; `prompt` = 30s; `agent` = 60s; `SessionEnd` = a distinct 1.5s-shared-budget rule (raised to match per-hook timeout up to 60s). Do NOT use the skill doc's flat "60s command / 30s prompt" — it is stale for `command`. (web supersedes skill)
51. `[hook]` Env vars available to hook processes: `$CLAUDE_PROJECT_DIR`, `$CLAUDE_PLUGIN_ROOT`, `$CLAUDE_PLUGIN_DATA`, `$CLAUDE_ENV_FILE` (SessionStart-only, append `export` lines to persist session env vars), `$CLAUDE_CODE_REMOTE`, `$CLAUDE_CODE_BRIDGE_SESSION_ID`, `$CLAUDE_EFFORT`, `$CLAUDE_PLUGIN_OPTION_*` (one per declared `userConfig` field). Inherited environment excludes `OTEL_*`, plus further scrubbing under `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`. (skill+web)
52. `[hook]` All hooks matching a given event+matcher run in PARALLEL with no defined ordering and no visibility into each other's output — a compat host must not assume or provide sequential hook execution within one matcher group. (skill)
53. `[hook]` Plugin hooks and user (settings.json) hooks for the same event MERGE (both fire) rather than one overriding the other. (skill)
54. `[hook]` Hooks load only at session start; config/script edits require a full restart to take effect — no hot-reload contract to honor. Invalid JSON in `hooks.json` is a hard startup load failure; a missing referenced script is a startup warning, not a hard failure. (skill)
55. `[mcp]` Two equally-valid declaration sites: standalone `.mcp.json` at plugin root (`{"mcpServers": {...}}`), or inline `mcpServers` object directly in `plugin.json`. (skill)
56. `[mcp]` Four server-config shapes by (often-implicit) `type`: unset/`stdio` (`command`, `args[]`, `env{}`), `sse` (`url`, OAuth handled automatically by the host, no manual token config expected), `http` (`url`, `headers{}`), `ws` (`url`, `headers{}`). (skill, `http` confirmed disk-real)
57. `[mcp]` Support bash-style default-value env expansion `${VAR:-}` inside `.mcp.json` string values (confirmed on a real shipped file), not just bare `${VAR}` substitution. (disk)
58. `[mcp]` MCP tool naming: `mcp__plugin_<plugin-name>_<server-name>__<tool-name>` — THREE identifier segments before the tool name (plugin, then server, undeduplicated even if identical to plugin name), joined by single underscores, with a final double-underscore before the tool name itself. Do not assume a simpler two-segment `mcp__plugin_<name>__<tool>` scheme. (skill, worked example)
59. `[mcp]` `allowed-tools` referencing MCP tools should be pre-allow-specific-tools by convention (security best practice from the doc), but wildcard forms like `mcp__plugin_x_y__*` must still be syntactically accepted even though discouraged. (skill)
60. `[mcp]` `/mcp`-equivalent introspection: a compat host should expose a way to list all connected MCP servers including plugin-provided ones, for parity with this documented debugging surface. (skill)
61. `[settings]` Plugin-scoped per-project user config convention: `.claude/<plugin-name>.local.md` — YAML frontmatter + markdown body, git-ignored by convention, hand-parsed by each plugin's own hooks/commands (no first-party parser exists in the format itself — every real example hand-rolls `sed`/`grep`/`awk` frontmatter extraction). Changes require restart if consumed by a hook. (skill)
62. `[settings]` `enabledPlugins` in `settings.json` is a flat `{"<plugin>@<marketplace>": true|false}` map; composite key format matches `installed_plugins.json`'s key format exactly — treat plugin identity as ALWAYS the composite `name@marketplace` pair, never bare plugin name alone, since names are only unique within one marketplace. (disk)
63. `[settings]` `extraKnownMarketplaces` in `settings.json` lets a user's own settings file register additional marketplaces (same source-object shape as the central `known_marketplaces.json`), for settings-file portability across machines. (disk)
64. `[marketplace]` `marketplace.json` top level: `$schema`, `name`, `description`, `owner{name,email}`, `renames{old:new}` (rename/redirect history for plugin names), `plugins[]`. (disk)
65. `[marketplace]` Per-plugin marketplace entry supports: `name`, `displayName` (overrides plugin.json's), `version` (takes precedence over plugin.json's), `description`, `author` (string OR `{name,email,url}` object — both forms real), `homepage`, `repository`, `license`, `keywords[]`, `tags[]`, `category` (free string), `strict` (bool — tolerance-strictness flag, exact semantics not fully confirmed), `skills[]` (subpaths — lets one marketplace source register multiple independently-named skill-only sub-plugins), `lspServers` (inline, see #15), `defaultEnabled` (bool, overrides plugin's own), `source` (required). (disk+web)
66. `[marketplace] [conflict]` The `source` object's type-discriminator field is **`source`** (reusing the outer key name one level down) in every real, currently-live marketplace.json file inspected on disk — e.g. `{"source": {"source": "git-subdir", "url", "path", "ref", "sha"}}` — while the live documentation page describes the discriminator field as **`type`** instead — e.g. `{"source": {"type": "git-subdir", "url", "path"}}`. **A compat parser MUST accept both spellings** (`source.source` and `source.type` as equally valid discriminator keys) since the disk form is empirically what `anthropics/claude-plugins-official` ships today and the docs form is the officially-documented spelling — do not implement only one.
67. `[marketplace]` `source` variants to support, keyed by discriminator value (see #66 for which key holds it): bare relative-path string (`"./plugins/x"`, relative to marketplace root, loads in place, no auto dependency-install); `github` (`repo: "owner/name"`, optional `ref`); `git` (`url`); `git-subdir` (`url`, `path`, optional `ref`+`sha` — real files always carry BOTH `ref` and a resolved 40-hex `sha` together); `url` (whole external repo, `url`+`sha`, no `path`/`ref` on this variant); `npm` (`package: "@scope/name"`, `version: "semver-range"`, use `npm-shrinkwrap.json` for reproducibility — web-only, not disk-confirmed); `local` (`path`, absolute or `~`-relative — deliberately looser than the `plugin.json` internal path rules, do not conflate, see #9); `command` (`command` string, `mode: "copy"|"link"` — executes arbitrary code to materialize a plugin tree; flag for security review as a supply-chain-sensitive source type); `archive` (`url`, `format: tar.gz|tar.bz2|zip`, `strip: N`, optional `headersHelper` command for auth headers); `synced` (internal-only marker, no further fields observed). (disk for path/git-subdir/url; web for github/git/npm/local/command/archive/synced)
68. `[marketplace]` `known_marketplaces.json` (Claude Code's own local tracking file — infra, not a distributed format) uses the SAME doubled `{"source": {"source": "github", "repo": "..."}}` shape as the real marketplace.json entries (#66), for tracking where a whole marketplace itself came from, as distinct from where one plugin within it came from. (disk)
69. `[marketplace]` `installed_plugins.json` schema: root `{"version": 2, "plugins": {"<name>@<marketplace>": [{"scope": "user", "installPath", "version", "installedAt", "lastUpdated", "gitCommitSha"?}]}}` — value is an ARRAY per key (design allows multiple scope-entries per composite key, though only single-element arrays with `"scope": "user"` were observed on this machine; a `project`-scope entry was never seen and should be treated as plausible-but-unconfirmed). (disk)
70. `[settings]` `.claude/commands/` (flat legacy command format) and `skills/<name>/SKILL.md` are explicitly documented as loading "identically" from Claude Code's point of view despite different file layouts and different invocation models (always-listed-slash-command vs autonomously-triggered) — a compat host should treat commands as a first-class citizen alongside skills, not as a deprecated stub, per the skill doc's own framing of "legacy format" (deprecated in *recommendation*, not in *support*). (skill)

