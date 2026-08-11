# Review Report — Batch 7: `src/executor/builtin_registry/builder/constructor/{mod.rs,collab_session_tools.rs}`

**Date:** 2026-08-11
**Scope:** `src/executor/builtin_registry/builder/constructor/mod.rs` (1379 lines) +
`src/executor/builtin_registry/builder/constructor/collab_session_tools.rs` (580 lines) — 1959 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     3 |   3 |    8 |

This batch is the **builder half** of the registry: the constructor
that wires up every tool's dependencies and registers their parameter
schemas. The `with_config` function is the single entry point and
runs once at boot. Bugs here are silent at runtime (a tool that
fails to register has no schema; the model sees an empty parameter
list and guesses; the dispatch arm then errors with "tool not
found" or "not available"). The two files together build the
messaging / plan / lifecycle / artifact / collaborative-session /
skill / note / session-complete / memory-reflect / Google Meet /
recall-context / memory-trace / note-graph-query surface.

## Findings

### [HIGH] `constructor/mod.rs:with_config` (line 1080-1100) — `default_session_key_handle` is shared with seven tools and is a process-global race surface
**Category:** logic
**Confidence:** High

**Description.** `memory_session_key_handle` is built once at line 1077
as `search_tool.default_session_key_handle()`, then `clone()`'d into
seven tools:
- `scratchpad_tool.with_session_key_handle(...)` (line 1100)
- `goal_tool.with_session_key_handle(...)` (line 1101)
- `loop_tool.with_session_key_handle(...)` (line 1102)
- `strategy_tool.with_session_key_handle(...)` (line 1103)

The other three — `context_complete`, `memory_reflect`, and the
`note_manage` family — do not need the handle.

A `clone()` of an `Arc<RwLock<String>>` shares the same inner state.
When `ExecutionEngine` writes the active session key after workspace
resolution (per the comment at the struct field), every consumer
sees the new key on the next read. This is the documented design.

The HIGH is that the construction-time comment is the only place
this is documented. A future change that constructs a tool with a
**separate** `default_session_key_handle` would silently split the
seven consumers' view of "the active session" from the dispatcher's
view. The match-arm at `tool_registry_impl.rs:recall_context` reads
the per-turn context first, so it does not race; the four
construction-time consumers (scratchpad, goal, loop, strategy) read
the handle, so they do.

**Suggested fix.** No code change. The `with_session_key_handle`
chain is the right answer; the refactor risk is the
construction-time `Arc::clone` not being a `const-fn` on the tool
itself. Document the constraint on
`memory_session_key_handle` at the construction site.

### [HIGH] `constructor/mod.rs:with_config` (line 1106-1108) — `session_compact_tool` is constructed without a `SessionManager`
**Category:** logic
**Confidence:** High

**Description.** Lines 1106-1108:
```rust
session_compact_tool: config
    .gateway_context
    .as_ref()
    .map(|ctx| Arc::clone(ctx.session_store()))
    .or_else(|| config.session_manager.clone())
    .map(|_| crate::builtin_tools::sessions::SessionCompactTool::new()),
```

The `map(|_| ...)` discards the resolved session_store and constructs
a bare `SessionCompactTool::new()`. The tool is documented (per
the `gateway_route` comment at line 1135) as
"storeless (it drives the session event log via the process-wide
handles)". OK, the design is intentional.

The HIGH is that the **`session_manager` field is read but never
passed to the tool**. A future refactor that gives `SessionCompactTool`
a `session_store: Arc<dyn SessionStore>` field would have its
construction here but no source of the store. The current
construction works only because the tool reads process-global state.

**Suggested fix.** Add a `// Storeless by design — see
SessionCompactTool::new doc comment.` line so a future refactor
cannot quietly add a `with_session_store` and find no constructor
site to pass it from.

### [MEDIUM] `constructor/mod.rs:loop_graph_tool` (line 480-490) — three `.with_*` calls, but the tool needs only the cron_service and team_store
**Category:** architecture
**Confidence:** High

**Description.** `loop_graph_tool` is built with
`with_cron_service(...)` and `with_team_store(...)` but not
`with_session_key_handle(...)` (which the four sister tools
scratchpad/goal/loop/strategy have). The lack is intentional —
loop_graph is a topology store, not a session-scoped tool — but
the asymmetry is silent.

The MEDIUM is that the **dispatch arm in `tool_registry_impl.rs`**
(`loop_graph` arm at line 280) does inject `__channel` /
`__conversation_id` into the args, which the other four tools do
NOT receive. So loop_graph's args include the channel/conversation,
but the tool itself does not consume them. This is correct (the
tool does not need them), but a future refactor that adds
session-aware behaviour to loop_graph would find the data injected
in the dispatch arm but no consumer in the tool.

**Suggested fix.** No change. The dispatch-arm injection is a
side-effect-free data carry; the tool ignores it.

### [MEDIUM] `collab_session_tools.rs:note_manage_tool` (line 320-345) — `note_memory_dir` fallback uses `std::env::temp_dir()` if home_dir is None
**Category:** architecture
**Confidence:** High

