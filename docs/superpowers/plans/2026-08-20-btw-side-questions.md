# `/btw` Side Questions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `/btw <question>` answer a side question — read-only, concurrent with the main run, never entering the main context window — on the channel, TUI and CLI faces.

**Architecture:** Aleph already has `/btw`, wired only into the channel inbound router, building an *empty* ephemeral session with *full* tools. This plan moves the derivation to `ExecutionEngine::stamp_slash_mode` (the one chokepoint all three faces already pass through, and the only one that runs *before* the busy-wait lane), and connects it to three primitives the repo already owns: `SpawnContext::Fork` for context, an `ExecTier::Plan` ceiling for read-only, and a deterministic `SessionKey::Ephemeral` derivation for the side thread. The old router special-case is deleted.

**Tech Stack:** Rust (tokio + serde), ratatui (`aleph-tui`), existing `alephcore` gateway / tools / agents modules. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-20-btw-side-questions-design.md`

## Global Constraints

- **Worktree:** all work happens in `/Volumes/TBU/Workspace/Aleph/.claude/worktrees/btw-side-questions` on branch `worktree-btw-side-questions`. Never touch `main`.
- **MSRV 1.95**, toolchain pinned by `rust-toolchain.toml` (1.96.0). Do not run `rustup default` or `cargo +<ver>`.
- **Redlines:** R10 — nothing in this plan adds a line to `src/harness/`. R7 — no regex/keyword intent classification anywhere. R3 — no new third-party crates.
- **Commit style:** `<scope>: <description>`, English, e.g. `gateway: derive btw turns at the shared slash chokepoint`.
- **Fork depth default:** `turns: Some(10)`, configurable.
- **Minimum verification set (five commands, run before every commit that touches Rust):**
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo clippy --all-targets
  cargo test -p aleph-tui -p aleph-cli
  ```
  `cargo check` alone is **not** verification — it does not compile `#[cfg(test)]`.
- **Source-level guards:** never anchor a split delimiter to line start/end. Use `src.replace('\r', "")` before `split("#[cfg(test)]")`. A CRLF checkout makes `"\n#[cfg(test)]\n"` match nothing, and the guard then silently scans its own test module.
- **Every guard must be falsified once by hand** before its task is complete, and the failing line number recorded in the commit body.

---

## File Structure

| File | Responsibility | New? |
|---|---|---|
| `src/gateway/btw/mod.rs` | `BtwTurn` — the single-source predicate + the side-key derivation. Pure, no I/O. | Create |
| `src/gateway/btw/seed.rs` | Incremental fork re-seed with the cursor. | Create |
| `src/gateway/btw/tests.rs` | Unit tests + the source-level guards for this module. | Create |
| `src/gateway/execution_engine/slash_command.rs` | Wire `BtwTurn::resolve` into `stamp_slash_mode`. | Modify |
| `src/gateway/execution_engine/turn_permissions.rs` | Mint the `side_question` flag beside `plan_gate`. | Modify |
| `src/tools/turn_context.rs` | Carry `side_question: bool`. | Modify |
| `src/tools/scoped/builder.rs` | `permission_for` composes the side-question floor. | Modify |
| `src/tools/scoped/gate_chain.rs` | `GateRule::SideQuestion`, ordered ahead of `PlanMode`. | Modify |
| `src/routing/session_key.rs` | Fix the lying doc comment. | Modify |
| `src/gateway/continuation_lifecycle.rs` | Retire the side session on epoch bump. | Modify |
| `src/gateway/inbound_router/command_handler.rs` | **Delete** `SpecialSlash::Btw`, its arm, `handle_btw`, 4 tests. | Modify |
| `src/gateway/inbound_router/mod.rs` | **Delete** the dispatch site. | Modify |
| `src/thinker/nudges.rs` | Promote carrier, taught to `is_synthetic_reminder`. | Modify |
| `interfaces/tui/src/tui/btw_overlay.rs` | TUI overlay controller + render. | Create |
| `docs/reference/FEATURE_LOCATOR.md`, `GATEWAY.md`, `SECURITY.md` | Documentation. | Modify |

---

## Task 0: Probe — does the gateway scope TUI transcript frames by session?

The spec's §6.1 records that `interfaces/tui/src/tui/app/mod.rs:662` says the TUI *subscribes to no topics*, and that `app/events.rs` uses `session_key` only for the clarification dialog (lines 260–295). If transcript frames are not scoped server-side, every background / cron / delegated run is **already** polluting the TUI transcript today — a pre-existing P1 independent of `/btw`.

**Files:**
- Create: `docs/superpowers/plans/2026-08-20-btw-probe-result.md`

**Interfaces:**
- Produces: a recorded boolean `tui_frames_are_session_scoped` that Task 1 reads.

- [ ] **Step 1: Read the server-side visibility predicate**

Run:
```bash
grep -rn "fn should_receive" -A 40 src/gateway/ | head -60
```
Read whether the predicate filters by session key for transcript topics (`stream.response_chunk`, `agent_trace` text) or only by owner/role.

- [ ] **Step 2: Confirm on a real machine**

Start a server with an isolated home, attach the TUI, and trigger a run on a *different* session key while the TUI sits on its own:

```bash
export ALEPH_HOME=/private/tmp/claude-501/-Volumes-TBU-Workspace-Aleph/btw-probe
mkdir -p "$ALEPH_HOME"
cargo run --bin aleph-server &
# in a second terminal, attached to the TUI, ask the agent to spawn a background subagent:
#   "spawn a background subagent that counts to five"
# then watch whether the child's assistant text appears in the TUI transcript
```

Expected outcome is one of:
- **scoped** — the child's text never appears in the TUI transcript
- **not scoped** — the child's text interleaves into the TUI transcript

- [ ] **Step 3: Record the result**

Write `docs/superpowers/plans/2026-08-20-btw-probe-result.md` containing exactly:

```markdown
# btw probe result — TUI transcript frame scoping

Date: <YYYY-MM-DD>
Method: background subagent spawned from a TUI-attached session; observed whether
the child's assistant text reached the TUI transcript.

**tui_frames_are_session_scoped: <true|false>**

Server-side predicate read at: <file:line>
Observed: <one sentence of what actually appeared on screen>
```

