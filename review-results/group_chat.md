# Module: src/group_chat

- Path: `src/group_chat/`
- Files scanned: 8
- Total LOC: 2913
- Date: 2026-08-12
- Reviewer: static (four-perspective checklist: security / logic / architecture / quality)
- Branch: `fix/review-group-chat`
- Worktree: `/tmp/aleph-review-group-chat`

## Summary

| Severity | Count |
|----------|------:|
| critical | 2 |
| high     | 5 |
| medium   | 6 |
| low      | 7 |
| **Total**| **20** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness

```
ISSUE|src/group_chat/executor.rs:153|critical|`execute_round` mutates session state and persists turns incrementally
(user turn → coordinator → persona 1 → persona 2 → ...). A failure on persona N leaves the session
with (N-1) committed persona turns plus the user turn + a coordinator turn at the persisted DB,
and the in-memory `session.history` carries the same orphan rows. On the next call
`current_round.saturating_add(1)` advances to a NEW round, so the partial round is silently
abandoned and any subsequent `get_group_chat_turns` query returns an incomplete round that is
impossible to replay or roll back. Effect: a 503 from the persona provider poisons the
session's history permanently (R7 silent corruption). Fix: collect every persisted/memory turn
in a single transaction-like staging area and only commit when all respondents complete (or
rollback by popping the in-memory history and not persisting partial turns).
```

```
ISSUE|src/group_chat/session.rs:88|high|`GroupChatSession::new` reads
`crate::scope::current_scope()` and stores `owner_user_id`. The doc comment (lines 51-69)
explicitly positions this field as the P1 ownership stamp mirrored from
`SessionMetadata::stamp_attribution`. BUT: (a) the `insert_group_chat_session` SQL
(`src/resilience/database/group_chat.rs:37-58`) writes only `id, topic, status,
source_channel, source_session_key, created_at, updated_at` — `owner_user_id` is never
persisted; (b) the `group_chat_sessions` schema (`src/resilience/database/state_database/
schema.rs:310-319`) has no `owner_user_id` column. So the ownership stamp is silently
dropped on every reload — `stamped_owner_visible`-style visibility queries see `None`
forever and fall through to the operator-default branch (R7 ownership data loss, R1
P1-stamp contract violation).
```

