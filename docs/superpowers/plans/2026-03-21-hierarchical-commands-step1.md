# Hierarchical Slash Commands — Step 1: Gateway Parsing

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert flat slash commands (`/session_new`) to hierarchical (`/session new`) with namespace-based guided mode, all within the Gateway. Client changes are Step 2.

**Architecture:** Tool names change from underscore-separated (`session_new`) to dot-separated (`session.new`). The command resolver is updated to parse space-separated input and match against dot-separated tool names. Namespace structure is derived at query time from tool name prefixes — no namespace nodes stored in ToolRegistry. Underscore input permanently supported as fallback alias.

**Tech Stack:** Rust, alephcore (ToolRegistry, CommandParser, handlers)

**Spec:** `docs/reference/2026-03-21-hierarchical-slash-commands-design.md`

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `src/dispatcher/types/unified/mod.rs` | Add `param_hint`, remove `subtools`/`has_subtools` |
| Modify | `src/dispatcher/registry/registration.rs` | Rename builtin commands to dot notation |
| Modify | `src/dispatcher/registry/query.rs` | Hierarchical `resolve_command()` with underscore fallback |
| Modify | `src/command/parser.rs` | Update `parse_async` for hierarchical results |
| Modify | `src/gateway/handlers/commands.rs` | Tree-shaped `commands.list`, `needs_interaction` in `command.execute` |
| Modify | `src/gateway/inbound_router/mod.rs` | Replace hardcoded `session_new` / `groupchat` checks |
| Modify | `src/executor/builtin_registry/registry.rs` | Update match arms to dot notation |
| Modify | `src/executor/builtin_registry/definitions.rs` | Update tool name strings |
| Modify | `src/executor/builtin_registry/builder.rs` | Update registration names |
| Modify | `src/builtin_tools/sessions/new_tool.rs` | Update `NAME` const |
| Modify | `src/builtin_tools/sessions/set_topic_tool.rs` | Update `NAME` const |
| Modify | Other builtin tool files | Update `NAME` consts where renamed |

---

### Task 1: Rename Builtin Tool `NAME` Constants

The simplest, safest first step. Change the authoritative tool name strings at their source.

**Files:**
- Modify: `src/builtin_tools/sessions/new_tool.rs` — `NAME: "session_new"` → `"session.new"`
- Modify: `src/builtin_tools/sessions/set_topic_tool.rs` — `NAME: "session_set_topic"` → `"session.rename"`
- Modify: All other builtin tools that need renaming per spec mapping table

- [ ] **Step 1: Find all NAME constants**

Run: `grep -rn 'const NAME.*=.*"' src/builtin_tools/ src/executor/builtin_registry/`

This gives the authoritative list of tool name strings. Cross-reference with the spec's naming table.

- [ ] **Step 2: Rename session tools**

In `src/builtin_tools/sessions/new_tool.rs`:
```rust
const NAME: &'static str = "session.new";
```

In `src/builtin_tools/sessions/set_topic_tool.rs`:
```rust
const NAME: &'static str = "session.rename";
```

Also update the `sessions_list` and `sessions_send` tools if they have NAME consts.

- [ ] **Step 3: Rename other tools per spec**

Follow the spec mapping table:
- `generate_image` → `image.generate`
- `generate_speech` → `speech.generate`
- `list_skills` → `skill.list`
- `read_skill` → `skill.read`
- `memory_browse` → `memory.browse`
- `cron_manage` → `cron.manage`
- `vault_store` → `vault.store`
- `agent_create` → `agent.create`
- `agent_list` → `agent.list`
- `agent_delete` → `agent.delete`
- `snapshot_capture` → `snapshot`

Keep unchanged: `search`, `webfetch`, `switch`, `groupchat` (independent commands, no namespace)

- [ ] **Step 4: Update builtin definitions list**

In `src/executor/builtin_registry/definitions.rs`, update all `name:` fields to match new dot-notation names.