Do not write a conclusion you did not observe. If the probe cannot be run, record `unknown` and treat Task 1 as required (fail-closed).

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/plans/2026-08-20-btw-probe-result.md
git commit -m "docs: record btw probe result for TUI frame scoping"
```

---

## Task 1: TUI transcript frames route by session

**Skip this task only if Task 0 recorded `tui_frames_are_session_scoped: true`.** If skipped, add a line to the probe-result file saying so and move to Task 2. If the probe recorded `unknown`, do this task.

This is an independent pre-existing defect. Commit it separately so it can be merged ahead of `/btw` if desired.

**Files:**
- Modify: `interfaces/tui/src/tui/app/events.rs`
- Test: `interfaces/tui/src/tui/app/tests.rs`

**Interfaces:**
- Produces: `AppState::frame_belongs_here(&self, session_key: Option<&str>) -> bool`

- [ ] **Step 1: Write the failing test**

Add to `interfaces/tui/src/tui/app/tests.rs`:

```rust
#[test]
fn a_frame_from_another_session_is_not_appended_to_this_transcript() {
    let mut app = AppState::for_test();
    app.session_key = "agent:main:main".to_string();

    // A frame that names a different session must be dropped, not appended.
    assert!(!app.frame_belongs_here(Some("agent:main:ephemeral:btw-abc")));
    // Our own session is kept.
    assert!(app.frame_belongs_here(Some("agent:main:main")));
    // A frame that names no session at all is kept: "I cannot tell" must not
    // become "drop", or the first turn of a new session and older cores both
    // go silent.
    assert!(app.frame_belongs_here(None));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p aleph-tui frame_belongs_here -- --nocapture`
Expected: FAIL — `no method named 'frame_belongs_here'`.

- [ ] **Step 3: Implement**

In `interfaces/tui/src/tui/app/events.rs`:

```rust
impl AppState {
    /// Whether a frame carrying `session_key` belongs on THIS screen.
    ///
    /// Only a frame that can be *proved* to belong elsewhere is dropped. A
    /// frame with no session key is kept: "I cannot tell" is not "it is not
    /// mine", and reading it that way silences a new session's first turn and
    /// every frame from a core too old to stamp one. Same shape as the Panel's
    /// `resolve_target` after its 2026-08 fix.
    pub fn frame_belongs_here(&self, session_key: Option<&str>) -> bool {
        match session_key {
            Some(k) if !k.is_empty() => k == self.session_key,
            _ => true,
        }
    }
}
```

Then guard every transcript-appending arm in `events.rs` with it. The arms are the ones that touch `turn_streamed_len` or push assistant text — locate them with:

```bash
grep -n "turn_streamed_len\|push_assistant\|append" interfaces/tui/src/tui/app/events.rs
```

For each, add an early `if !self.frame_belongs_here(session_key.as_deref()) { return; }` at the top of the arm.

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p aleph-tui frame_belongs_here -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the full TUI suite**

Run: `cargo test -p aleph-tui -p aleph-cli`
Expected: all pass. If pre-existing failures appear, record their names in the commit body — do not fix them here.

- [ ] **Step 6: Commit**

```bash
git add interfaces/tui/src/tui/app/events.rs interfaces/tui/src/tui/app/tests.rs
git commit -m "tui: drop transcript frames that name another session"
```

---

## Task 2: `BtwTurn` — the single-source predicate and side-key derivation

**Files:**
- Create: `src/gateway/btw/mod.rs`, `src/gateway/btw/tests.rs`
- Modify: `src/gateway/mod.rs` (add `pub mod btw;`)

**Interfaces:**
- Produces:
  - `pub struct BtwTurn { pub question: String, pub promote: bool }`
  - `pub fn BtwTurn::resolve(input: &str) -> Option<BtwTurn>`
  - `pub fn side_key_for(main: &SessionKey) -> SessionKey`
  - `pub const BTW_METADATA_KEY: &str = "btw"`

- [ ] **Step 1: Write the failing tests**

Create `src/gateway/btw/tests.rs`:

```rust
use super::*;
use crate::routing::session_key::SessionKey;

#[test]
fn resolve_accepts_the_documented_spellings() {
    assert_eq!(
        BtwTurn::resolve("/btw what was that config file called?"),
        Some(BtwTurn { question: "what was that config file called?".into(), promote: false })
    );
    // Case-insensitive command, body case preserved verbatim for the model.
    assert_eq!(
        BtwTurn::resolve("/BTW Explain Async/Await").map(|b| b.question),
        Some("Explain Async/Await".into())
    );
    // Telegram's @botname suffix is tolerated.
    assert_eq!(
        BtwTurn::resolve("/btw@MyBot why?").map(|b| b.question),
        Some("why?".into())
    );
    // Newline separator.
    assert_eq!(
        BtwTurn::resolve("/btw\nnext line").map(|b| b.question),
        Some("next line".into())
    );
}

#[test]
fn resolve_rejects_non_btw_and_empty_bodies() {
    assert_eq!(BtwTurn::resolve("hello"), None);
    assert_eq!(BtwTurn::resolve("/help"), None);
    assert_eq!(BtwTurn::resolve("/btwlike this"), None);
    // An empty side question has nowhere to go.
    assert_eq!(BtwTurn::resolve("/btw"), None);
    assert_eq!(BtwTurn::resolve("/btw    "), None);
}

#[test]
fn resolve_recognises_the_promote_verb() {
    let b = BtwTurn::resolve("/btw promote").expect("promote parses");
    assert!(b.promote);
    assert!(b.question.is_empty());
    // "promote" as the first word of a real question is still promote —
    // documented and deliberate; ask "/btw please promote ..." to disambiguate.
    assert!(BtwTurn::resolve("/btw what does promote mean?").expect("q").promote == false);
}

#[test]
fn the_side_key_is_derived_from_the_main_key_including_its_epoch() {
    let main = SessionKey::main("assistant");
    let bumped = main.with_epoch(1);

    let a = side_key_for(&main);
    let b = side_key_for(&main);
    let c = side_key_for(&bumped);

    // Deterministic: same main key, same side key. This is what gives the
    // side thread its memory.
    assert_eq!(a.to_key_string(), b.to_key_string());
    // Epoch-inclusive: /new bumps the epoch, so the side thread starts empty
    // by construction rather than by anyone remembering to clear it.
    assert_ne!(a.to_key_string(), c.to_key_string());
    assert!(matches!(a, SessionKey::Ephemeral { .. }));
    // Agent identity is preserved so partition/visibility predicates still work.
    assert_eq!(a.agent_id(), main.agent_id());
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib gateway::btw -- --nocapture`
Expected: FAIL — `unresolved module or unlinked crate 'btw'`.

- [ ] **Step 3: Implement**

Create `src/gateway/btw/mod.rs`:

```rust
//! `/btw` side questions — the one derivation every surface shares.
//!
//! A side question runs as its own turn on a *derived* ephemeral session:
//! read-only, in its own busy-queue lane (so it answers while the main run
//! keeps going), and never appended to the main conversation.
//!
//! # Why this is not a sixth session knob
//!
//! The five knobs in `CLAUDE.md`'s table all share one mechanism: precedence
//! request > session > global, and a request-carried value is **written back
//! onto the session** so the choice outlives its turn. `btw` is the opposite:
//! it must affect exactly one call. Filing it with the knobs would make a
//! single side question permanently drop the main conversation to `Plan`.
//! It therefore does NOT appear in `turn_*.rs`, in `sessions.patch`'s
//! `knob_validators()`, or in `session_snapshot.rs` — see the guard in
//! `tests.rs`.

use crate::routing::session_key::SessionKey;

/// Metadata key stamped on a run request that is a side question.
pub const BTW_METADATA_KEY: &str = "btw";

/// A resolved `/btw` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwTurn {
    /// The question, with its original case preserved for the model to read
    /// verbatim. Empty when `promote` is set.
    pub question: String,
    /// `/btw promote` — move the latest side answer into the main
    /// conversation. Explicit by construction: nothing crosses that boundary
    /// without the user asking out loud.
    pub promote: bool,
}

impl BtwTurn {
    /// Resolve a raw input into a side question.
    ///
    /// **Single source.** Every surface calls this one function; none of them
    /// re-derives "is this a btw" from its own string handling. The predicate
    /// that this replaced lived in `inbound_router`, a channel-only module, so
    /// the TUI and Panel could not reach it at all.
    #[must_use]
    pub fn resolve(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        let (head, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((h, r)) => (h, r),
            None => (trimmed, ""),
        };
        // Strip Telegram's `@botname` suffix before comparing.
        let cmd = head.split_once('@').map_or(head, |(c, _)| c);
        if !cmd.strip_prefix('/')?.eq_ignore_ascii_case("btw") {
            return None;
        }
        let body = rest.trim();
        if body.eq_ignore_ascii_case("promote") {
            return Some(Self { question: String::new(), promote: true });
        }
        if body.is_empty() {
            // An empty side question has nowhere to go.
            return None;
        }
        Some(Self { question: body.to_string(), promote: false })
    }
}

/// The side session key for `main`.
///
/// **Single source — write and read must be this same function.** Two call
/// sites each hashing the key "the same way" are byte-identical at epoch 0 and
/// diverge only on a machine that has run `/new`, which is exactly the shape
/// that never reproduces locally.
///
/// The derivation includes the epoch (via `to_key_string`, see
/// `SessionKey::append_epoch`). That buys two things:
///
/// 1. `/new` bumps the epoch, so the derived key changes and the side thread
///    starts empty **by construction** — not because anyone remembered to
///    clear it.
/// 2. The previous side session becomes unaddressable, so the retirement hook
///    only has to *delete* it, never also to hide it. A missed retirement
///    leaves disk residue, never a crossed side thread.
#[must_use]
pub fn side_key_for(main: &SessionKey) -> SessionKey {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    main.to_key_string().hash(&mut hasher);
    SessionKey::Ephemeral {
        agent_id: main.agent_id().to_string(),
        ephemeral_id: format!("btw-{:016x}", hasher.finish()),
    }
}

#[cfg(test)]
mod tests;
```

Add to `src/gateway/mod.rs`, in alphabetical position among the existing `pub mod` lines:

```rust
pub mod btw;
```

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p alephcore --lib gateway::btw -- --nocapture`
Expected: 4 tests PASS.

- [ ] **Step 5: Run the minimum verification set**

Run the five commands from Global Constraints.
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/btw/ src/gateway/mod.rs
git commit -m "gateway: add BtwTurn, the one derivation every surface shares"
```

---

## Task 3: Stamp btw at the shared chokepoint

`stamp_slash_mode` is the only seam all three faces pass through *before* the busy lane. Stamping anywhere later means `steering::carries_more_than_text` cannot see it, and every `/btw` sent while a run is in flight is folded into that run as plain steering text — silently entering the main context window, which is precisely what the feature exists to prevent.

**Files:**
- Modify: `src/gateway/execution_engine/slash_command.rs:62-72`
- Test: `src/gateway/execution_engine/slash_command.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `BtwTurn::resolve`, `BTW_METADATA_KEY` (Task 2)
- Produces: a run request whose `metadata` contains `BTW_METADATA_KEY` for a btw turn.

- [ ] **Step 1: Write the failing test**

Append to the test module in `src/gateway/execution_engine/slash_command.rs`:

```rust
#[test]
fn btw_is_stamped_and_therefore_never_folded_into_a_running_sibling() {
    use crate::gateway::btw::BTW_METADATA_KEY;
    let mut metadata = std::collections::HashMap::new();

    // The pure half of stamp_slash_mode: btw resolution needs no parser and
    // must therefore work even when the command-parser cell is empty (tests,
    // simulated mode) — the exact condition under which try_resolve_slash_command
    // returns None.
    super::stamp_btw("/btw what was that file called?", &mut metadata);
    assert_eq!(
        metadata.get(BTW_METADATA_KEY).map(String::as_str),
        Some("what was that file called?")
    );

    let mut plain = std::collections::HashMap::new();
    super::stamp_btw("just a message", &mut plain);
    assert!(plain.is_empty());
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib btw_is_stamped -- --nocapture`
Expected: FAIL — `cannot find function 'stamp_btw'`.

- [ ] **Step 3: Implement**

In `src/gateway/execution_engine/slash_command.rs`, add a free function next to `is_continuation_driven_slash` (module scope, so the test reaches it without an engine):

```rust
/// Stamp `BTW_METADATA_KEY` if `input` is a side question.
///
/// Free-standing and parser-free on purpose. `try_resolve_slash_command`
/// returns `None` whenever the shared `CommandParser` cell is empty, and a
/// side question that silently degraded to a normal turn under that condition
/// would run at the session's real tier with the main session's key — the
/// two failures this feature exists to prevent, in the one configuration
/// (tests, simulated mode) where nobody would notice.
pub(crate) fn stamp_btw(input: &str, metadata: &mut HashMap<String, String>) {
    use crate::gateway::btw::{BtwTurn, BTW_METADATA_KEY};
    if metadata.contains_key(BTW_METADATA_KEY) {
        return;
    }
    if let Some(turn) = BtwTurn::resolve(input) {
        metadata.insert(
            BTW_METADATA_KEY.to_string(),
            if turn.promote { "promote".to_string() } else { turn.question },
        );
    }
}
```

Then call it from `stamp_slash_mode`, **before** the existing early return:

```rust
    pub async fn stamp_slash_mode(&self, input: &str, metadata: &mut HashMap<String, String>) {
        // Side questions first: they are resolved without the command parser,
        // and they must be stamped even when the parser cell is empty.
        stamp_btw(input, metadata);
        if metadata.contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY) {
            return;
        }
        // ... unchanged ...
    }
```

Add `BTW_METADATA_KEY` to `steering::carries_more_than_text` in `src/gateway/execution_engine/steering.rs:207`:

```rust
fn carries_more_than_text(request: &RunRequest) -> bool {
    !request.attachments.is_empty()
        || request
            .metadata
            .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY)
        // A side question is a turn of its own on its own session. Folding it
        // into a running sibling would put it in the main context window.
        || request
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY)
        || request.sandbox_override.is_some()
        || request.max_iterations_override.is_some()
        || request.timeout_secs.is_some()
}
```

- [ ] **Step 4: Write the second failing test — the folding guard**

```rust
#[test]
fn a_btw_request_is_never_folded_into_a_running_sibling() {
    let mut request = RunRequest::for_test("/btw why?");
    request.metadata.insert(
        crate::gateway::btw::BTW_METADATA_KEY.to_string(),
        "why?".to_string(),
    );
    assert!(
        super::carries_more_than_text(&request),
        "a btw turn folded as steering text lands in the main context window"
    );
}
```

Place it in `src/gateway/execution_engine/steering.rs`'s test module. If `RunRequest::for_test` does not exist, construct the request the way neighbouring tests in that file do — copy their construction verbatim rather than inventing a helper.

- [ ] **Step 5: Run both and watch them pass**

Run: `cargo test -p alephcore --lib btw_is_stamped folded_into_a_running_sibling -- --nocapture`
Expected: 2 PASS.

- [ ] **Step 6: Run the minimum verification set, then commit**

```bash
git add src/gateway/execution_engine/slash_command.rs src/gateway/execution_engine/steering.rs
git commit -m "gateway: stamp btw turns at the shared pre-lane chokepoint"
```

---

## Task 4: The read-only ceiling and the `SideQuestion` gate rule

`ExecTier::Plan` is the read-only rung, and it composes through `ExecTier::most_restrictive` (`exec_tier.rs:248`), so a btw turn in a `Full` session still lands on `Plan` and btw itself can never widen anything.

But `Plan` carves out `PLAN_REACHABLE_TOOLS = ["scratchpad", "subagent"]` (`exec_tier.rs:131`), and both are wrong for btw: `scratchpad` writes the **main** session's execution list, and a `subagent` spawned from a side question outlives the side session with no surface able to enumerate it.

**The revocation cannot go through the `allowed` set.** `ScopedToolService::is_allowed` (`src/tools/scoped/builder.rs:411`) returns `true` unconditionally for the attached subagent tool, bypassing both the listing `retain` and the `execute()` check. It has to go through the permission chain.

**Files:**
- Modify: `src/tools/turn_context.rs`, `src/gateway/execution_engine/turn_permissions.rs:237`, `src/tools/scoped/builder.rs:231`, `src/tools/scoped/gate_chain.rs:69`
- Test: `src/tools/scoped/tests.rs`

**Interfaces:**
- Consumes: `BTW_METADATA_KEY` (Task 2)
- Produces: `TurnContext.side_question: bool`; `GateRule::SideQuestion`

- [ ] **Step 1: Write the failing test**

Add to `src/tools/scoped/tests.rs`:

```rust
/// The two `Plan` carve-outs are revoked for a side question, and the
/// revocation is DERIVED from `PLAN_REACHABLE_TOOLS` rather than restating it.
/// A third member added to that constant is denied for btw automatically —
/// the safe direction — and this test names it so the author must confirm.
#[test]
fn a_side_question_revokes_every_plan_carve_out() {
    use crate::config::types::policies::PLAN_REACHABLE_TOOLS;
    use crate::extension::PermissionAction;

    for tool in PLAN_REACHABLE_TOOLS {
        let svc = scoped_service_for_test()
            .with_exec_tier(ExecTier::Plan)
            .with_side_question(true)
            .build();
        assert_eq!(
            svc.permission_for(tool),
            PermissionAction::Deny,
            "{tool} is reachable under Plan but must not be during a side question"
        );
    }

    // Control: without the side-question flag the carve-outs still hold, so
    // this test cannot pass by breaking Plan mode itself.
    for tool in PLAN_REACHABLE_TOOLS {
        let svc = scoped_service_for_test()
            .with_exec_tier(ExecTier::Plan)
            .with_side_question(false)
            .build();
        assert_ne!(svc.permission_for(tool), PermissionAction::Deny, "{tool}");
    }
}

/// A mutating tool is refused during a side question, and the reason names the
/// side question rather than the plan handoff — pointing the reader at "get
/// your plan approved" would name a repair that cannot work here.
#[test]
fn a_side_question_refusal_names_itself_not_the_plan_handoff() {
    let svc = scoped_service_for_test()
        .with_exec_tier(ExecTier::Plan)
        .with_side_question(true)
        .build();
    let rule = svc.deny_rule_for("file_write").expect("file_write is refused");
    assert!(
        matches!(rule, GateRule::SideQuestion),
        "expected SideQuestion, got {rule:?}"
    );
}
```

If `scoped_service_for_test()` has a different name in that file, use the existing builder — locate it with `grep -n "fn scoped_service\|fn test_service" src/tools/scoped/tests.rs`.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib side_question -- --nocapture`
Expected: FAIL — `no method named 'with_side_question'` / `no variant 'SideQuestion'`.

- [ ] **Step 3: Make `PLAN_REACHABLE_TOOLS` visible**

In `src/config/types/policies/exec_tier.rs:131`, change the constant from private to crate-visible so the revocation derives from it instead of restating it:

```rust
/// ... existing doc unchanged ...
///
/// Visible to the crate because the side-question floor is DERIVED from this
/// set (`ScopedToolService::permission_for`): a side question revokes exactly
/// these carve-outs. Keeping a second list would be the very drift this
/// constant exists to prevent.
pub(crate) const PLAN_REACHABLE_TOOLS: &[&str] = &["scratchpad", "subagent"];
```

Re-export it from `src/config/types/policies/mod.rs` alongside the other public items in that module.

- [ ] **Step 4: Carry the flag**

In `src/tools/turn_context.rs`, add to `TurnContext`:

```rust
    /// This turn is a `/btw` side question.
    ///
    /// Minted beside `plan_gate` in `resolve_turn_permissions` — the one place
    /// that already resolves per-turn permission facts. Deliberately NOT
    /// derived from the session key's shape: matching an `ephemeral_id`
    /// prefix would be a second derivation of a fact the request already
    /// carries, and a string match is exactly the kind of predicate that
    /// keeps working right up until someone renames the prefix.
    pub side_question: bool,
```

Fix every `TurnContext` construction site the compiler now rejects; for non-gateway constructions (cron, internal, tests) the value is `false`.

In `src/gateway/execution_engine/turn_permissions.rs`, beside the `plan_gate` mint at line 237:

```rust
        let side_question = request
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY);
```

and add `side_question` to the struct literal at line ~257, plus to the `info!` at line ~251 so it is observable in logs.

- [ ] **Step 5: Compose the floor**

In `src/tools/scoped/builder.rs`, change `permission_for` (line 231):

```rust
    pub(super) fn permission_for(&self, name: &str) -> crate::extension::PermissionAction {
        use crate::extension::PermissionAction;
        // The side-question floor. Above the tier verdict, because it is not
        // removable by any configuration and not resolvable by approving a
        // plan — the two repairs the rules beneath it point at.
        if self.is_side_question()
            && crate::config::types::policies::PLAN_REACHABLE_TOOLS.contains(&name)
        {
            return PermissionAction::Deny;
        }
        crate::config::types::policies::effective_permission(
            self.tool_permissions.as_ref(),
            self.effective_exec_tier(),
            self.tool_facts(name),
        )
    }

    /// Whether this turn is a side question (from `TurnContext`, the same
    /// carrier `plan_gate` rides).
    pub(super) fn is_side_question(&self) -> bool {
        self.turn_context.as_ref().is_some_and(|t| t.side_question)
    }