```
ISSUE|src/group_chat/coordinator.rs:59-67|high|`build_coordinator_prompt` formats each
persona line as `- id="{p.id}" name="{p.name}" prompt="{truncated}"`. Persona `id` and
`name` come from configuration (preset) OR from inline `/groupchat --role` commands (any
operator-supplied text). Neither is escaped. A persona name containing `"` produces a
line like `- id="x" name="she said "hi"" prompt="..."` which is malformed JSON-sample
spans the LLM must mimic for its output. The resulting CoordinatorPlan JSON can be
mis-parsed on the OUTPUT side by `parse_coordinator_plan` or, more subtly, can bias the
LLM to emit syntactically-broken plans that fall through to `build_fallback_plan`
without the operator ever seeing the real failure mode. Fix: replace `"` with `”` (or
strip/quote-escape) before formatting.
```

```
ISSUE|src/group_chat/executor.rs:79-97|medium|`resolve_provider` logs a `warn!` per
fallback call: `persona provider not found in registry, using default`. With `N` rounds
and `M` personas, a misconfigured persona generates N×M log lines. Tracing spans on
production gateways collapse to one of the most common lines in the log, drowning real
warnings. Fix: dedupe by `(persona_id, provider_name)` — emit the warn only on the
first miss.
```

```
ISSUE|src/group_chat/executor.rs:283-291|medium|`persona.thinking_level` is parsed and
falls back to `ThinkLevel::default()` on parse failure. `ThinkLevel`'s own docs
(`src/agents/thinking.rs:128-160`) state: "callers must REJECT rather than default.
Silently falling back would run the turn at a different depth (and a different price)
than the one the user believes they picked." Group chat is one of those callers, and
silently substituting the default (rather than letting the provider default take over)
violates the documented contract for any operator who set a typo'd thinking level.
Fix: on parse failure, drop to `None` (use the provider's default) AND emit a warn, so
the turn still runs but at the provider's documented fallback rather than at a level
the operator never asked for.
```

```
ISSUE|src/group_chat/session.rs:96-110|medium|`add_turn` updates `current_round` only
when `round > self.current_round`, but appends the turn to history unconditionally. A
caller can call `add_turn(2, …)` then `add_turn(1, …)` and get
`current_round == 2` with a stray `round == 1` row in history. `get_group_chat_turns`
orders by `round, sequence`, so the stray row floats to the top of the replay. Fix:
either reject non-monotonic rounds, or always advance `current_round = max(seen)` and
note that history rows are independent.
```

```
ISSUE|src/group_chat/orchestrator.rs:113-122|medium|`create_session` inserts the
session into the in-memory `HashMap`, then best-effort persists to the DB. If
`insert_group_chat_session` fails, the in-memory session is kept and the failure is
logged at `warn!`. From the operator's perspective, the session "exists" for the
rest of the daemon's lifetime but disappears on restart. Combined with
`end_session` (which silently fails to update the DB row if the row was never written
in the first place), this creates a class of "ghost sessions" that show up in
`all_sessions()` but never appear in `list_active_group_chats()`. Fix: either
fail-fast on DB insert error, or document the asymmetry loudly in the function doc.
```

```
ISSUE|src/group_chat/persona.rs:34-56|medium|`PersonaRegistry::resolve` does NOT
deduplicate personas. Two `PersonaSource::Inline(p)` sources with the same `p.id`
both land in `participants: Vec<Persona>` and `execute_round` finds the FIRST one via
`.iter().find(|p| p.id == respondent.persona_id)`. A `/groupchat start --role
"Foo: a" --role "Foo: b"` produces two distinct participants with id `foo`, and the
Coordinator's `arch` reference resolves to whichever happens to be first. Fix: dedupe
inline personas by `id` in `create_session` (keep first or last with a clear rule).
```

```
ISSUE|src/group_chat/channel.rs:115-145|low|`tokenize` handles `\X` escapes inside
quotes: the same-quote escape (`\\"` inside `"…"` → `"`) is documented, but ANY other
backslash escape (`\n`, `\t`, `\\X`) is pushed as `\\X` to the output (literal two
chars). A user typing `--role "Foo: a\nb"` gets the literal `a\nb` string passed
through as the persona prompt. Low severity because the parser is for human-facing
commands and the behavior is consistent, but the partial escape handling is surprising.
Fix: either fully honor `\n`/`\t`/`\\` as the corresponding char, or strip ALL
backslashes in quoted strings (documented as "literal" mode).
```

```
ISSUE|src/group_chat/channel.rs:98-104|low|`parse_inline_role` produces `id` via
`name.to_lowercase().replace([' ', '-'], "_")` but does NOT validate
`Persona::validate()`. A `/groupchat start --role ":  empty"` returns `None`
(name empty → split_once fails), but a `/groupchat start --role "x: "` is rejected
(name="x", prompt empty), and `/groupchat start --role "x: "` is similarly rejected.
However, a 2001-char prompt passes through `parse_inline_role` and only fails at
`PersonaRegistry`/`orchestrator::create_session`. This is fine for VALIDATION but
means a long prompt is parsed then re-validated — wasteful, but not a bug.
```

```
ISSUE|src/group_chat/channel.rs:28-52|low|`parse_group_chat_command` matches the
literal prefix `/groupchat` without a word boundary. `/groupchatter` (a hypothetical
non-command starting with `/groupchat`) is checked first and returns None after
`strip_prefix` + `start`/`end` check, so it's safe, but the check is wasteful. Lower
priority — the current behavior is correct, just inefficient.
```

### Perspective 2 — Logic Correctness

```
ISSUE|src/group_chat/executor.rs:248-263|medium|`coordinator_visible = true` path:
the raw coordinator LLM output (typically a JSON `CoordinatorPlan`) is pushed as the
first message AND recorded as a `Speaker::Coordinator` turn in session history. From
the channel's perspective (`send_group_chat_messages`), this is sent verbatim to the
user as `**[Coordinator]**: {raw}`. Users see raw JSON in their chat. The doc comment
calls this "include coordinator plan as a message" but doesn't note that the
"plan" is the LLM's raw output (which may include markdown fences, prose preamble,
or trailing tokens). Fix: either document loudly that `coordinator_visible` is a debug
switch, or parse the plan first and format `plan.respondents` as a human-readable
summary before exposing it.
```

```
ISSUE|src/group_chat/orchestrator.rs:148-194|low|`end_session` removes the session
from the map AND attempts to call `session.end()` on the handle. If
`handle.try_lock()` fails (someone holds the session lock), the function silently
returns the handle to the caller with a debug log and expects the caller to call
`session.end()` itself. The handler (`group_chat_handler.rs:222-228`) DOES call
`session.end()` itself, so this works in practice, but the contract is fragile —
any future caller that forgets to call `session.end()` on the returned handle will
leave an "ended" session with `status = Active` in memory. Fix: rename to
`take_session` and document the must-call-end obligation, OR persist status with the
"ended" stamp on the orchestrator side without touching the session mutex.
```

```
ISSUE|src/group_chat/orchestrator.rs:74-95|low|`persona_registry.resolve` iterates
`sources` and short-circuits on the first error. Inline personas are validated AFTER
all preset resolutions succeed. If source[0] is `Inline` and source[1] is a missing
preset, the inline persona is never validated. Low severity because validation only
adds constraints, but a malformed inline persona slips through if it's followed by a
broken preset reference. Fix: validate all personas BEFORE collecting the resolved
list.
```

```
ISSUE|src/group_chat/session.rs:113-118|low|`build_history_text` formats
`[Speaker]: content\n\n`. If a speaker name contains `]`, e.g. `Persona { name:
"Alice]" }`, the format produces `[Alice]]: …`. Subsequent LLM-side parsing of the
history (the coordinator prompt is fed history verbatim) cannot distinguish this from
a speaker named `Alice` whose content is `]: …`. Fix: replace `]` with `】` (or escape)
in speaker names before formatting.
```

### Perspective 3 — Architecture Compliance

| Redline | Status | Notes |
|---------|--------|-------|
| **R1** (no platform APIs in core) | ✅ clean | no AppKit/Vision/CoreGraphics/objc2 calls anywhere in this module |
| **R2** (complex UI in Leptos only) | ✅ N/A | no UI code in this module |
| **R3** (no heavy deps for non-core) | ⚠️ | `orchestrator.rs:99` uses `uuid::Uuid::new_v4()` for session IDs. R3 was historically violated by `shared/protocol` with the same dependency and the fix was to switch to `AtomicU64`. `src/group_chat` brings `uuid` (with `rand` + `getrandom` transitively) back into `alephcore` — review whether `AtomicU64` would suffice, since session IDs only need to be unique within a single daemon lifetime (already guaranteed by the `Arc<Mutex<HashMap>>` registry) |
| **R4** (interface = pure I/O) | ✅ clean | `group_chat_handler.rs` (gateway/inbound_router) is a thin I/O dispatcher; no business logic in the handler |
| **R7** (one core, many shells) | ✅ clean | orchestrator + executor + coordinator are core-side; channel is the trait seam |
| **R8** (regex only for machine formats) | ✅ clean | no `regex::` calls anywhere in this module |
| **R9** (config as tools) | ✅ N/A | no configurable switches exposed |
| **R10** (intelligence in prompts) | ✅ clean | all LLM behavior is in `build_coordinator_prompt` / `build_persona_prompt`; no business logic in code |

### Perspective 4 — Quality

```
ISSUE|src/group_chat/executor.rs:99-138|medium|`persist_turn` uses
`tokio::task::spawn_blocking` to call SQLite. The `speaker` value is `clone()`d into
the closure; the `db` Arc is also cloned. This is correct, but every turn triggers a
spawn_blocking round-trip, including the user message. For a 3-persona round that
means 4 spawn_blocking tasks. For chatty groups the SQLite write contention can
backpressure the executor. Low-medium risk; acceptable for current load but worth
batching if perf becomes a concern.
```

```
ISSUE|src/group_chat/executor.rs:153-356|low|`execute_round` is a 200-line function
with nested state machines (record user → coordinator → optional coordinator message
→ persona loop → persistence). Splitting into `record_user_turn`,
`invoke_coordinator`, `invoke_respondents`, and `finalize_round` would make the
critical-path rollback fix (see Perspective 1 critical issue) more localized.
```

```
ISSUE|src/group_chat/coordinator.rs:155-164|low|`truncate_str` delegates to
`crate::utils::text_format::truncate_chars`. The 120-char coordinator-prompt
truncation is silent — the LLM sees a truncated prompt with no indication. Adding
`…` (or a structured "TRUNCATED" marker) would let the LLM notice when the prompt
is incomplete.
```

```
ISSUE|src/group_chat/protocol.rs:74-93|low|`Persona::validate` enforces
`MAX_SYSTEM_PROMPT_LEN = 2000`, but neither `parse_inline_role` nor
`Persona::from_configs` call `validate`. The validation is centralized at
`orchestrator::create_session`, which is correct for the session-creation path, but
any future caller that builds a `Persona` and passes it through a different surface
must remember to call `validate()`. Mark `validate` as part of the public API or add
a constructor that validates.
```

## Fix Strategy

Critical + surgical high fixes land as separate commits on `fix/review-group-chat`.
No `cargo check` mid-flight. Single `cargo check -p alephcore` after all fixes verified clean.
Final verification: `cargo check -p alephcore --message-format=short` → `EXIT=0`.

### Fixes Planned (commit-per-module)

1. **executor: rollback session state on mid-round failure** (critical)
2. **group_chat: persist `owner_user_id` to DB via schema migration + insert update** (high)
3. **executor: dedupe provider-fallback warn** (medium)
4. **executor: invalid persona thinking_level → None + warn** (medium)
5. **coordinator: escape `"` in persona name/id before formatting the prompt** (high)
6. **session: monotonic round enforcement in add_turn** (medium)
7. **orchestrator: validate all inline personas before collection** (low)
8. **channel: document escape behavior in tokenize** (low)
9. **executor: extract `record_user_turn` / `invoke_coordinator` / `invoke_respondents` helpers** (low, refactor)

### Not Fixed (deliberately)

- `coordinator_visible` raw-JSON UX: documented behavior, not a defect; channel layer can pre-format if it wants.
- `end_session` try_lock contract: caller already handles it; renaming would break the API.
- `R3 uuid dependency`: out of scope for a static-review pass; tracked separately for the protocol-style UUID discussion.

## Categories Summary

- **Critical**: 2 (executor partial-failure rollback, session owner_user_id persistence gap)
- **Race / lock**: 0 (no new findings; orchestrator lock discipline is correct)
- **Logic / state corruption**: 5 (mid-round rollback, monotonic round, persona dedup, validation order)
- **Log hygiene**: 1 (provider fallback warn dedupe)
- **Architecture (R3)**: 1 (uuid dependency, tracked)
- **Prompt-injection-shaped bugs**: 1 (persona name `"` escape)
- **Quality / refactor**: 4