- [ ] **Step 5: Update builder registration strings**

In `src/executor/builtin_registry/builder.rs`, find all `reg(tools, "session_new", ...)` calls and update to `reg(tools, "session.new", ...)`. Same for all renamed tools.

- [ ] **Step 6: Update execution match arms**

In `src/executor/builtin_registry/registry.rs`, update the large match statement:
- `"session_new"` → `"session.new"`
- `"session_set_topic"` → `"session.rename"`
- All other renamed tools

- [ ] **Step 7: Update groups**

In `src/executor/builtin_registry/groups.rs`, update tool name strings in group membership lists.

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p alephcore --lib`

Expected: Compile errors in files that reference old names (registration.rs, inbound_router). That's expected — we fix those in Task 2 and 3.

Actually — check if there are compile errors. If the NAME const is only used internally by the tool and the match arm references a string literal, they may compile independently. Fix any errors.

- [ ] **Step 9: Commit**

```bash
git add src/builtin_tools/ src/executor/builtin_registry/
git commit -m "gateway: rename builtin tool names to dot notation (session.new, agent.create, etc.)"
```

---

### Task 2: Update Command Registration

Change the builtin command entries in the dispatch registry to use dot-notation names.

**Files:**
- Modify: `src/dispatcher/registry/registration.rs`

- [ ] **Step 1: Read current registration code**

Read `src/dispatcher/registry/registration.rs` fully.

- [ ] **Step 2: Update registered command names**

Change all `UnifiedTool::new(...)` calls:

```rust
// Before:
UnifiedTool::new("builtin:session_new", "session_new", "Start a new conversation session", ToolSource::Builtin)
    .with_usage("/session_new")

// After:
UnifiedTool::new("builtin:session.new", "session.new", "Start a new conversation session", ToolSource::Builtin)
    .with_usage("/session new")
    .with_param_hint("[topic]")
```

Update all builtin commands per the naming table. Add `.with_param_hint(...)` where applicable.

- [ ] **Step 3: Add `param_hint` to UnifiedTool**

If `with_param_hint` doesn't exist yet, add it to `src/dispatcher/types/unified/mod.rs`:

```rust
pub param_hint: Option<String>,

pub fn with_param_hint(mut self, hint: &str) -> Self {
    self.param_hint = Some(hint.to_string());
    self
}
```

Also initialize `param_hint: None` in the constructor.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore --lib`

- [ ] **Step 5: Commit**

```bash
git add src/dispatcher/
git commit -m "gateway: update command registration to dot notation with param_hint"
```

---

### Task 3: Hierarchical Command Resolution

The core change — update `resolve_command` to parse hierarchical input.

**Files:**
- Modify: `src/dispatcher/registry/query.rs`

- [ ] **Step 1: Read current `resolve_command`**

Read `src/dispatcher/registry/query.rs` lines 141-188 fully.

- [ ] **Step 2: Write hierarchical resolution logic**

Replace the current resolution logic with:

```rust
pub async fn resolve_command(&self, input: &str) -> Option<ResolvedCommand> {
    let without_slash = input.strip_prefix('/').unwrap_or(input).trim();
    if without_slash.is_empty() { return None; }

    // Split input into words
    let words: Vec<&str> = without_slash.split_whitespace().collect();

    // Strip @botname from first word
    let first_word = words[0].split_once('@').map(|(n, _)| n).unwrap_or(words[0]).to_lowercase();

    let tools = self.tools.read().await;

    // Strategy 1: Try hierarchical match (space → dot)
    // "/session new my-topic" → try "session.new" with args "my-topic"
    for depth in (1..=words.len().min(3)).rev() {
        let candidate = words[..depth]
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { first_word.clone() } else { w.to_lowercase() })
            .collect::<Vec<_>>()
            .join(".");

        if let Some(tool) = tools.values()
            .filter(|t| t.is_active && t.name.to_lowercase() == candidate)
            .max_by(|a, b| a.source.priority().cmp(&b.source.priority()))
            .cloned()
        {
            let arguments = if depth < words.len() {
                Some(words[depth..].join(" "))
            } else {
                None
            };
            return Some(ResolvedCommand { tool, arguments });
        }
    }

    // Strategy 2: Underscore fallback ("/session_new" → try "session.new")
    let underscore_name = first_word.replace('_', ".");
    if underscore_name != first_word {
        if let Some(tool) = tools.values()
            .filter(|t| t.is_active && t.name.to_lowercase() == underscore_name)
            .max_by(|a, b| a.source.priority().cmp(&b.source.priority()))
            .cloned()
        {
            let arguments = if words.len() > 1 {
                Some(words[1..].join(" "))
            } else {
                None
            };
            return Some(ResolvedCommand { tool, arguments });
        }
    }

    None
}
```

