# Change: Flatten Tool Namespace (Unified Flat Namespace)

## Status
- **Stage**: Proposed
- **Created**: 2026-01-10
- **Depends on**: unify-tool-registry (deployed)

## Why

### The Problem: Leaky Implementation

Currently, users must remember and use source-based prefixes to invoke tools:

```
/mcp git status     # MCP tool from git server
/skill refine-text  # Claude Agent skill
/search query       # Native search capability
```

This is **"Leaky Implementation"** - exposing internal architecture (MCP, Skill, Native) to users. Users don't care WHERE a capability comes from; they only care WHAT they want to do.

### User Mental Model vs Current Implementation

**What users think:**
> "I want to check git status" → `/git status`
> "I want to search the web" → `/search query`
> "I want to translate to English" → `/en some text`

**What we force them to do:**
> "I want to check git status" → `/mcp git status` (remember it's MCP!)
> "I want to run a skill" → `/skill refine-text` (remember it's a Skill!)

### The Vision: Unified Flat Namespace

All tools should be invocable directly by name, regardless of source:

```
/git status        # Just works (routed to MCP: git-server)
/search query      # Just works (routed to Native: search)
/refine-text       # Just works (routed to Skill: refine-text)
/en some text      # Just works (routed to Custom: prompt rule)
```

The UI shows tool source via **icons and badges**, not via command prefixes.

## What Changes

### 1. Remove `/mcp` and `/skill` as User-Facing Namespaces

**Before:**
- `/mcp` is a builtin command that acts as a namespace
- `/skill` is a builtin command that acts as a namespace
- Users type `/mcp git status` or `/skill refine-text`

**After:**
- `/mcp` and `/skill` removed from BUILTIN_COMMANDS
- MCP tools registered directly: `/git`, `/fs`, `/github`, etc.
- Skills registered directly: `/refine-text`, `/code-review`, etc.
- Source shown via icon badge in command completion UI

### 2. Unified Command Registry with Conflict Resolution

When multiple sources provide the same command name, apply priority:

```
Priority: System Builtin > Native Capability > User Custom > MCP Tool > Skill
```

**Conflict Resolution Strategy:**
1. **System Builtin** (highest): `/search`, `/video`, `/chat` are reserved
2. **Native Capability**: System-provided capabilities
3. **User Custom**: User-defined prompt rules (config.toml `[[rules]]`)
4. **MCP Tool**: Tools from MCP servers
5. **Skill** (lowest): Claude Agent skills

**If conflict detected:**
- Lower priority tool gets auto-renamed: `{name}` → `{name}-{source}` (e.g., `search-mcp`)
- Warning logged for developer awareness
- User notified in Settings UI about shadowed tools

### 3. Command Completion UI Changes

**Before:**
```
/           → [search, mcp, skill, video, chat, en, ...]
/mcp        → [git, fs, github, ...]  (navigate into namespace)
/skill      → [refine-text, code-review, ...]  (navigate into namespace)
```

**After:**
```
/           → [search, video, chat, git, fs, refine-text, en, ...]
                ↑System  ↑System ↑System ↑MCP ↑MCP  ↑Skill    ↑Custom
```

Each command shows:
- **Icon**: Tool-specific icon (magnifyingglass, folder, etc.)
- **Name**: Direct command name
- **Description**: What it does
- **Badge**: Source indicator (System, MCP, Skill, Custom)

### 4. Routing Architecture Update

**Current L1 Routing:**
```rust
// Pattern: ^/mcp\s+(.+)
// Then parse the rest as MCP tool name + args
```

**New L1 Routing:**
```rust
// Each MCP tool has its own L1 pattern
// /git\s+(.+) → route to mcp:git-server
// /fs\s+(.+) → route to mcp:filesystem
```

**ToolRegistry Changes:**
- `register_mcp_tools()` creates root-level entries
- Each tool gets its own routing regex: `^/{tool_name}\s+`
- Tool ID format: `mcp:{server}:{tool}` (internal), but command is just `/{tool}`

### 5. Backward Compatibility (Optional Prefix Mode)

For power users who prefer explicit namespacing:

```toml
[dispatcher]
flat_namespace = true      # Default: true (new behavior)
# flat_namespace = false   # Opt-out: keep /mcp and /skill prefixes
```

When `flat_namespace = false`:
- `/mcp` and `/skill` namespaces remain
- All MCP tools also available via flat names (dual access)

## Impact

### Affected Components

| Component | Change |
|-----------|--------|
| `builtin_defs.rs` | Remove `/mcp` and `/skill` from BUILTIN_COMMANDS |
| `registry.rs` | Register MCP tools and Skills as root commands |
| `registry.rs` | Add conflict resolution logic |
| `types.rs` | Add `original_name` field for shadowed tools |
| `CommandCompletionManager.swift` | Remove namespace navigation logic |
| `SubPanelView.swift` | Update to show flat list with badges |
| `aleph.udl` | Remove namespace-related APIs |

### Breaking Changes

- **User-facing**: `/mcp git status` → `/git status`
- **User-facing**: `/skill refine-text` → `/refine-text`
- **Internal**: CommandCompletionManager no longer has `navigateIntoNamespace()`

### Migration

1. **Automatic command translation** during transition period:
   - Input `/mcp git status` → warn user, still execute
   - Suggest: "Tip: You can now use `/git status` directly"

2. **Config migration** (if any user has custom rules with `/mcp` prefix):
   - Show warning in Settings UI
   - Suggest updating to flat names

## Success Criteria

1. **User types `/git status`** → Git MCP tool executes correctly
2. **User types `/refine-text`** → Skill executes correctly
3. **Command completion shows all tools in flat list** with source badges
4. **Conflict between MCP `search` and System `search`** → System wins, MCP renamed to `search-mcp`
5. **Settings UI shows all tools** without namespace hierarchy
6. **L3 router sees all tools** in flat list for intent detection
7. **No `/mcp` or `/skill` in user-facing documentation**

## References

- Depends on: `unify-tool-registry` (provides ToolRegistry infrastructure)
- Related: `Unified-Flat-Namespace.md` (initial design sketch)
- Pattern reference: Raycast command system (flat, no namespaces)
