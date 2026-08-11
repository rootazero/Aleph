# Review Report — Batch 6: `src/executor/builtin_registry/registry/*`

**Date:** 2026-08-11
**Scope:** `src/executor/builtin_registry/registry/{mod.rs,struct_def.rs,inherent.rs,free_fns.rs,tool_registry_impl.rs,tests.rs}` — ~2780 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     5 |   2 |   10 |

This batch is the central tool-dispatch seam. Every model-side tool
call funnels through `ToolRegistry::execute_tool`, which is a single
`match tool_name` over every builtin (deliberately indivisible per
the file's module-level comment). The findings here are about the
match's correctness, the oncecell-injection pattern, and the
identity-resolution helpers (`caller_agent_id`, `caller_memory_partition`).

## Findings

### [HIGH] `tool_registry_impl.rs:execute_tool` `_ =>` arm (line 1611-1633) — unknown tool error message leaks the name verbatim
**Category:** logic
**Confidence:** Medium

**Description.** The `_ =>` arm at the end of the match produces
`AlephError::tool(format!("Unknown tool: {tool}"))` where `tool` is the
tool name. The name is what the model asked for, which is generally
the same name the LLM was told — but a malicious or buggy harness
that asks for `"<internal:secret-leak>"` (a name NOT in the model
list) would get back the verbatim name. The error then propagates
to the LLM as a tool result, and the LLM sees "Unknown tool:
<internal:secret-leak>".

This is a small information disclosure. A defensive error message
would just say "Unknown tool: redacted" or omit the name entirely.

**Suggested fix.** Truncate the name to the first 64 chars before
echoing it back. The model is told the tool list; the model knows
what it asked for; the verbatim echo is for diagnostics.

### [HIGH] `tool_registry_impl.rs:loop_graph` (line 280-330) — args mutation happens in the dispatch arm, but `current_turn_context()` is the only source
**Category:** logic
**Confidence:** High

**Description.** The `loop_graph` arm reads `current_turn_context()` and
`session_context_handle` to inject `__channel` and `__conversation_id`
into the args, but it does so in the **dispatch arm** before the
`Box::pin`. This is correct in the sense that the closure captures
the mutated `args`, but the same pattern is duplicated across
`loop_graph`, `cron_manage`, `heartbeat_create`, `session_new`,
`session_compact`, `session_rename`, `session_set_mode`, and the
`agent_create|...|agent_update` block (line 836-908).

The duplication is fragile: a future change to the `TurnContext`
shape (e.g. adding `thread_id`) must touch **eight** arms. The
existing `inject_delivery_route` helper at `inherent.rs:130-150` is
the single source for two of the eight arms (cron_manage and
heartbeat_create), but the others roll their own.

**Suggested fix.** Promote `inject_delivery_route` to take an
explicit `Option<(&str, &str)>` (channel, conversation_id) and
have ALL eight arms go through it. The current shape requires
`&self`, which the dispatch arm does not have; the refactor is
to make it a free function that takes the two values explicitly.

This is a wider refactor than this pass. For now, the duplication
is documented in code and the existing `inject_delivery_route`
helper covers the two most-frequently-touched arms.

### [HIGH] `inherent.rs:caller_agent_id` (line 100-115) — fallback to `caller_agent_id` of `"main"` masks config errors
**Category:** logic
**Confidence:** High

**Description.** `caller_agent_id(fallback)` reads the per-turn
`TURN_CONTEXT` first, then falls back to `session_context_handle`,
then to the `fallback` string. The default `fallback` at every
caller is `"main"` (e.g. `tool_registry_impl.rs:425`, `:546`,
`:563`). A future caller that does NOT pass a fallback would get
`unwrap_or_default()` for the Option, which is `""` — but no
production caller relies on that.

The HIGH is the **silent swallow**: a tool that fails to resolve
its identity because both the per-turn context and the shared
handle are missing falls through to `"main"`. The tool then
operates on `main`'s memory, `main`'s session, `main`'s ACL. If
the actual run is by a sub-agent named `researcher`, the tool
silently writes to `main`'s partition.

**Suggested fix.** Either:
1. Log a `tracing::warn!` when both sources are missing and a
   fallback is used (so the operator can spot misconfigured
   contexts), or
2. Return `Result<String, AlephError>` and have the dispatch arm
   surface the missing identity as a structured error to the
   model.

Path (1) is the smaller change and matches the rest of the file's
diagnostic style.

### [MEDIUM] `tool_registry_impl.rs:execute_tool` — `_ =>` arm does not consult `resolve_plugin_handler` first
**Category:** logic
**Confidence:** Low

**Description.** The `_ =>` arm checks `resolve_plugin_handler` (line
1611) and only after that returns "Unknown tool". This is correct
behavior for plugin tools. But the check is at the END of the
match, after the `match` exhausts every builtin name. A builtin
name that is misspelled in the match arms (e.g. `"agent_creates"`
vs `"agent_create"`) would fall through to the plugin check and
return "Unknown tool" with a confusing error path.

The forward test in `tests.rs` (e.g. `channel_tool_dispatch_tests`)
catches this for the four channel tools. A wider test that asserts
every `BUILTIN_TOOL_DEFINITIONS` name has a match arm would be the
strongest check, but that test is `definitions.rs`'s
`every_registered_core_tool_is_accounted` (already in place).

**Suggested fix.** No change. The tests cover the surface.

### [MEDIUM] `tool_registry_impl.rs:agent_create | ... | agent_update` (line 836-908) — `__conversation_id` is never injected
**Category:** logic
**Confidence:** High

**Description.** The agent-management arm injects `__channel` but not
`__conversation_id`. The other arm (line 280, loop_graph) injects
both. The `session_context_handle` carries `conversation_id`, and
the per-turn context carries it too, so the data is available. But
agent_switch / agent_unbind / agent_create need `__conversation_id`
to bind the agent to a specific chat (e.g. Telegram's chat_id is
the `conversation_id`, not the `channel`).

A tool that runs `agent_switch` with `__channel` set but no
`__conversation_id` will rebind the wrong chat — the binding goes
to the channel's default conversation rather than the one the user
is currently in.

**Suggested fix.** Mirror the `loop_graph` pattern:

```rust
let (channel, conversation_id) = match crate::tools::turn_context::current_turn_context() {
    Some(t) => (Some(t.channel_id.clone()), Some(t.conversation_id.clone())),
    None => self
        .session_context_handle
        .as_ref()
        .and_then(|h| h.try_read().ok())
        .map(|ctx| (Some(ctx.channel.clone()), Some(ctx.conversation_id.clone())))
        .unwrap_or((None, None)),
};
if let Some(obj) = args.as_object_mut() {
    if let Some(channel) = channel.filter(|c| !c.is_empty()) {
        obj.insert("__channel".into(), serde_json::Value::String(channel));
    }
    if let Some(conversation_id) = conversation_id.filter(|c| !c.is_empty()) {
        obj.insert("__conversation_id".into(), serde_json::Value::String(conversation_id));
    }
}
```

This is a behaviour change: a previously-working agent_switch might
now bind to a different conversation. The risk is acceptable
because the current behavior is a bug (binds to default rather than
the current chat).

### [MEDIUM] `struct_def.rs:BuiltinToolRegistry` — 80+ fields, the `tools: HashMap<String, UnifiedTool>` is the only field used at lookup time
**Category:** architecture
**Confidence:** High

**Description.** The struct carries ~80 tool fields (each
`Option<…>` or `crate::builtin_tools::X`). `get_tool(name)` looks up
in `self.tools: HashMap<String, UnifiedTool>`, which is populated by
the builder. The individual tool fields are read only by the match
arms in `execute_tool`. A field that is constructed but never read
(silent field) would compile, run, and never be exercised.

The forward tests (`tests.rs::channel_tool_dispatch_tests`) and the
assertion in `definitions.rs` catch the matching-arm half. A field
that is constructed but never read is not caught by any test.

**Suggested fix.** This is a wider refactor. A macro-based
`BuiltinToolRegistryBuilder` that emits one `pub(crate)` field per
`reg(...)` site in `core_tools.rs` and asserts every field is
referenced from `execute_tool` would close the gap. Out of scope
for this pass.

### [MEDIUM] `inherent.rs:set_node_registry` (line 175-185) — `Arc::get_mut` success is the only way the setter works
**Category:** logic
**Confidence:** High

**Description.** `set_node_registry(&self, registry: Arc<...>)` uses
`self.node_registry.set(registry)` to write the OnceCell. The
OnceCell's `set` returns `Result<(), T>` and the code uses
`is_ok()` to log only on first success. A second call (which
should never happen) is silently ignored.

The MEDIUM is that the comment at line 178-184 says
"this is the only setter for this cell", but the structure is
shared with `set_node_security_store`, `set_config_patcher`,
`set_config_broadcaster`, `set_memory_reflector`, and
`set_query_filer`. All five use the same `OnceCell::set` pattern
with a `is_ok()` log gate. A bug in any one of them is mirrored
across all five.

**Suggested fix.** Extract a `fn set_once<T>(cell: &OnceCell<T>, value: T, label: &str)`
helper at the top of `inherent.rs` that does the `set` and the log
in one place. The five call sites all become one line.

### [MEDIUM] `free_fns.rs:parse_caller_agent_id` (line 30-35) — comment block is correct but the test gap for `peer:` legacy form is implicit
**Category:** quality
**Confidence:** Low

**Description.** The function delegates to `SessionKey::from_key_string`
and falls back to the literal fallback. The comment at line 5-30
documents the historical bug (`split(':').next()` returned the
namespace prefix) and the test at `tests.rs` covers five key
shapes. The legacy `peer:` form is mentioned in the comment but
not in the test.

**Suggested fix.** Add a regression test:

```rust
#[test]
fn extracts_agent_id_from_legacy_peer_key() {
    assert_eq!(parse_caller_agent_id("peer:legacy-user", "fallback"), "legacy-user");
}
```

(Assuming `SessionKey::from_key_string` supports the legacy form;
if it does not, the comment should be removed.)

### [LOW] `struct_def.rs` — no `Default` impl
**Category:** quality
**Confidence:** Low

**Description.** `BuiltinToolRegistry` has no `Default` impl. Every
construction goes through `with_config`. This is intentional
(`with_config` does 1300+ lines of work), but it means a future
`Default` consumer (a test that wants an empty registry) would
have to construct one with `BuiltinToolConfig::default()`.

**Suggested fix.** No change — `with_config` is the only legitimate
constructor. The `Default` impl would be 80 lines of `Option::None`
which is worse than the current `BuiltinToolConfig::default() +
with_config(default())` path.

### [LOW] `tests.rs:recall_context_identity_tests` — `IsolatedAlephHome` is reused across tests in the same module
**Category:** quality
**Confidence:** High

**Description.** Each test creates `IsolatedAlephHome` and
`BuiltinToolRegistry::new()`. The `new()` constructor opens
`goal_store` (a SQLite file under `data_dir/goals.db`). The
constructor comment at `constructor/mod.rs:452-462` says the store
is opened under whatever `ALEPH_HOME` says, which is process-global.

The `IsolatedAlephHome` test guard scopes `ALEPH_HOME` to a tempdir,
so each test gets a fresh store. The test comment at
`recall_context_identity_tests` line 24-30 explicitly says: "without
this the test both touches the developer's real `~/.aleph/data` and
races any sibling holding an `IsolatedAlephHome`".

The LOW is that this is the third or fourth time this pattern shows
up. A `RegistryTestHarness` that wraps `IsolatedAlephHome` +
`BuiltinToolRegistry::new()` in a single struct would close the
duplication.

**Suggested fix.** Out of scope for this pass. The pattern is
documented and the duplication is bounded.

## Cross-References

- `tool_registry_impl.rs:execute_tool` (line 60-1633) — the
  single match. Every tool's dispatch path is one arm. The
  duplication of `__channel` / `__conversation_id` injection across
  eight arms is the widest refactor surface.
- `inherent.rs:caller_agent_id` (line 100-115) — the identity
  resolver used by every tool that needs the per-turn agent. The
  `caller_memory_partition` and `caller_profile_partition` helpers
  compose on top. See Batch 7 (builder) for the consumer side.
- `free_fns.rs:parse_caller_agent_id` (line 30-35) — the
  single source for parsing the agent id out of a session key
  string. The `SessionKey::from_key_string` parser is the actual
  implementation; the helper is the registry-side alias.
- `tests.rs:every_memory_dispatch_arm_composes_the_partition` (line
  1770-1810) — the strongest test in this batch. Asserts every
  memory/note dispatch arm goes through `caller_memory_partition`
  (or `caller_profile_partition` for the one reader that must
  refuse inside a room).