Key behaviors:
- `/session new my-topic` → tries "session.new.my-topic" (no match), then "session.new" (match!) with args "my-topic"
- `/search weather` → tries "search.weather" (no match), then "search" (match!) with args "weather"
- `/session_new` → no hierarchical match → underscore fallback → "session.new" (match!)

- [ ] **Step 3: Add namespace query helper**

Add a method to find all tools under a namespace prefix:

```rust
pub async fn list_namespace_children(&self, namespace: &str) -> Vec<UnifiedTool> {
    let prefix = format!("{}.", namespace);
    let tools = self.tools.read().await;
    tools.values()
        .filter(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix))
        .filter(|t| {
            // Only direct children (one level deeper)
            let suffix = &t.name[prefix.len()..];
            !suffix.contains('.')
        })
        .cloned()
        .collect()
}

pub async fn is_namespace(&self, name: &str) -> bool {
    let prefix = format!("{}.", name);
    let tools = self.tools.read().await;
    tools.values().any(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix))
}
```

- [ ] **Step 4: Write tests**

```rust
#[tokio::test]
async fn test_hierarchical_resolution() {
    let registry = ToolRegistry::new();
    // Register a hierarchical tool
    let tool = UnifiedTool::new("builtin:session.new", "session.new", "New session", ToolSource::Builtin);
    registry.register(tool).await;

    // Space-separated input
    let resolved = registry.resolve_command("/session new my-topic").await.unwrap();
    assert_eq!(resolved.tool.name, "session.new");
    assert_eq!(resolved.arguments.as_deref(), Some("my-topic"));

    // Underscore fallback
    let resolved = registry.resolve_command("/session_new").await.unwrap();
    assert_eq!(resolved.tool.name, "session.new");
}

#[tokio::test]
async fn test_namespace_children() {
    let registry = ToolRegistry::new();
    registry.register(UnifiedTool::new("b:session.new", "session.new", "", ToolSource::Builtin)).await;
    registry.register(UnifiedTool::new("b:session.list", "session.list", "", ToolSource::Builtin)).await;
    registry.register(UnifiedTool::new("b:search", "search", "", ToolSource::Builtin)).await;

    let children = registry.list_namespace_children("session").await;
    assert_eq!(children.len(), 2);
    assert!(registry.is_namespace("session").await);
    assert!(!registry.is_namespace("search").await);
}
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test -p alephcore --lib -- resolve_command namespace_children`

- [ ] **Step 6: Commit**

```bash
git add src/dispatcher/
git commit -m "gateway: hierarchical command resolution with namespace support"
```

---

### Task 4: Update `commands.list` RPC to Return Tree Structure

**Files:**
- Modify: `src/gateway/handlers/commands.rs`

- [ ] **Step 1: Read current handler**

Read `src/gateway/handlers/commands.rs` lines 78-92.

- [ ] **Step 2: Rewrite `handle_list_from_registry` to return tree**