```

Match the exact field access pattern used by the neighbouring `plan_gate` reads at lines 256 and 301.

- [ ] **Step 6: Add the gate rule**

In `src/tools/scoped/gate_chain.rs`, add to `GateRule` **immediately after `PolicyDeny`** — ahead of `ToolDeclared`, `GateRemoval` and `PlanMode`:

```rust
    /// This turn is a `/btw` side question, which is read-only by
    /// construction.
    ///
    /// Reported ahead of [`Self::PlanMode`] for the ordering rule this chain
    /// already follows: a reason must never mislead the reader about **what
    /// they could change to get a different result**. `PlanMode` points at the
    /// plan handoff — approve the plan and the tool runs. For a side question
    /// there is no handoff and no setting; the repair is "ask the main agent
    /// instead". Naming the removable rule would send the reader to a fix that
    /// cannot work.
    SideQuestion,
```

Wire it into the rule-resolution function in the same file (the one that returns the first matching rule) as the first check after `PolicyDeny`, conditioned on `is_side_question()`. Update its `id()` and `reason()` arms:

- `id()` → `"side_question"`
- `reason()` → `"This is a read-only /btw side question — it can read and search, but not change anything. Ask the main agent to do it instead."`

The chain has a guard asserting "the set of classified rules == the set of gated calls". Run it and fix what it names.

- [ ] **Step 7: Run and watch it pass**

Run: `cargo test -p alephcore --lib side_question gate_chain -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Falsify the guard by hand**