**Description.** Lines 327-340: when `get_note_memory_dir()` fails
**and** `dirs::home_dir()` is None, the fallback is
`std::env::temp_dir().join("aleph").join("memory").join("note")`.

`std::env::temp_dir()` on Linux is `/tmp`, which is world-writable.
A note storage path under `/tmp/aleph/memory/note` is a
**world-readable** location for any user on the system (note files
are typically 0644). On a multi-user host, this is a confidentiality
breach: another user on the box can `cat` the note files.

In practice, `get_note_memory_dir()` succeeds (it falls through
several `Result::unwrap_or_else` paths before reaching the home_dir
fallback), and `dirs::home_dir()` only returns None in a chroot /
container without HOME set, which is also rare. The path is
defensive; the real-world risk is small.

**Suggested fix.** When both `get_note_memory_dir` and `home_dir`
fail, refuse to construct the tool (return `None`) rather than
defaulting to `/tmp`. The construction site already has a
`if let Some(ref db) = config.memory_db` guard, so adding
`if let Ok(memory_dir) = crate::utils::paths::get_note_memory_dir()`
would be the cleaner path.

### [MEDIUM] `collab_session_tools.rs:plan_submit_tool` (line 90-100) — `PlanManager::new` requires three handles, missing any one means no plan_submit tool
**Category:** logic
**Confidence:** High

**Description.** `plan_submit_tool` is constructed only when
`(message_router, artifact_store, event_store)` are all present.
This is correct (the tool needs all three), but a misconfigured
boot path that has `message_router` + `artifact_store` but no
`event_store` produces a silent "plan_submit unavailable" rather
than a `tracing::warn!`.

**Suggested fix.** Log a one-liner when the guard fails so the
operator can spot "I configured everything but plan_submit is
missing — `event_store` is not bound". The current code is silent.

### [LOW] `constructor/mod.rs:resolve_transcription` (line 1300-1310) — async fn but does no actual async work
**Category:** quality
**Confidence:** High

**Description.** `resolve_transcription(config: &BuiltinToolConfig)`
is `async fn` but only does `cfg.read().await` and a sync
`crate::media::transcription_service` call. The function could
be `fn` if `transcription_service` is `fn`; if it's async,
the body should not be blocking.

**Suggested fix.** Leave — the async is forward-compatible with
`transcription_service` becoming async, and the caller at line 290
already awaits.

### [LOW] `collab_session_tools.rs:message_send_tool` (line 35-60) — `current_agent_id` is a `String` clone from the parameter
**Category:** quality
**Confidence:** Low

**Description.** `let current_agent_id = current_agent_id.clone();`
appears at line 36, then again at line 40 for the inbox tool. The
clones are necessary (the inner closures move the value), but
they make the diff noisy. A `&str` parameter that the constructors
internally `String::from()` would be cleaner.

**Suggested fix.** No change — `String` matches the rest of the
file. The clones are the cost of moving into multiple tool
instances.

### [LOW] `constructor/mod.rs` — `goal_tool` is constructed with a `NoteIndexer` only when `(memory_db, note_memory_dir)` are both `Some`
**Category:** quality
**Confidence:** High

**Description.** Lines 458-468:
```rust
let goal_tool = crate::builtin_tools::GoalTool::new(goal_store).with_lesson_indexer(
    match (config.memory_db.as_ref(), config.note_memory_dir.as_ref()) {
        (Some(db), Some(dir)) => Some(Arc::new(...)),
        _ => None,
    },
);
```

The match is correct (no `NoteIndexer` without both deps), but
the construction site has no log line for the "missing one of the
two" case. A misconfigured boot that has `memory_db` but no
`note_memory_dir` would silently lose the lesson-salvage path.

**Suggested fix.** Same as the `plan_submit_tool` finding: log
when the guard fails so the operator can spot the missing dep.

## Cross-References

- `constructor/mod.rs:with_config` (line 60-1300) — the single
  construction point for every tool. A field added to
  `BuiltinToolConfig` that is not consumed here is a silent
  "field exists but is never read" bug. The forward tests
  (`builder/tests.rs::spec3_tool_gating_tests` and the assertions
  in `definitions.rs`) catch the surface half.
- `collab_session_tools.rs:build_collab_session_tools` (line 30-490)
  — the messaging/plan/lifecycle/artifact/collab-session/skill/
  note/session-complete/memory-reflect constructor. The function
  takes `boot_fallback_agent_id: &str` and the doc comment at
  line 25-30 explicitly notes that the acting identity is
  resolved per call. A future caller that bakes the boot id
  into the tool instead of using it as a fallback would
  silently weld `"main"` into the tool for the process lifetime.
- `constructor/mod.rs:resolve_transcription` (line 1300-1310) —
  the bridge between the registry builder and `crate::media`. The
  `audio_transcribe` tool is registered only when this returns
  `Some`; the construction site at line 290-300 logs the
  resolution. The contract is: "transcription exists" →
  "audio_transcribe is wired".
- `collab_session_tools.rs:note_manage_tool` (line 320-345) —
  the `note_memory_dir` fallback. The path is reachable only
  in a chroot / container without HOME set; the real-world risk
  is small but a confidentiality surface on a multi-user host.