The handler should:
1. Get all tools from registry
2. Group tools by namespace prefix (derive from dot-separated name)
3. Build tree response: namespaces with children + independent commands

```rust
pub async fn handle_list_from_registry(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools = tool_registry.list_root_commands().await;
    let tree = build_command_tree(&tools);
    JsonRpcResponse::success(request.id, json!({ "commands": tree }))
}

fn build_command_tree(tools: &[UnifiedTool]) -> Vec<serde_json::Value> {
    let mut namespaces: BTreeMap<String, Vec<&UnifiedTool>> = BTreeMap::new();
    let mut independent: Vec<&UnifiedTool> = Vec::new();

    for tool in tools {
        if let Some(dot_pos) = tool.name.find('.') {
            let ns = &tool.name[..dot_pos];
            namespaces.entry(ns.to_string()).or_default().push(tool);
        } else {
            independent.push(tool);
        }
    }

    let mut result = Vec::new();

    for (ns, children) in &namespaces {
        result.push(json!({
            "name": ns,
            "is_namespace": true,
            "hint": format!("{} management", capitalize(ns)),
            "children": children.iter().map(|t| {
                let action = t.name.split('.').last().unwrap_or(&t.name);
                json!({
                    "name": action,
                    "hint": t.description,
                    "param_hint": t.param_hint,
                })
            }).collect::<Vec<_>>(),
        }));
    }

    for tool in &independent {
        result.push(json!({
            "name": tool.name,
            "is_namespace": false,
            "hint": tool.description,
            "param_hint": tool.param_hint,
        }));
    }

    result
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --lib`

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/commands.rs
git commit -m "gateway: commands.list returns tree structure with namespaces"
```

---

### Task 5: Update `command.execute` RPC for `needs_interaction`

**Files:**
- Modify: `src/gateway/handlers/commands.rs`

- [ ] **Step 1: Rewrite `handle_execute`**

When the input is a namespace without subcommand, return `needs_interaction: true` with children list:

```rust
pub async fn handle_execute(
    request: JsonRpcRequest,
    command_parser: Arc<CommandParser>,
    tool_registry: Arc<ToolRegistry>,
) -> JsonRpcResponse {
    let params: ExecuteParams = match parse_params(&request) { ... };
    let input = params.input.trim();
    let slash_input = if input.starts_with('/') { input.to_string() } else { format!("/{}", input) };

    // Try to resolve as a full command
    match command_parser.parse_async(&slash_input).await {
        Some(parsed) => {
            // Resolved! Return command info
            JsonRpcResponse::success(request.id, json!({
                "resolved": true,
                "command": { "namespace": ..., "action": ..., "args": ..., "internal_id": ..., "source_type": ... }
            }))
        }
        None => {
            // Not resolved — check if it's a namespace
            let words: Vec<&str> = slash_input.trim_start_matches('/').split_whitespace().collect();
            let candidate_ns = words.join(".");

            if tool_registry.is_namespace(&candidate_ns).await {
                let children = tool_registry.list_namespace_children(&candidate_ns).await;
                JsonRpcResponse::success(request.id, json!({
                    "resolved": false,
                    "needs_interaction": true,
                    "namespace": candidate_ns.replace('.', " "),
                    "children": children.iter().map(|t| {
                        let action = t.name.split('.').last().unwrap_or(&t.name);
                        json!({ "name": action, "hint": t.description, "param_hint": t.param_hint })
                    }).collect::<Vec<serde_json::Value>>(),
                }))
            } else {
                // Check if subcommand typo within known namespace
                if words.len() >= 2 {
                    let parent_ns = words[..words.len()-1].join(".");
                    if tool_registry.is_namespace(&parent_ns).await {
                        let children = tool_registry.list_namespace_children(&parent_ns).await;
                        return JsonRpcResponse::success(request.id, json!({
                            "resolved": false,
                            "error": format!("Unknown subcommand: {}", words.last().unwrap()),
                            "needs_interaction": true,
                            "namespace": parent_ns.replace('.', " "),
                            "children": children.iter().map(|t| {
                                let action = t.name.split('.').last().unwrap_or(&t.name);
                                json!({ "name": action, "hint": t.description, "param_hint": t.param_hint })
                            }).collect::<Vec<serde_json::Value>>(),
                        }));
                    }
                }

                JsonRpcResponse::success(request.id, json!({
                    "resolved": false,
                    "error": format!("Unknown command: {}", slash_input),
                }))
            }
        }
    }
}
```

Note: `handle_execute` now needs `tool_registry` in addition to `command_parser`. Update the handler registration in `agent_init.rs` to pass both.

- [ ] **Step 2: Update handler registration**

In `src/bin/aleph-server/commands/start/builder/agent_init.rs`, update the `command.execute` registration to pass `dispatch_registry` as well:

```rust
server.handlers_mut().register("command.execute", move |req| {
    let p = parser.clone();
    let r = registry.clone();
    async move {
        alephcore::gateway::handlers::commands::handle_execute(req, p, r).await
    }
});
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --bin aleph-server`

- [ ] **Step 4: Commit**

```bash
git add core/
git commit -m "gateway: command.execute supports needs_interaction for namespace commands"
```

---

### Task 6: Update Inbound Router

Remove hardcoded `session_new` and `groupchat` checks. Let the hierarchical resolver handle them.

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs`