Temporarily change the floor in `permission_for` to `if false && self.is_side_question()`. Run:

Run: `cargo test -p alephcore --lib a_side_question_revokes_every_plan_carve_out`
Expected: **FAIL**, naming `src/tools/scoped/tests.rs:<line>` and the tool name. Record that line number. Then revert the change and confirm green.

- [ ] **Step 9: Run the minimum verification set, then commit**

```bash
git add src/tools/turn_context.rs src/gateway/execution_engine/turn_permissions.rs \
        src/tools/scoped/builder.rs src/tools/scoped/gate_chain.rs \
        src/tools/scoped/tests.rs src/config/types/policies/
git commit -m "tools: revoke the Plan carve-outs for /btw side questions

Falsified: flipping the floor to a constant false fails
src/tools/scoped/tests.rs:<line>."
```

---

## Task 5: Incremental fork seeding

The side session inherits the main transcript through `SpawnContext::Fork`. Seeding the whole transcript on **every** question would append the same prefix repeatedly — the "two projections feeding one append-only state" shape — and would re-key the provider prefix cache each time. pi-btw avoids the doubling by seeding only once, at the cost of a side agent whose view of the main session is frozen at the first question. Aleph carries a cursor and appends only what closed since.

**Files:**
- Create: `src/gateway/btw/seed.rs`
- Modify: `src/gateway/btw/mod.rs` (add `mod seed; pub use seed::*;`)
- Test: `src/gateway/btw/tests.rs`

**Interfaces:**
- Consumes: `fork::snapshot`, `fork::seed` (`src/agents/subagent_spawner/fork.rs:333`, `:357`), `side_key_for` (Task 2)
- Produces: `pub async fn ensure_seeded(session: &dyn SessionService, main: &SessionId, side: &SessionId, turns: usize) -> Result<SeedOutcome, String>`; `pub struct SeedOutcome { pub events_added: usize, pub cursor: Option<String> }`

- [ ] **Step 1: Write the failing test**

Add to `src/gateway/btw/tests.rs`:

```rust
#[tokio::test]
async fn seeding_twice_does_not_duplicate_the_main_prefix() {
    let session = in_memory_session_service();
    let main = SessionId::from("agent:main:main");
    let side = SessionId::from("agent:main:ephemeral:btw-test");

    append_closed_turn(&session, &main, "first user turn", "first answer").await;

    let a = ensure_seeded(&session, &main, &side, 10).await.expect("first seed");
    assert!(a.events_added > 0, "the first seed must carry the transcript");

    // Nothing new closed on the main session in between.
    let b = ensure_seeded(&session, &main, &side, 10).await.expect("second seed");
    assert_eq!(b.events_added, 0, "a second seed with no new turns must be a no-op");

    let text = transcript_text(&session, &side).await;
    assert_eq!(
        text.matches("first user turn").count(),
        1,
        "the main prefix appears twice — the side transcript is doubling"
    );
}

#[tokio::test]
async fn seeding_carries_only_what_closed_since_the_cursor() {
    let session = in_memory_session_service();
    let main = SessionId::from("agent:main:main");
    let side = SessionId::from("agent:main:ephemeral:btw-test2");

    append_closed_turn(&session, &main, "turn one", "answer one").await;
    ensure_seeded(&session, &main, &side, 10).await.expect("first seed");

    append_closed_turn(&session, &main, "turn two", "answer two").await;
    let b = ensure_seeded(&session, &main, &side, 10).await.expect("delta seed");

    assert!(b.events_added > 0, "the new turn must be carried");
    let text = transcript_text(&session, &side).await;
    assert_eq!(text.matches("turn one").count(), 1);
    assert_eq!(text.matches("turn two").count(), 1);
}
```