- [ ] **Step 1: Read current routing code**

Read `src/gateway/inbound_router/mod.rs` lines 290-326.

- [ ] **Step 2: Update hardcoded command checks**

Replace:
```rust
if parsed.command_name == "session_new" {
    return self.handle_new_session(&msg, &ctx).await;
}
```

With:
```rust
if parsed.command_name == "session.new" {
    return self.handle_new_session(&msg, &ctx).await;
}
```

Same for `groupchat` (stays as `groupchat` — independent command, no rename).

Update the fallback checks too:
```rust
if trimmed == "/session_new" {
```
→
```rust
if trimmed == "/session_new" || trimmed == "/session new" {
```

Actually per spec, the resolver now handles `/session new` → `session.new`, so the fallback should check for `"/session.new"` or just remove it (the resolver handles it).

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --lib`

- [ ] **Step 4: Commit**

```bash
git add src/gateway/inbound_router/
git commit -m "gateway: update inbound router for dot-notation command names"
```

---

### Task 7: Full Build and Test Verification

- [ ] **Step 1: Build all**

```bash
cargo check -p alephcore --lib && cargo check -p alephcore --bin aleph-server && cargo check -p aleph-cli && cargo check -p aleph-tui
```

- [ ] **Step 2: Run core tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```

Fix any test failures caused by renamed tool names (tests may reference old names like `"session_new"`).

- [ ] **Step 3: Grep for stale old names**

```bash
grep -rn '"session_new"\|"sessions_list"\|"session_set_topic"\|"generate_image"\|"generate_speech"\|"list_skills"\|"read_skill"\|"memory_browse"\|"cron_manage"\|"vault_store"\|"agent_create"\|"agent_list"\|"agent_delete"\|"snapshot_capture"' src/ --include='*.rs'
```

Any remaining old names need updating. Common locations:
- Test assertions
- Error messages
- Documentation comments

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "gateway: fix stale tool name references after hierarchical rename"
```

---

## Step 1 Complete Checklist

- [ ] All builtin tool `NAME` constants use dot notation (`session.new`, `agent.create`, etc.)
- [ ] Command registration uses dot notation with `param_hint`
- [ ] `resolve_command()` supports hierarchical parsing + underscore fallback
- [ ] `commands.list` returns tree structure (namespaces + independent commands)
- [ ] `command.execute` returns `needs_interaction` for namespace-only input
- [ ] Inbound router uses new command names
- [ ] All core tests pass
- [ ] No stale old-format tool name references remain