Write the three helpers (`in_memory_session_service`, `append_closed_turn`, `transcript_text`) in the same test file by copying the construction pattern from `src/agents/subagent_spawner/fork.rs`'s own tests — locate them with `grep -n "mod tests" -A 40 src/agents/subagent_spawner/fork.rs`.

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib gateway::btw::tests::seeding -- --nocapture`
Expected: FAIL — `cannot find function 'ensure_seeded'`.

- [ ] **Step 3: Implement**

Create `src/gateway/btw/seed.rs`:

```rust
//! Incremental fork seeding for the side session.
//!
//! # Why incremental
//!
//! The side session key is deterministic, so the side session persists across
//! questions — that is what gives the side thread its memory. Re-seeding the
//! whole main transcript on each question would therefore append the same
//! prefix again and again (`seed₁ + Q1A1 + seed₁seed₂ + …`), and each re-seed
//! would re-key the provider prefix cache for a conversation that is meant to
//! be cheap.
//!
//! Seeding only what closed since a cursor keeps the side transcript
//! append-only, which is exactly what prefix caching rewards.
//!
//! # The cursor has one writer
//!
//! `ensure_seeded` writes it, in the same step that performs the copy. A
//! second writer would be a second answer to "how much have we carried", and
//! the two would disagree on the first interleaved question.

use crate::session::service::{SessionId, SessionService};

/// What one `ensure_seeded` call actually carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedOutcome {
    /// Events copied from the main session this call. `0` means the side
    /// session was already current — the normal case for a follow-up asked
    /// seconds after the previous one.
    pub events_added: usize,
    /// The main-session event id now carried, stored on the side session.
    pub cursor: Option<String>,
}

/// Metadata key on the SIDE session holding the last-carried main event id.
const CURSOR_KEY: &str = "btw_seed_cursor";

/// Bring `side` up to date with `main`, carrying at most `turns` complete
/// turns on a cold seed.
///
/// `turns` bounds the **cold** seed only. Once a cursor exists the delta is
/// whatever closed since, which is naturally small; clamping the delta too
/// would silently drop the middle of a long-running main session and leave the
/// side agent reading a transcript with a hole in it that nothing announces.
pub async fn ensure_seeded(
    session: &dyn SessionService,
    main: &SessionId,
    side: &SessionId,
    turns: usize,
) -> Result<SeedOutcome, String> {
    let cursor = read_cursor(session, side).await;
    let source = crate::agents::subagent_spawner::fork::snapshot(session, main).await?;

    let pending: Vec<_> = match &cursor {
        Some(last) => source
            .iter()
            .skip_while(|e| e.id() != last.as_str())
            .skip(1)
            .cloned()
            .collect(),
        None => source.as_ref().clone(),
    };

    if pending.is_empty() {
        return Ok(SeedOutcome { events_added: 0, cursor });
    }

    let bound = if cursor.is_none() { Some(turns) } else { None };
    let carried =
        crate::agents::subagent_spawner::fork::seed_events(session, side, &pending, bound).await?;

    let new_cursor = carried.last().map(|e| e.id().to_string()).or(cursor);
    if let Some(ref c) = new_cursor {
        write_cursor(session, side, c).await?;
    }
    Ok(SeedOutcome { events_added: carried.len(), cursor: new_cursor })
}

async fn read_cursor(session: &dyn SessionService, side: &SessionId) -> Option<String> {
    session.get_metadata(side, CURSOR_KEY).await.ok().flatten()
}

async fn write_cursor(
    session: &dyn SessionService,
    side: &SessionId,
    value: &str,
) -> Result<(), String> {
    session
        .set_metadata(side, CURSOR_KEY, value)
        .await
        .map_err(|e| format!("btw: write seed cursor: {e}"))
}
```

`fork::seed` today takes a parent id and plans the fork itself. Extract the copy half into a sibling `fork::seed_events(session, child, events, bound)` that takes the events directly, and have the existing `fork::seed` call it. Do not duplicate the copy logic — `SessionForked` provenance marking must stay in one place. If `SessionService` has no `get_metadata` / `set_metadata`, use whatever per-session metadata accessor it does expose (`grep -n "fn .*metadata" src/session/service.rs`) and adapt these two helpers only.

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p alephcore --lib gateway::btw::tests::seeding -- --nocapture`
Expected: 2 PASS.

- [ ] **Step 5: Falsify the doubling guard by hand**

Temporarily change `read_cursor` to `async { None }`. Run:

Run: `cargo test -p alephcore --lib seeding_twice_does_not_duplicate`
Expected: **FAIL** at the `matches(...).count() == 1` assertion. Record the line number, revert, confirm green.

- [ ] **Step 6: Run the minimum verification set, then commit**

```bash
git add src/gateway/btw/ src/agents/subagent_spawner/fork.rs
git commit -m "gateway: seed the btw side session incrementally from a cursor

Falsified: dropping the cursor read fails
src/gateway/btw/tests.rs:<line>."
```

---

## Task 6: Route the btw turn onto the side session

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs` (the seam that resolves a request's session key)

**Interfaces:**
- Consumes: `side_key_for`, `ensure_seeded`, `BTW_METADATA_KEY`
- Produces: a btw run executing on the side key with `ExecTier::Plan` composed in.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_btw_run_executes_on_the_derived_side_key_not_the_main_one() {
    let engine = engine_for_test().await;
    let main = SessionKey::main("assistant");

    let mut request = RunRequest::for_test("/btw why?");
    request.session_key = main.to_key_string();
    engine.stamp_slash_mode("/btw why?", &mut request.metadata).await;

    let resolved = engine.resolve_execution_session(&request);
    assert_eq!(
        resolved.to_key_string(),
        crate::gateway::btw::side_key_for(&main).to_key_string()
    );
    // And the main session is untouched — this is the whole promise.
    assert_ne!(resolved.to_key_string(), main.to_key_string());
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib executes_on_the_derived_side_key -- --nocapture`
Expected: FAIL — `no method named 'resolve_execution_session'`.

- [ ] **Step 3: Implement**

In `src/gateway/execution_engine/execute.rs`, add beside the existing session-key handling:

```rust
    /// The session a request actually executes on.
    ///
    /// Everything runs on its own declared key except a side question, which
    /// is redirected onto the deterministic side key so it gets its own
    /// busy-queue lane (answering while the main run continues) and never
    /// appends to the main transcript.
    pub(crate) fn resolve_execution_session(&self, request: &RunRequest) -> SessionKey {
        let declared = SessionKey::parse(&request.session_key)
            .unwrap_or_else(|| SessionKey::main(&request.agent_id));
        if request.metadata.contains_key(crate::gateway::btw::BTW_METADATA_KEY) {
            return crate::gateway::btw::side_key_for(&declared);
        }
        declared
    }
```

Call it at the point `execute()` currently derives the key, and — for a btw request only — before dispatch:

```rust
        if request.metadata.contains_key(crate::gateway::btw::BTW_METADATA_KEY) {
            crate::gateway::btw::ensure_seeded(
                session_service.as_ref(),
                &SessionId::from(declared.to_key_string()),
                &SessionId::from(side.to_key_string()),
                self.btw_fork_turns(),
            )
            .await?;
        }
```

Add `btw_fork_turns()` reading config with a default of 10:

```rust
    /// How many complete main-session turns a cold side seed carries.
    ///
    /// Bounded on purpose: a side question is meant to be cheap, and an
    /// unbounded fork makes every "quick question" pay a full-price prefix
    /// write against a main session that may be hundreds of thousands of
    /// tokens long.
    fn btw_fork_turns(&self) -> usize {
        self.app_config
            .as_ref()
            .and_then(|c| c.try_read().ok())
            .and_then(|g| g.policies.btw_fork_turns)
            .unwrap_or(10)
    }
```

Add `pub btw_fork_turns: Option<usize>` to the policies config struct with a doc comment stating the default and why it is bounded.

Compose the tier ceiling where `resolve_turn_permissions` computes `tier` (`turn_permissions.rs`, just before the `plan_gate` mint at line 237):

```rust
        // A side question is read-only regardless of the session's tier. This
        // composes through the same rule every other ceiling uses, so it can
        // only ever tighten.
        let tier = if side_question {
            ExecTier::most_restrictive(tier, ExecTier::Plan)
        } else {
            tier
        };
```

Ensure `side_question` is computed above this line, not below it.

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p alephcore --lib executes_on_the_derived_side_key -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Write the in-process end-to-end guard**

This is the one test that matters most. Isolated unit tests on either half pass while the wire between them is cut — the shape that hid the `EXEC_WORKSPACE` defect for a full round. Put both halves in one process.

Create `tests/btw_is_read_only.rs`:

```rust
//! A real btw turn refuses a mutating tool.
//!
//! Deliberately an integration test, not a unit test. The permission floor has
//! a unit test and the routing has a unit test; both stay green when the
//! metadata key never reaches `TurnContext`, because each half is exercised
//! with a hand-built input the other half never produces. Only running one
//! actual turn shows whether the ceiling ARRIVED.

#[tokio::test]
async fn a_side_question_cannot_write_a_file() {
    let harness = alephcore::test_support::gateway_harness().await;
    let out = harness
        .run_turn("/btw create a file called proof.txt with the word hi in it")
        .await;

    assert!(
        out.refusals.iter().any(|r| r.rule_id == "side_question"),
        "expected a side_question refusal, got: {:?}",
        out.refusals
    );
    assert!(
        !harness.workspace().join("proof.txt").exists(),
        "the side question wrote a file — the read-only ceiling did not arrive"
    );
}
```

Adapt `gateway_harness()` / `run_turn` to whatever the `test-helpers` feature already exposes — find it with `grep -rn "pub fn gateway_harness\|mod test_support" src/`. Do not add a new harness if one exists.

- [ ] **Step 6: Falsify it by hand**

Temporarily delete the `most_restrictive` composition added in Step 3. Run:

Run: `cargo test -p alephcore --features test-helpers --test btw_is_read_only`
Expected: **FAIL** on the `proof.txt` assertion — the file exists. Record the line number, revert, confirm green.

- [ ] **Step 7: Run the minimum verification set, then commit**

```bash
git add src/gateway/execution_engine/ src/config/types/policies/ tests/btw_is_read_only.rs
git commit -m "gateway: route btw turns onto the side session under a Plan ceiling

Falsified: removing the ceiling composition fails
tests/btw_is_read_only.rs:<line> (proof.txt is written)."
```

---

## Task 7: Fix the ephemeral-session leak

`src/routing/session_key.rs:76` says `Ephemeral session (no persistence)`. `src/gateway/session_store/file_backend/mod.rs:343` writes `session_type: "ephemeral"` into `SessionMetadata`. There is no sweeper anywhere. The comment is the lying half, and its real cost is that everyone who reads it concludes no cleanup is needed.

**Files:**
- Modify: `src/routing/session_key.rs:76`, `src/gateway/continuation_lifecycle.rs:95`
- Test: `src/gateway/continuation_lifecycle.rs` test module

**Interfaces:**
- Consumes: `side_key_for` (Task 2)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn an_epoch_bump_retires_the_old_side_session() {
    let store = session_store_for_test();
    let main = SessionKey::main("assistant");
    let side = crate::gateway::btw::side_key_for(&main);
    store.create_session(&side).await.expect("side session exists");

    terminate_session_continuations(&main.to_key_string(), "/new");
    // Give the fire-and-forget retirement a chance to land.
    tokio::task::yield_now().await;

    assert!(
        store.get_metadata(&side.to_key_string()).await.is_err(),
        "the old side session survived /new — it is now unreachable disk residue"
    );
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib an_epoch_bump_retires -- --nocapture`
Expected: FAIL — the side session still resolves.

- [ ] **Step 3: Fix the comment**

In `src/routing/session_key.rs`, replace lines 76–80:

```rust
    /// Ephemeral session: not part of any conversation's addressable history,
    /// but **stored like every other session**
    /// (`session_store::file_backend` writes it with `session_type =
    /// "ephemeral"`).
    ///
    /// The name once said "no persistence", which was never true and was the
    /// reason nothing ever cleaned these up: every reader concluded there was
    /// nothing to clean. Whoever creates one owns retiring it — see
    /// `gateway::continuation_lifecycle::terminate_session_continuations` for
    /// the `/btw` side session's retirement.
    Ephemeral {
        agent_id: String,
        ephemeral_id: String,
    },
```

- [ ] **Step 4: Add the retirement**

In `src/gateway/continuation_lifecycle.rs::terminate_session_continuations`, after the existing loop/goal handling:

```rust
    // The `/btw` side session is derived from the retiring key, so the epoch
    // bump already makes it unaddressable. Deleting it here is what keeps it
    // from becoming permanent residue — a missed delete costs disk, never a
    // crossed side thread, which is why the derivation includes the epoch.
    if let Some(key) = SessionKey::parse(old_session) {
        let side = crate::gateway::btw::side_key_for(&key);
        if let Some(store) = crate::gateway::session_store::global() {
            let side_str = side.to_key_string();
            tokio::spawn(async move {
                if let Err(e) = store.delete_session(&side_str).await {
                    tracing::debug!(session = %side_str, error = %e, "btw: side session retire");
                }
            });
        }
    }
```

Use whatever global accessor the store actually exposes — `grep -n "pub fn global" src/gateway/session_store/mod.rs`. If deletion is not on the trait, use the nearest existing removal method rather than adding one.

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p alephcore --lib an_epoch_bump_retires -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run the minimum verification set, then commit**

```bash
git add src/routing/session_key.rs src/gateway/continuation_lifecycle.rs
git commit -m "gateway: retire the btw side session on epoch bump, and stop the doc lying about ephemeral persistence"
```

---

## Task 8: Channel face — delivery and side-answer marking

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs`
- Test: `src/gateway/inbound_router/tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_side_answer_is_marked_so_it_reads_as_a_side_answer() {
    let out = super::format_side_answer("the file is config.toml");
    assert!(out.starts_with("💬 "), "got: {out}");
    assert!(out.contains("the file is config.toml"));
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib a_side_answer_is_marked -- --nocapture`
Expected: FAIL — `cannot find function 'format_side_answer'`.

- [ ] **Step 3: Implement**

```rust
/// Prefix a side answer so it is visually separable from the main run's
/// replies, which arrive in the same conversation.
///
/// A side answer deliberately does NOT wait behind the main session's queued
/// replies: ordering protects a causal chain (reply B may quote reply A), and
/// a side answer is on no such chain. Making it queue would trade the entire
/// value of the feature — an immediate answer — for an ordering property that
/// says nothing about it. The visible cost is that a side answer can land
/// between two main replies; this marker is what makes that legible.
pub(super) fn format_side_answer(text: &str) -> String {
    format!("💬 {text}")
}
```

Apply it where the btw run's final text is emitted to the reply route.

- [ ] **Step 4: Run and watch it pass, then run the channel QA**

Run: `cargo test -p alephcore --lib a_side_answer_is_marked -- --nocapture`
Expected: PASS.

Run: `qa/channels/run.sh`
Expected: all 16 assertions pass. Record any pre-existing failures in the commit body rather than fixing them here.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/inbound_router/
git commit -m "gateway: mark btw side answers on the channel face"
```

---

## Task 9: TUI overlay

**Files:**
- Create: `interfaces/tui/src/tui/btw_overlay.rs`
- Modify: `interfaces/tui/src/tui/mod.rs`, `interfaces/tui/src/tui/render.rs`, `interfaces/tui/src/tui/keys.rs`

**Interfaces:**
- Consumes: `AppState::frame_belongs_here` (Task 1)
- Produces: `BtwOverlay { exchanges: Vec<BtwExchange>, view_index: usize, active: Option<BtwActive> }`

Follow the existing modal pattern in `interfaces/tui/src/tui/approval.rs` — same construction, same tick integration, same session filter. Do not invent a second overlay mechanism.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_overlay_pages_through_history_without_running_off_either_end() {
    let mut o = BtwOverlay::default();
    o.finish_exchange(BtwExchange::answered("q1", "a1"));
    o.finish_exchange(BtwExchange::answered("q2", "a2"));
    assert_eq!(o.view_index, 1, "a new answer is the one on screen");

    o.page_left();
    assert_eq!(o.view_index, 0);
    o.page_left();
    assert_eq!(o.view_index, 0, "paging past the start must clamp, not wrap");

    o.page_right();
    o.page_right();
    assert_eq!(o.view_index, 1, "paging past the end must clamp, not wrap");
}

#[test]
fn the_overlay_only_shows_frames_from_the_side_session() {
    let mut o = BtwOverlay::default();
    o.side_session_key = "agent:main:ephemeral:btw-abc".into();
    assert!(o.accepts_frame(Some("agent:main:ephemeral:btw-abc")));
    assert!(!o.accepts_frame(Some("agent:main:main")));
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p aleph-tui btw_overlay -- --nocapture`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the controller**

Create `interfaces/tui/src/tui/btw_overlay.rs` with `BtwExchange { question, answer, aborted, error }`, `BtwActive { question, answer, tool_name }`, and `BtwOverlay` holding `exchanges`, `view_index`, `active`, `side_session_key`. Implement `finish_exchange`, `page_left`, `page_right` (both saturating — never wrapping), and `accepts_frame`.

Keys, matching the reference implementation:

| Key | Action |
|---|---|
| `Enter` | submit the follow-up in the input |
| `Esc` | abort while answering; close when idle |
| `c` | copy the current answer (raw markdown) |
| `←` / `→` | page history |
| `↑` / `↓` | scroll a long answer |
| `p` | promote the current answer (Task 10) |

- [ ] **Step 4: Run and watch it pass**

Run: `cargo test -p aleph-tui btw_overlay -- --nocapture`
Expected: 2 PASS.

- [ ] **Step 5: Verify on a real machine**

Drive the TUI over a pty (see the `reference-realmachine-qa-rig-additions` memory), type `/btw what files are in src?` while a main run is active, and confirm: the overlay opens, the answer streams into it, and **nothing** about the side question appears in the main transcript.

- [ ] **Step 6: Commit**

```bash
git add interfaces/tui/src/tui/
git commit -m "tui: add the /btw side-question overlay"
```

---

## Task 10: Explicit promote

Nothing crosses into the main conversation unless the user asks out loud. When it does, the injected text must be classifiable, because text riding the `User` role is otherwise replayed verbatim as if the user had typed it — the most expensive single carrier can consume 20k of the user budget.

**Files:**
- Modify: `src/thinker/nudges.rs`
- Test: `src/thinker/nudges.rs` test module

**Interfaces:**
- Produces: `pub fn promoted_side_answer(question: &str, answer: &str) -> String`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_promoted_side_answer_is_classified_as_synthetic() {
    let text = promoted_side_answer("what is X?", "X is the config loader.");
    assert!(
        is_synthetic_reminder(&text),
        "a promoted answer replayed as the user's own words eats the user budget"
    );
    assert!(text.contains("X is the config loader."));
    assert!(text.contains("what is X?"), "the question gives the answer its referent");
}

#[test]
fn a_real_user_interjection_is_still_not_synthetic() {
    // Control: promote must not widen the classifier into swallowing genuine
    // user steering, which rides the same fence.
    assert!(!is_synthetic_reminder(&user_interjection_note("do it faster")));
}
```

- [ ] **Step 2: Run and watch it fail**

Run: `cargo test -p alephcore --lib promoted_side_answer -- --nocapture`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

```rust
/// Carrier for a `/btw` answer the user explicitly promoted into the main
/// conversation.
///
/// Rides the `User` role because that is the only role a client may append,
/// but it is NOT the user's own words — so it must be classifiable by
/// [`is_synthetic_reminder`]. Verbatim-fidelity paths skip only summaries; an
/// unclassified carrier on this role is replayed whole as user speech, and
/// this one can be an entire tool-assisted answer.
///
/// Deliberately not [`user_interjection_note`]: that fence wraps text the user
/// really did type, and the classifier must keep telling the two apart.
#[must_use]
pub fn promoted_side_answer(question: &str, answer: &str) -> String {
    format!(
        "<system-reminder>\nThe user promoted a side question into this \
         conversation.\n\nQ: {}\n\nA: {}\n</system-reminder>",
        crate::utils::xml_util::escape_xml(question),
        crate::utils::xml_util::escape_xml(answer),
    )
}
```

Extend `is_synthetic_reminder` to recognise it, following the ordering documented at `nudges.rs:250` — the interjection fence check must keep running first.

- [ ] **Step 4: Wire the two promote surfaces**

- Channel: `BtwTurn { promote: true }` appends `promoted_side_answer(..)` of the latest side exchange to the main session.
- TUI: the overlay's `p` key sends `/btw promote`.

Both call the same function. Neither writes the main session from any other path.

- [ ] **Step 5: Run and watch it pass**

Run: `cargo test -p alephcore --lib promoted_side_answer a_real_user_interjection -- --nocapture`
Expected: 2 PASS.

- [ ] **Step 6: Commit**

```bash
git add src/thinker/nudges.rs src/gateway/
git commit -m "thinker: add a classifiable carrier for promoted btw answers"
```

---

## Task 11: Entropy reduction — delete the old router special case

**Files:**
- Modify: `src/gateway/inbound_router/command_handler.rs` (remove lines 51 variant, 67 arm, 256 fn, 591–630 tests)
- Modify: `src/gateway/inbound_router/mod.rs` (remove the dispatch at 826)

- [ ] **Step 1: Delete**

Remove, in this order:
1. `SpecialSlash::Btw { body }` from the enum (line ~51)
2. The `"btw" => { ... }` arm in `classify_special_slash` (line ~67) — keep `Help` and `Stop`
3. `handle_btw` entirely (line ~256)
4. The four btw tests: `classify_btw_lowercase`, `classify_btw_uppercase_preserves_body_case`, `classify_btw_at_bot_preserves_body_case`, `classify_btw_newline_separator` (lines ~591–630)
5. The `Some(SpecialSlash::Btw { body }) => ...` dispatch arm in `mod.rs` (~826) and the `/btw` mention in the comment at ~820

- [ ] **Step 2: Compile and read the warnings before the errors**

Run: `cargo test -p alephcore --lib --no-run 2>&1 | grep -E "^(warning|error)" | head -30`
Expected: clean. An `unused variable` or `unused import` here means something else referenced the deleted path — follow it rather than silencing it.

- [ ] **Step 3: Confirm nothing still references it**

Run: `grep -rn "SpecialSlash::Btw\|handle_btw" src/`
Expected: no output. A comment-only hit is still a hit — a stale comment is how a deleted mechanism keeps looking wired.

- [ ] **Step 4: Register `/btw` for discovery**

Add `btw` to the shared command registry so `commands.list`, the TUI command tree and `/help` all list it. Find the registration table with:

```bash
grep -rn "fn builtin_commands\|COMMANDS: &\[" src/command/
```

Add one entry: name `btw`, description `Ask a read-only side question without interrupting the main run`. Do **not** add a second registry.

- [ ] **Step 5: Run the minimum verification set, then commit**

```bash
git add src/gateway/inbound_router/ src/command/
git commit -m "gateway: delete the inbound-only btw special case, register /btw for discovery"
```

---

## Task 12: The remaining guards

Guards 1 (Task 6) and 4 (Task 4) already exist and were falsified. This task adds the other four.

**Files:**
- Modify: `src/gateway/btw/tests.rs`

- [ ] **Step 1: Write all four**

```rust
/// Guard 2 — every surface derives btw the same way.
///
/// Derived from the call sites of `stamp_slash_mode` rather than from a list
/// of three surface names: a fourth surface added later would satisfy a
/// name list by not being on it.
#[test]
fn every_stamp_slash_mode_call_site_also_gets_btw() {
    let mut checked = 0usize;
    for path in ["src/bin/aleph-server/server_init.rs", "src/gateway/execution_engine/execute.rs"] {
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .replace('\r', "");
        let production = src.split("#[cfg(test)]").next().unwrap_or_default();
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if line.contains("stamp_slash_mode(") {
                checked += 1;
            }
        }
    }
    assert!(checked >= 3, "expected at least 3 stamp_slash_mode call sites, found {checked}");
    // btw is stamped INSIDE stamp_slash_mode, so every call site inherits it.
    let engine_src = std::fs::read_to_string("src/gateway/execution_engine/slash_command.rs")
        .expect("slash_command.rs")
        .replace('\r', "");
    let production = engine_src.split("#[cfg(test)]").next().unwrap_or_default();
    assert!(
        production.contains("stamp_btw(input, metadata)"),
        "stamp_slash_mode no longer stamps btw — the surfaces have silently diverged"
    );
}

/// Guard 5 — promote is the only path from the side session to the main one.
#[test]
fn nothing_in_the_btw_module_writes_the_main_session() {
    let mut checked = 0usize;
    for entry in std::fs::read_dir("src/gateway/btw").expect("btw dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read");
        let production = src.replace('\r', "");
        let production = production.split("#[cfg(test)]").next().unwrap_or_default();
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            assert!(
                !line.contains("append_message") && !line.contains("emit_event"),
                "{}: writes a session directly — promote (nudges::promoted_side_answer) \
                 is the only sanctioned path into the main conversation:\n  {line}",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "scanned no production files — the guard is blind");
}

/// Guard 6 — btw is not a session knob.
///
/// The five real knobs are written back onto the session so a choice outlives
/// its turn. btw must affect exactly one call; filing it with them would let
/// one side question drop the main conversation to Plan permanently.
#[test]
fn btw_is_not_registered_as_a_session_knob() {
    for path in [
        "src/gateway/handlers/sessions/modify.rs",
        "src/gateway/session_snapshot.rs",
    ] {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let src = src.replace('\r', "");
        let production = src.split("#[cfg(test)]").next().unwrap_or_default();
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            assert!(
                !line.contains("\"btw\""),
                "{path}: btw appears in the session-knob machinery:\n  {line}"
            );
        }
    }
}

/// Guard 3's companion — the side key derivation has exactly one writer.
#[test]
fn the_side_key_is_derived_in_one_place_only() {
    let mut hits = 0usize;
    for entry in walk_rs_files("src") {
        let src = std::fs::read_to_string(&entry).expect("read").replace('\r', "");
        let production = src.split("#[cfg(test)]").next().unwrap_or_default();
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if line.contains("btw-") && line.contains("format!") {
                hits += 1;
            }
        }
    }
    assert_eq!(
        hits, 1,
        "the `btw-` id is built in {hits} places; write and read must be one function"
    );
}
```

Write `walk_rs_files` as a small recursive helper in the same test module, or reuse an existing one — `grep -rn "fn walk_rs_files\|fn rust_files" src/`.

- [ ] **Step 2: Run them**

Run: `cargo test -p alephcore --lib gateway::btw::tests -- --nocapture`
Expected: all PASS.

- [ ] **Step 3: Falsify guard 5 by hand**

Add a line `session.emit_event(main, ev);` to `src/gateway/btw/seed.rs`. Run:

Run: `cargo test -p alephcore --lib nothing_in_the_btw_module_writes_the_main_session`
Expected: **FAIL**, naming `src/gateway/btw/seed.rs` and printing the offending line. Revert, confirm green.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/btw/tests.rs
git commit -m "gateway: add the four remaining btw guards

Falsified: an emit_event added to seed.rs fails
nothing_in_the_btw_module_writes_the_main_session, naming the file and line."
```

---

## Task 13: Documentation

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/GATEWAY.md`, `docs/reference/SECURITY.md`

- [ ] **Step 1: `FEATURE_LOCATOR.md`**

`/btw` has **zero** mentions today. Add a section under the Gateway chapter (§4) numbered after the last existing subsection, covering: what it is; the six defects the round fixed with their anchors; the three primitives it connects to; the surface table; and the **Panel face as a declared boundary**, not an omission:

```markdown
> **Panel 面：已声明的边界。** Panel 刻意不接 `/btw`（2026-08-20 裁定）。
> 接它需要新 overlay 组件 + 帧路由改造（`resolve_target` 现会把 ephemeral
> 会话当 background run），约为 TUI 面两倍工作量。这不是遗漏——写在这里
> 是为了让下一个读者不用重新发现一遍。
```

Also add the one-line entry to the §0 速查索引 table: 口语关键词「侧问 / 顺口一问 / btw」→ 规范名 `/btw` → 状态。

- [ ] **Step 2: `GATEWAY.md`**

Document `stamp_slash_mode` as the three-face chokepoint and why btw must be stamped before the busy lane. Include the failure mode in one sentence: *stamped after the lane gate, every `/btw` sent during a run is folded into that run as steering text.*

- [ ] **Step 3: `SECURITY.md`**

Add to the exec-tier section: btw composes an `ExecTier::Plan` ceiling through `most_restrictive`, and **revokes both `PLAN_REACHABLE_TOOLS` carve-outs**, with the reason for each (`scratchpad` writes the main session's plan; a `subagent` outlives the side session with no surface able to enumerate it). Note that the revocation cannot go through `allowed` because `is_allowed` exempts the attached subagent tool (`src/tools/scoped/builder.rs:411`).

- [ ] **Step 4: Update the criteria list if the round earned it**

Two candidates from this round, both new shapes:

- *「一个专属于某一张脸的模块里的推导，等于这个能力只有那一张脸——而其它脸够不到它这件事没有任何测试会说」*
- *「一个 allow-set 里的豁免，会让任何试图经它收窄的新规则静默失效」* (`is_allowed`'s subagent exemption)

Add them to `CLAUDE.md`'s §0 only if they are not already covered by an existing entry. Check first.

- [ ] **Step 5: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "docs: record the /btw side-question round"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §1.1 context inheritance | 5, 6 |
| §1.2 read-only | 4, 6 |
| §1.3 side-thread memory | 2 (stable key), 5 (cursor) |
| §1.4 face count | 3 (chokepoint), 8 (channel), 9 (TUI), CLI free via `agent.run` |
| §1.5 discoverability | 11 step 4 |
| §1.6 leak | 7 |
| §4.1 not a knob | 12 guard 6 |
| §4.2 chokepoint | 3 |
| §4.4.1 carve-out revocation | 4 |
| §5.1 incremental seed | 5 |
| §5.2 retirement | 7 |
| §6.1 TUI frames | 0, 1 |
| §6.2 delivery ordering | 8 |
| §7 promote | 10 |
| §8 entropy | 11 |
| §9 six guards | 4, 5, 6, 12 |
| §12 docs | 13 |

No gaps.

**Placeholder scan:** every code step carries real code; every run step carries a real command and an expected result. The `<line>` markers in commit bodies are values the implementer records from an observed failure, not deferred work.

**Type consistency:** `BtwTurn { question, promote }`, `side_key_for(&SessionKey) -> SessionKey`, `BTW_METADATA_KEY`, `SeedOutcome { events_added, cursor }`, `ensure_seeded(session, main, side, turns)`, `TurnContext.side_question`, `GateRule::SideQuestion`, `promoted_side_answer(question, answer)`, `frame_belongs_here(Option<&str>)` — each defined once and referenced consistently.

**Known adaptation points** (the implementer confirms against real signatures, and the step says so inline): `SessionService` metadata accessors (Task 5), `fork::seed_events` extraction (Task 5), the `test-helpers` gateway harness (Task 6), `session_store::global` + delete (Task 7), the command registry table (Task 11).
