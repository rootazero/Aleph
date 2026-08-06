//! Durable registry + regression net for per-user session visibility (P1
//! data isolation, spec §5.4).
//!
//! Sibling of `method_admin.rs` — same shape, different question.
//! `method_admin.rs` asks "does this method require operator privilege";
//! this file asks "does this method's response depend on WHO is asking, and
//! if so, is that dependency enforced." **This registry is NOT a dispatch
//! gate** — nothing here intercepts a request. Filtering a `sessions.list`
//! response or denying a foreign `session_key` needs per-method data access
//! (the session's own `owner_user_id`), which a generic method-name gate
//! cannot supply. Enforcement lives in each handler (`gateway::visibility`'s
//! predicates, applied at the site — see `session/db_handlers/{query,
//! modify}.rs` and `handlers/chat.rs`). This table exists so a REMOVED or
//! never-added enforcement call is a named test failure, not a silent gap —
//! the same audit-trail role `ADMIN_PREFIXES`/`MEMBER_CARVE_OUTS` play for
//! the admin gate.
//!
//! ## Scope of this table (read before extending it)
//!
//! Task 6 (this file's origin) owns the `sessions.*`/`session.*`/`chat.*`
//! family. Task 7 of the same plan
//! (`docs/superpowers/plans/2026-08-05-p1-data-isolation.md`) owns
//! `memory.*`, `artifacts.*`, `clarification.*`, `subagent.tree`, and
//! `graph.query` — all five are now enforced and registered below.
//!
//! ## Task 7 additions
//!
//! - `memory.search` / `memory.listFacts` / `graph.query` → **PartitionChecked**:
//!   each runs its (possibly-defaulted) `agent_id` through
//!   `visibility::partition_visible` before touching the store; an invisible
//!   partition reads as an empty result — the SAME shape a genuinely unused
//!   `agent_id` produces, so there is no existence oracle distinguishing
//!   "denied" from "never used".
//! - `memory.stats` → **PartitionChecked**: an explicit invisible `agent_id`
//!   reads as a real-but-empty partition (same no-oracle contract); an
//!   *omitted* `agent_id` additionally now depends on the caller —
//!   unrestricted keeps the pre-P1 whole-store rollup, a member is scoped to
//!   the org partition (`crate::routing::DEFAULT_AGENT_ID`) instead of ever
//!   falling through to the whole store just because they left the field off
//!   (see that handler's doc).
//! - `artifacts.list` / `artifacts.read_text` / `session.export_html` →
//!   **KeyChecked**: the Task-6 addressed-key pattern, applied as defense in
//!   depth (`artifacts.rs`'s own module doc — the store is already
//!   session-keyed and cross-session-proof, but a handler must not depend on
//!   `sessions.list` having filtered the key out first).
//! - `clarification.resolve` → **KeyChecked**, same pattern; a malformed
//!   `session_key` is a separate, pre-existing `INVALID_PARAMS` validation
//!   error, not an existence question.
//! - `clarification.pending` → **ListFiltered**: the list is process-wide and
//!   small, so each item is filtered by ITS OWN session's visibility rather
//!   than pushed through a single `SessionFilter` — an unrestricted caller
//!   sees every pending item unchanged from pre-P1; a member sees only the
//!   ones whose session they own.
//! - `subagent.tree` → **ListFiltered**: registered under this label because
//!   its default (and panel-dashboard-common) shape — `root_session`
//!   omitted — is exactly `ListFiltered`'s contract (unrestricted caller
//!   unchanged whole-process view; a member gets the same flat list filtered
//!   to roots whose session is visible, never an error — an empty tree is a
//!   valid answer). The method ALSO accepts an optional `root_session`, in
//!   which case it additionally applies the Task-6 addressed-key check
//!   (`not_found_response` on a foreign owner) — the same "optional key
//!   narrows an otherwise-list endpoint" shape `chat.abort` uses for
//!   `KeyChecked` (its `session_key` is optional too; absent, it does
//!   nothing session-scoped). One `Treatment` per method cannot carry both
//!   labels; `handle_tree`'s own doc is the source for the addressed-key
//!   half.
//!
//! ## Task 7 fix round 1
//!
//! Review triage on Task 7 found four more member-reachable methods sharing
//! the exact same unenforced `agent_id`/bare-`id` shape as the four above —
//! all four are now fixed and registered, closing the "Known gaps" bullets
//! this section used to carry for them:
//!
//! - `memory.trace` → **PartitionChecked**, same contract as `memory.search`
//!   (invisible partition → an empty evidence chain, the same shape a
//!   genuinely unused partition produces).
//! - `graph.node_detail` → **PartitionChecked**: an invisible partition gets
//!   the exact same "not found" response a nonexistent node produces — no
//!   oracle, and the (potentially large, full-markdown) note body is never
//!   read for a denied caller.
//! - `graph.search` → **PartitionChecked**, same contract as `graph.query`
//!   (invisible partition → no hits, not a leaked title/tag/content).
//! - `memory.delete` → **PartitionChecked**, but shaped differently from its
//!   siblings: the RPC addresses a row by bare `id` with no `agent_id` at
//!   all, so there is no caller-supplied partition to check directly. The
//!   fix resolves the row's OWNING partition FIRST (a new, deliberately
//!   narrow `SqliteMemoryBackend::raw_memory_agent_id` lookup — the existing
//!   `delete_raw_memory` query itself was NOT widened) and runs THAT through
//!   `visibility::partition_visible` before the delete executes. A
//!   nonexistent id and an id whose partition is invisible to the caller
//!   produce the exact same response (byte-identical, tested), and a denied
//!   delete never touches the row.
//!
//! `graph.update_note`/`rename_note`/`delete_note` (siblings of the now-fixed
//! `graph.search`/`graph.node_detail`) were NOT checked this round — still a
//! recorded gap, see below.
//!
//! ## Task 7 fix round 2
//!
//! Review triage ruled the `graph.update_note`/`rename_note`/`delete_note`
//! trio MUST-FIX: same unenforced `agent_id` shape as `graph.search`/
//! `graph.node_detail`, member-reachable (`graph.` absent from
//! `ADMIN_PREFIXES`), and all three MUTATE — strictly worse than the reads
//! this task already closed. All three are now **PartitionChecked**, but
//! each denial reuses whatever THAT specific operation's own natural
//! "genuinely nonexistent node" response already is, rather than inventing
//! one new shared shape:
//!
//! - `graph.update_note` → upsert (create-or-overwrite) with no "not found"
//!   error of its own — a nonexistent node_id under a VISIBLE partition
//!   already just creates it, reporting success. So an invisible partition
//!   gets that SAME fixed success shape (`{node_id, saved: true}`, provably
//!   constant regardless of new-vs-existing — no dynamic content to
//!   byte-compare), without the store ever being touched.
//! - `graph.delete_note` → idempotent — `NoteIndexer::delete_note`'s own doc
//!   states deleting a note that never existed already reports success, not
//!   an error. An invisible partition gets that SAME idempotent-success
//!   shape (`{node_id, deleted: true}`), without the store ever being
//!   touched.
//! - `graph.rename_note` → the one genuine exception: renaming a missing
//!   note is NOT idempotent, and `NoteIndexer::rename_note` hits the
//!   filesystem `rename()` syscall directly, so its natural failure message
//!   embeds the real on-disk path — reusing it verbatim for an invisible
//!   partition would leak exactly what P1 exists to hide. Rather than
//!   duplicate that internal path-reconstruction logic to fabricate a
//!   byte-identical leaky message, the handler gained its OWN existence
//!   check (`NoteStore::find_by_filename`, matching `rename_note`'s own
//!   internal category-resolution-by-title so a stale/wrong category in the
//!   caller's `node_id` — a pre-existing, deliberate tolerance — is not
//!   mistaken for "not found") that short-circuits BEFORE the indexer is
//!   ever called, for BOTH the invisible-partition case and (as a
//!   necessary, not scope-creeping, side effect) the genuinely-missing-under-
//!   a-visible-partition case — both now produce `graph.node_detail`'s
//!   established clean "Note not found: {node_id}" shape instead of a raw
//!   OS path. Tested with the same same-node_id-vs-fresh-store comparison
//!   methodology fix round 1 established.
//!
//! No further same-shape siblings were found while fixing this trio.
//!
//! ## Enumeration evidence
//!
//! Every method below was found by a mechanical sweep of the same four
//! registration patterns `method_admin.rs` swept for the 74-family admin
//! table: `register_handler!` (files under
//! `src/bin/aleph-server/commands/start/builder/handlers/`), direct
//! `registry.register(...)` placeholders (`src/gateway/handlers/mod.rs`),
//! `server.handlers_mut().register(...)` inline in `agent_init/mod.rs` and
//! `agent_init/common_handlers.rs`, and `commands/start/mod.rs`'s inline
//! registrations — filtered to methods whose handler reads a caller-supplied
//! or caller-scoped `session_key` (or, for `sessions.list`, filters by
//! agent/owner). Read (not guessed) per method:
//!
//! - `sessions.list` → `handle_list_db` sets `SessionFilter.owner_visible_to`
//!   from `visibility::visible_owner_filter()` — **ListFiltered**.
//! - `sessions.history`, `sessions.preview`, `session.usage`,
//!   `sessions.delete`, `sessions.reset` → each resolves a caller-supplied
//!   `session_key` to `SessionMetadata` and calls `visibility::
//!   session_visible` before touching the store — **KeyChecked**.
//! - `chat.send` → the REAL-provider production path
//!   (`server_init.rs::handle_chat_send_with_engine`) sends session
//!   resolution through `visibility::existing_session_is_visible` before the
//!   run starts (a session that doesn't exist yet is not a denial — see that
//!   fn's doc); registered here as **KeyChecked** on that basis. **Carve-out:
//!   the Simulated-execution fallback path** (`chat_handlers::handle_send` →
//!   `AgentRunManager::start_run`, used only when no LLM provider is
//!   configured) is NOT covered — `AgentRunManager` has no `SessionStore`
//!   dependency, and this table must not overstate what's actually enforced.
//! - `chat.abort`, `chat.history`, `chat.clear`, `chat.rewind` →
//!   `KeyChecked`, same pattern (`chat.abort`'s `session_key` is optional;
//!   absent it does nothing session-scoped, present it is checked).
//! - `sessions.new` (`handle_new_session_db`) → KeyChecked on the addressed
//!   (closing) session, before continuation termination or `close_session`.
//! - `sessions.patch` (`handle_patch_db`) → KeyChecked before any field
//!   validation, so a foreign caller gets an identical denial regardless of
//!   what they put in `metadata` (no oracle via a validation-error side
//!   channel).
//! - `sessions.set_project_root` (`handle_set_project_root_db`) → KeyChecked
//!   before the write; without it a foreign caller could redirect the
//!   victim's next run to an attacker-chosen filesystem path.
//! - `session.compact` (`handle_compact_db`) → KeyChecked; this one is a
//!   **content-disclosure** site (the RPC response includes a summary of the
//!   session's real messages) as well as a mutation (irreversibly rewrites
//!   the event log), so it needed a new `SessionStore` parameter that didn't
//!   exist before (the compaction operation itself still doesn't use the
//!   store — see the fn's doc).
//! - `session.truncate` (`handle_truncate_db`) → KeyChecked before the
//!   irreversible tail deletion.
//! - `sessions.compaction.list` (`handle_list_checkpoints_db`) → KeyChecked;
//!   without it a foreign caller learns whether/how many checkpoints a
//!   victim session has.
//! - `sessions.compaction.restore` (`handle_restore_checkpoint_db`) →
//!   KeyChecked on the addressed session, before it's overwritten with
//!   checkpoint content.
//! - `sessions.compaction.branch` (`handle_branch_checkpoint_db`) → the
//!   worst of this batch: it copies the SOURCE session's full verbatim
//!   checkpoint messages into `new_session_key` — a read compromise if the
//!   source isn't the caller's. **Two checks, not one**: the source
//!   (addressed) session is KeyChecked as usual; `new_session_key` (the
//!   caller-chosen TARGET) is separately checked for a collision — the store
//!   writes to it with no existence check of its own (confirmed by reading
//!   both backends), so a target key that already names a foreign session
//!   would otherwise be silently overwritten. There is no pre-existing
//!   "target already exists" error to reuse (the store never had one), so
//!   the target-collision case reuses the SAME `not_found_response` the
//!   source check uses rather than inventing a new error shape.
//! - `session.create` → creation surface with no addressed key, not
//!   enumerated by this table (nothing to check).
//!
//! ## Final-review fix round: the sweep's blind spot
//!
//! The Task 6/7 sweep keyed on handlers taking a `session_key` or `agent_id`
//! PARAMETER, which made it structurally blind to a handler that routes the
//! caller-supplied key through `AgentRouter` FIRST and only then holds a
//! `SessionKey`. `agent.run` is exactly that shape, and it is `chat.send`
//! with the guard missing:
//!
//! - `agent.run` → **KeyChecked**. The real-`ExecutionEngine` production path
//!   (`server_init.rs::handle_run_with_engine`) now runs session resolution
//!   through `visibility::existing_session_is_visible` immediately after
//!   `router.route` and before the run starts — byte-identical placement and
//!   semantics to its `chat.send` twin in the same file. **Same Simulated-
//!   execution carve-out as `chat.send`**: the no-provider fallback path
//!   (`handlers::agent::handle_run` → `AgentRunManager::start_run`,
//!   registered at `agent_init/mod.rs`'s simulated branch) is NOT covered,
//!   for the identical reason — `AgentRunManager` has no `SessionStore`
//!   dependency — and this table must not overstate what is enforced.
//!
//! The same blind spot hid two more families, which address their data by
//! `run_id` and by a group-chat `session_id` respectively:
//!
//! - `trace.by_runs` → **KeyChecked**. A persisted trace is a full transcript
//!   and the trace store records no owner, so ownership is resolved through
//!   the session instead: the method now takes a `session_key`, KeyChecks it
//!   with `visibility::session_visible`/`not_found_response`, and then serves
//!   only those requested `run_id`s that the session's own transcript
//!   attributes to itself (proving you own one session must not become a
//!   licence to read every run in the process). A run outside that set reads
//!   as `[]` — the same shape an unknown or trace-less run has always
//!   produced. Its siblings `trace.list`/`trace.get` are NOT in this table
//!   because they are not member-reachable at all: the `trace.` prefix is
//!   admin-gated in `method_admin.rs` and only `trace.by_runs` is carved out
//!   (`trace.list` was its own enumeration oracle — every `task_id` in the
//!   process — and the Panel calls neither).
//! - `group_chat.list` → **ListFiltered**; `group_chat.continue`/`mention`/
//!   `history`/`end` → **KeyChecked**. These sessions are NOT keyed by a
//!   `SessionKey` (`GroupChatSession::source_session_key` is the sentinel
//!   `"rpc:direct"` on the RPC path, so it names no session and cannot answer
//!   ownership), so `GroupChatSession` carries the same P1 owner stamp
//!   `SessionMetadata`/`LoopState`/`Goal` do, taken off the ambient scope in
//!   its constructor, and read through the shared
//!   `visibility::stamped_owner_visible`. `KeyChecked` here means "addressed
//!   record, ownership checked before anything else, denial reuses the
//!   method's own not-found response" — the label is about the shape of the
//!   check, not about the identifier being a `SessionKey`. `group_chat.start`
//!   is a creation surface with no addressed record (nothing to check),
//!   matching `session.create`'s ruling above.
//!
//! Five more siblings carried the caller-supplied-`agent_id` shape Task 7
//! ruled MUST-FIX, and were missed because that sweep stopped at the
//! `memory.*`/`graph.*` module boundary rather than following the shape:
//!
//! - `memory.retrieve_with_trace` → **PartitionChecked**. Strictly worse than
//!   the `memory.search` it sits next to: its results carry note CONTENT
//!   (truncated, not withheld). Invisible partition → empty stages, empty
//!   results, store untouched.
//! - `memory.list_corrections` → **PartitionChecked**. Rows carry verbatim
//!   `content` — things the user typed at the agent. Invisible partition →
//!   empty list, checked before the dream watermark is even read.
//! - `dreaming.list_insights` → **PartitionChecked**. Its `synthesis` list
//!   leaks note paths/titles/tags. Invisible partition → all three lists
//!   empty. (Only `synthesis` is genuinely partitioned; `daily` and `runs`
//!   read the whole store with no `agent_id` at all — pre-existing, recorded
//!   in the handler's doc, and the reason the denial empties the whole
//!   response instead of one list.)
//! - `insights.tools` → **PartitionChecked**. Discloses which tools a
//!   partition ran, how often, failure rates, distinct session counts. The
//!   denial is produced by the REAL aggregator over zero rows
//!   (`memory::insights::empty_tool_usage_report`) so it is byte-identical to
//!   a partition that ran nothing and cannot drift as the report type grows.
//! - `workspace.list` → **ListFiltered**, `workspace.get` →
//!   **PartitionChecked**. ⚠️ These two are DEFENSE IN DEPTH ONLY and the
//!   table would overstate them if read as more: a workspace id is a
//!   user-chosen name that encodes no owner and `agent_envs` has no owner
//!   column, so an ordinary workspace passes `partition_visible` for every
//!   caller and its `env_vars` are still cross-user readable. Closing that
//!   needs an owner column plus a migration — a schema and product decision,
//!   not a handler fix. Both handlers' docs say so at the site.
//!
//! ## `teams.*` (tightened 2026-08-06 — was `OrgShared`)
//!
//! P1 shipped this family as [`Treatment::OrgShared`]: `Team` carried no owner
//! field, so there was nothing to check without first inventing an ownership
//! model, and the plan deferred that to P2. The human ruled otherwise —
//! tighten it in P1 — so `Team` now carries `owner_user_id` on the same
//! adoption-by-absence terms as a session, and every addressed method below is
//! enforced.
//!
//! The enforcement shape differs from the rest of this table in one way worth
//! knowing before extending it. Teams are reachable from BOTH the gateway and
//! the `team_*` builtin tools, so the predicate lives in a `TeamStore`
//! decorator (`teams::scoped::ScopedTeamStore`) that both surfaces cross,
//! rather than at 50 call sites. The handlers still gate explicitly — via
//! `handlers::teams::visibility::{gate_team, gate_task}` — for the two things
//! a decorator cannot do: shape the byte-identical `not_found` response, and
//! reach the ~20 methods that address a team through the `coord_tasks` DAG (a
//! different database) rather than by `team_id`.
//!
//! Three `teams.*` methods are deliberately absent rather than registered:
//!
//! - `teams.create` — creates; there is no addressed record to check yet, same
//!   ruling as `session.create` / `group_chat.start`. The new row is stamped.
//! - `teams.list_templates` — reads template files off disk; carries no user
//!   data of any kind.
//! - `teams.chat.cancel` — addressed by `run_id` against the process-global
//!   `BackgroundAgentTracker`, with no run → team mapping to gate on. Left open
//!   on the §4.11 reasoning (an unguessable capability the caller can only have
//!   received from their own `teams.chat.send`), recorded at the handler. If a
//!   `teams.*` ENUMERATION face for run ids is ever added, this needs a real
//!   gate in the same change.
//!
//! Like every list in this file it is a curated pin, not a generated one: a
//! brand-new `teams.*` sibling does not fail a test by being absent. The
//! prefix-shaped `OrgShared` ruling used to give the family that property, and
//! removing it is the cost of enforcing per-method treatments.
//!
//! ## `projects.*` (P2 project rooms)
//!
//! A project room's roster IS its authorization (spec §6.1), so the predicate
//! is `visibility::project_visible` — membership, not owner equality. That is
//! the one structural difference from every family above: two different users
//! both legitimately see the same record.
//!
//! Enforcement is a single admission point in the handler file,
//! `handlers::projects::gate_project`, plus `require_owner` for the
//! owner-level verbs. `projects.list` filters; `projects.get` / `remove` /
//! `touch` / `rename` / `archive` / `member.*` are addressed and gated.
//!
//! Three siblings are deliberately absent rather than registered:
//!
//! - `projects.create` — creates an unbound room; no addressed record exists
//!   yet. Same ruling as `session.create` / `teams.create` / `group_chat.start`.
//! - `projects.add` — registers a folder. Also a creation surface: it resolves
//!   the caller off the ambient scope and collapses onto THEIR row for that
//!   path (`ProjectStore::find_by_path_for`, owner-keyed unique index), so it
//!   can never land on another user's room. There is no caller-supplied id to
//!   check.
//! - `projects.create_blank` — `mkdir` + `add`; same ruling, one layer up.
//!
//! Those three are asserted absent by name in the pin test below, because an
//! unregistered method and a method someone forgot are indistinguishable in
//! [`treatment_of`].
//!
//! ## Known gaps NOT covered by this table (found during the sweep, not
//! fixed — flagged here exactly as `method_admin.rs` flags its own
//! follow-ups)
//!
//! `sessions.set_topic` and `chat.context_estimate` take a caller-supplied
//! `session_key` with no ownership check today; deferred deliberately (lower
//! severity — a title-rename side effect and a token-count-only read,
//! respectively — and reviewed as out of this round's scope). The
//! Simulated-fallback `chat.send` path (see the `chat.send` bullet above) is
//! also a known, deliberate gap. `memory.reembed` is intentionally absent
//! from the table (whole-store maintenance, no per-user `agent_id` to check
//! — see the pin test). All are recorded here as the durable home for the
//! follow-up, same convention as `method_admin.rs`'s notes.
//! (`graph.update_note`/`rename_note`/`delete_note` were in this list until
//! Task 7 fix round 2 closed them — see that section above.)
//!
//! ## `OrgShared`
//!
//! No entry in this table is `OrgShared`. Every Task 6 method is genuinely
//! per-user (a session has exactly one owner); every Task 7 method is
//! per-user or per-partition, never org-level infrastructure. `OrgShared`
//! stays defined (see the enum doc) for a future task whose methods
//! genuinely are shared by design.

/// How a scoped-data method's response is (or should be) restricted to the
/// calling user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Treatment {
    /// A list/enumeration endpoint filters its result set by
    /// `visibility::visible_owner_filter()` (or the partition equivalent).
    ListFiltered,
    /// An addressed-key endpoint resolves the caller-supplied key and calls
    /// `visibility::session_visible` (or `existing_session_is_visible`)
    /// before doing anything else, denying with `visibility::
    /// not_found_response` on a foreign owner.
    KeyChecked,
    /// A partition-addressed endpoint (e.g. `agent_id="main__u-alice"`)
    /// checks `visibility::partition_visible` before reading/writing that
    /// partition. (Task 7: `memory.search`/`memory.listFacts`/`memory.stats`/
    /// `graph.query`/`memory.trace`/`graph.node_detail`/`graph.search`/
    /// `graph.update_note`/`graph.rename_note`/`graph.delete_note`;
    /// `memory.delete` too, via a resolved-owning-partition lookup since it
    /// has no caller-supplied `agent_id` of its own — see "Task 7 fix round
    /// 1"/"round 2" in the module doc.)
    PartitionChecked,
    /// Deliberately shared across every user by design — not a gap. Carries
    /// a reason string at the call site (see `org_shared_entries_all_carry_reasons`).
    /// Still unused after Task 7 — none of `memory.*`/`artifacts.*`/
    /// `clarification.*`/`subagent.tree`/`graph.query` are org-shared, they
    /// are all per-user or per-partition. Left for a future task whose
    /// methods genuinely are org-level infrastructure (e.g. `teams.*`).
    #[allow(dead_code)]
    OrgShared,
}

/// The durable pin list. One entry per method this file's module doc
/// attributes as enforced — see that doc for the sweep methodology and the
/// (recorded, not silently dropped) gaps this table does not yet cover.
pub const SCOPED_METHODS: &[(&str, Treatment)] = &[
    ("sessions.list", Treatment::ListFiltered),
    ("sessions.history", Treatment::KeyChecked),
    ("sessions.preview", Treatment::KeyChecked),
    ("session.usage", Treatment::KeyChecked),
    ("sessions.delete", Treatment::KeyChecked),
    ("sessions.reset", Treatment::KeyChecked),
    ("chat.send", Treatment::KeyChecked),
    ("chat.abort", Treatment::KeyChecked),
    ("chat.history", Treatment::KeyChecked),
    ("chat.clear", Treatment::KeyChecked),
    ("chat.rewind", Treatment::KeyChecked),
    ("sessions.new", Treatment::KeyChecked),
    ("sessions.patch", Treatment::KeyChecked),
    ("sessions.set_project_root", Treatment::KeyChecked),
    ("session.compact", Treatment::KeyChecked),
    ("session.truncate", Treatment::KeyChecked),
    ("sessions.compaction.list", Treatment::KeyChecked),
    ("sessions.compaction.restore", Treatment::KeyChecked),
    ("sessions.compaction.branch", Treatment::KeyChecked),
    // --- Task 7 ---
    ("memory.search", Treatment::PartitionChecked),
    ("memory.listFacts", Treatment::PartitionChecked),
    ("memory.stats", Treatment::PartitionChecked),
    ("graph.query", Treatment::PartitionChecked),
    ("artifacts.list", Treatment::KeyChecked),
    ("artifacts.read_text", Treatment::KeyChecked),
    ("session.export_html", Treatment::KeyChecked),
    ("clarification.resolve", Treatment::KeyChecked),
    ("clarification.pending", Treatment::ListFiltered),
    ("subagent.tree", Treatment::ListFiltered),
    // --- Task 7 fix round 1 ---
    ("memory.trace", Treatment::PartitionChecked),
    ("memory.delete", Treatment::PartitionChecked),
    ("graph.node_detail", Treatment::PartitionChecked),
    ("graph.search", Treatment::PartitionChecked),
    // --- Task 7 fix round 2 ---
    ("graph.update_note", Treatment::PartitionChecked),
    ("graph.rename_note", Treatment::PartitionChecked),
    ("graph.delete_note", Treatment::PartitionChecked),
    // --- Final-review fix round ---
    ("agent.run", Treatment::KeyChecked),
    ("trace.by_runs", Treatment::KeyChecked),
    ("group_chat.list", Treatment::ListFiltered),
    ("group_chat.continue", Treatment::KeyChecked),
    ("group_chat.mention", Treatment::KeyChecked),
    ("group_chat.history", Treatment::KeyChecked),
    ("group_chat.end", Treatment::KeyChecked),
    ("memory.retrieve_with_trace", Treatment::PartitionChecked),
    ("memory.list_corrections", Treatment::PartitionChecked),
    ("dreaming.list_insights", Treatment::PartitionChecked),
    ("insights.tools", Treatment::PartitionChecked),
    ("workspace.list", Treatment::ListFiltered),
    ("workspace.get", Treatment::PartitionChecked),
    // --- teams.* (see the module doc's `teams.*` section) ---
    ("teams.list", Treatment::ListFiltered),
    ("teams.snapshot.list", Treatment::ListFiltered),
    ("teams.get", Treatment::KeyChecked),
    ("teams.rename", Treatment::KeyChecked),
    ("teams.disband", Treatment::KeyChecked),
    ("teams.delete", Treatment::KeyChecked),
    ("teams.usage", Treatment::KeyChecked),
    ("teams.chat.send", Treatment::KeyChecked),
    ("teams.chat.thread", Treatment::KeyChecked),
    ("teams.chat.history", Treatment::KeyChecked),
    ("teams.list_tasks", Treatment::KeyChecked),
    ("teams.create_task", Treatment::KeyChecked),
    ("teams.update_task", Treatment::KeyChecked),
    ("teams.list_task_runs", Treatment::KeyChecked),
    ("teams.add_task_comment", Treatment::KeyChecked),
    ("teams.list_task_comments", Treatment::KeyChecked),
    ("teams.list_task_events", Treatment::KeyChecked),
    ("teams.task.trace", Treatment::KeyChecked),
    ("teams.task.pause", Treatment::KeyChecked),
    ("teams.task.resume", Treatment::KeyChecked),
    ("teams.task.retry", Treatment::KeyChecked),
    ("teams.task.skip", Treatment::KeyChecked),
    ("teams.task.journal.get", Treatment::KeyChecked),
    ("teams.task.journal.list", Treatment::KeyChecked),
    ("teams.workflow.approve_step", Treatment::KeyChecked),
    ("teams.workflow.reject_step", Treatment::KeyChecked),
    ("teams.acp_member.add", Treatment::KeyChecked),
    ("teams.acp_member.remove", Treatment::KeyChecked),
    ("teams.acp_member.list", Treatment::KeyChecked),
    ("teams.snapshot.create", Treatment::KeyChecked),
    ("teams.snapshot.get", Treatment::KeyChecked),
    ("teams.snapshot.restore", Treatment::KeyChecked),
    ("teams.snapshot.delete", Treatment::KeyChecked),
    // --- projects.* (P2 project rooms — see the module doc's section) ---
    ("projects.list", Treatment::ListFiltered),
    ("projects.get", Treatment::KeyChecked),
    ("projects.remove", Treatment::KeyChecked),
    ("projects.touch", Treatment::KeyChecked),
    ("projects.rename", Treatment::KeyChecked),
    ("projects.archive", Treatment::KeyChecked),
    ("projects.member.add", Treatment::KeyChecked),
    ("projects.member.remove", Treatment::KeyChecked),
    ("projects.member.list", Treatment::KeyChecked),
];

/// Whole families ruled [`Treatment::OrgShared`], as prefixes.
///
/// The exact-match [`SCOPED_METHODS`] table is the right shape for a method
/// that was individually read and enforced; it is the wrong shape for a
/// family whose ruling is "every method here, present and future, is
/// org-level infrastructure" — enumerating 37 `teams.*` methods would mean a
/// new sibling silently drops out of the audit trail, which is the failure
/// this file exists to prevent. Prefix form (the `method_admin.rs` idiom this
/// file is the sibling of) makes the ruling cover the family by construction.
///
/// Every entry needs a reason in [`ORG_SHARED_REASONS`], enforced by test.
///
/// Empty since 2026-08-06: `teams.` was the only entry and its ruling was
/// overturned — every method in the family is now individually enforced and
/// listed in [`SCOPED_METHODS`]. The mechanism stays because the shape is
/// right for the next family that genuinely is shared by design; an empty list
/// is a stronger statement than a deleted one, namely that nothing today
/// claims "shared" without a per-method audit.
const ORG_SHARED_PREFIXES: &[&str] = &[];

/// `OrgShared` entries carry a one-line reason at the point they're listed,
/// keyed by the exact method or by the family prefix
/// ([`ORG_SHARED_PREFIXES`]).
pub const ORG_SHARED_REASONS: &[(&str, &str)] = &[];

#[must_use]
pub fn treatment_of(method: &str) -> Option<Treatment> {
    if let Some(t) = SCOPED_METHODS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, t)| *t)
    {
        return Some(t);
    }
    ORG_SHARED_PREFIXES
        .iter()
        .any(|p| method.starts_with(p))
        .then_some(Treatment::OrgShared)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Curated pin, not a second source of truth — same philosophy as
    /// `method_admin.rs`'s `credential_and_config_methods_require_admin`: a
    /// deletion or typo in `SCOPED_METHODS` should fail a test by name.
    #[test]
    fn every_session_addressed_method_is_registered() {
        for m in [
            "sessions.list",
            "sessions.history",
            "sessions.preview",
            "session.usage",
            "sessions.delete",
            "sessions.reset",
            "chat.send",
            "chat.abort",
            "chat.history",
            "chat.clear",
            "chat.rewind",
            "sessions.new",
            "sessions.patch",
            "sessions.set_project_root",
            "session.compact",
            "session.truncate",
            "sessions.compaction.list",
            "sessions.compaction.restore",
            "sessions.compaction.branch",
        ] {
            assert!(
                treatment_of(m).is_some(),
                "{m} must be registered in SCOPED_METHODS"
            );
        }
    }

    #[test]
    fn list_methods_are_list_filtered_not_key_checked() {
        assert_eq!(treatment_of("sessions.list"), Some(Treatment::ListFiltered));
    }

    #[test]
    fn addressed_key_methods_are_key_checked() {
        for m in [
            "sessions.history",
            "sessions.preview",
            "session.usage",
            "sessions.delete",
            "sessions.reset",
            "chat.send",
            "chat.abort",
            "chat.history",
            "chat.clear",
            "chat.rewind",
            "sessions.new",
            "sessions.patch",
            "sessions.set_project_root",
            "session.compact",
            "session.truncate",
            "sessions.compaction.list",
            "sessions.compaction.restore",
            "sessions.compaction.branch",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
    }

    #[test]
    fn unregistered_method_reads_as_none_not_a_default_treatment() {
        // No silent "assume KeyChecked" default — an unlisted method must
        // read as unclassified, not falsely covered. `sessions.set_topic`,
        // `chat.context_estimate`, and `memory.reembed` (whole-store, no
        // `agent_id` at all) are DELIBERATE, documented gaps (see module
        // doc) — not silently dropped, but also not falsely claimed.
        // (`memory.trace` and `graph.update_note` were examples here until
        // Task 7 fix rounds 1/2 registered them — see those pin tests.)
        assert_eq!(treatment_of("sessions.set_topic"), None);
        assert_eq!(treatment_of("chat.context_estimate"), None);
        assert_eq!(treatment_of("memory.reembed"), None);
    }

    /// Task 7 fix round 1's own pin.
    #[test]
    fn task_7_fix_round_1_methods_are_registered_and_partition_checked() {
        for m in [
            "memory.trace",
            "memory.delete",
            "graph.node_detail",
            "graph.search",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::PartitionChecked), "{m}");
        }
    }

    /// Task 7 fix round 2's own pin.
    #[test]
    fn task_7_fix_round_2_methods_are_registered_and_partition_checked() {
        for m in [
            "graph.update_note",
            "graph.rename_note",
            "graph.delete_note",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::PartitionChecked), "{m}");
        }
    }

    /// Task 7's own pin — same philosophy as the Task 6 pin above: a
    /// deletion or typo in these `SCOPED_METHODS` entries fails a test by
    /// name, not silently reads as "unenforced".
    #[test]
    fn every_task_7_method_is_registered() {
        for m in [
            "memory.search",
            "memory.listFacts",
            "memory.stats",
            "graph.query",
            "artifacts.list",
            "artifacts.read_text",
            "session.export_html",
            "clarification.resolve",
            "clarification.pending",
            "subagent.tree",
        ] {
            assert!(
                treatment_of(m).is_some(),
                "{m} must be registered in SCOPED_METHODS"
            );
        }
    }

    #[test]
    fn task_7_partition_methods_are_partition_checked() {
        for m in [
            "memory.search",
            "memory.listFacts",
            "memory.stats",
            "graph.query",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::PartitionChecked), "{m}");
        }
    }

    #[test]
    fn task_7_addressed_key_methods_are_key_checked() {
        for m in [
            "artifacts.list",
            "artifacts.read_text",
            "session.export_html",
            "clarification.resolve",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
    }

    #[test]
    fn task_7_list_methods_are_list_filtered() {
        for m in ["clarification.pending", "subagent.tree"] {
            assert_eq!(treatment_of(m), Some(Treatment::ListFiltered), "{m}");
        }
    }

    /// The final review's C1: `agent.run` is `chat.send` with the same
    /// obligation, so it must carry the same `Treatment` — if this ever reads
    /// `None` again, the run-start guard has been dropped.
    #[test]
    fn agent_run_is_key_checked_like_its_chat_send_twin() {
        assert_eq!(treatment_of("agent.run"), Some(Treatment::KeyChecked));
        assert_eq!(treatment_of("agent.run"), treatment_of("chat.send"));
    }

    /// Every `OrgShared` entry — exact or family prefix — must carry a
    /// one-line reason. "Shared by design" and "nobody looked" are
    /// indistinguishable without one.
    #[test]
    fn org_shared_entries_all_carry_reasons() {
        let exact = SCOPED_METHODS
            .iter()
            .filter(|(_, t)| *t == Treatment::OrgShared)
            .map(|(m, _)| *m);
        for m in exact.chain(ORG_SHARED_PREFIXES.iter().copied()) {
            assert!(
                ORG_SHARED_REASONS
                    .iter()
                    .any(|(rm, reason)| *rm == m && !reason.is_empty()),
                "{m} is OrgShared but carries no reason in ORG_SHARED_REASONS"
            );
        }
    }

    /// Final-review C2/C3: the families the parameter-keyed sweep could not
    /// see.
    #[test]
    fn final_review_families_are_registered() {
        assert_eq!(treatment_of("trace.by_runs"), Some(Treatment::KeyChecked));
        assert_eq!(
            treatment_of("group_chat.list"),
            Some(Treatment::ListFiltered)
        );
        for m in [
            "group_chat.continue",
            "group_chat.mention",
            "group_chat.history",
            "group_chat.end",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
        // `group_chat.start` creates; there is no addressed record to check,
        // same ruling as `session.create`.
        assert_eq!(treatment_of("group_chat.start"), None);
    }

    /// The `teams.*` family, tightened 2026-08-06. Curated pin: a deletion or
    /// typo in the block above fails here by name.
    ///
    /// The three deliberate absences are asserted too — an unregistered method
    /// and a method someone forgot are indistinguishable in `treatment_of`, so
    /// the ruling has to be written down somewhere that breaks if reversed.
    #[test]
    fn every_teams_method_is_registered_or_deliberately_absent() {
        for m in ["teams.list", "teams.snapshot.list"] {
            assert_eq!(treatment_of(m), Some(Treatment::ListFiltered), "{m}");
        }
        // `agents.teams` returns the same filtered summaries but is NOT listed:
        // `method_admin.rs` gates the whole `agents.` family to operators, and
        // this table's `every_scoped_method_stays_open_to_members_in_method_admin`
        // requires the two tables to agree. Its result set is still filtered —
        // by the store decorator, which does not care which surface asked.
        assert_eq!(treatment_of("agents.teams"), None);
        for m in [
            "teams.get",
            "teams.rename",
            "teams.disband",
            "teams.delete",
            "teams.usage",
            "teams.chat.send",
            "teams.chat.thread",
            "teams.chat.history",
            "teams.list_tasks",
            "teams.create_task",
            "teams.update_task",
            "teams.list_task_runs",
            "teams.add_task_comment",
            "teams.list_task_comments",
            "teams.list_task_events",
            "teams.task.trace",
            "teams.task.pause",
            "teams.task.resume",
            "teams.task.retry",
            "teams.task.skip",
            "teams.task.journal.get",
            "teams.task.journal.list",
            "teams.workflow.approve_step",
            "teams.workflow.reject_step",
            "teams.acp_member.add",
            "teams.acp_member.remove",
            "teams.acp_member.list",
            "teams.snapshot.create",
            "teams.snapshot.get",
            "teams.snapshot.restore",
            "teams.snapshot.delete",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
        for m in ["teams.create", "teams.list_templates", "teams.chat.cancel"] {
            assert_eq!(
                treatment_of(m),
                None,
                "{m} is deliberately unregistered — see the module doc's teams.* section"
            );
        }
        // The family-wide OrgShared ruling is gone; nothing may inherit it.
        assert_eq!(treatment_of("teams.some_future_rpc"), None);
    }

    /// The `projects.*` family (P2 project rooms). Curated pin: a deletion or
    /// typo in the block above fails here by name.
    ///
    /// The three deliberate absences are asserted too, for the same reason the
    /// `teams.*` pin asserts its own — `treatment_of` cannot tell "ruled a
    /// creation surface" from "nobody looked".
    #[test]
    fn every_projects_method_is_registered_or_deliberately_absent() {
        assert_eq!(
            treatment_of("projects.list"),
            Some(Treatment::ListFiltered),
            "projects.list"
        );
        for m in [
            "projects.get",
            "projects.remove",
            "projects.touch",
            "projects.rename",
            "projects.archive",
            "projects.member.add",
            "projects.member.remove",
            "projects.member.list",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::KeyChecked), "{m}");
        }
        for m in ["projects.create", "projects.add", "projects.create_blank"] {
            assert_eq!(
                treatment_of(m),
                None,
                "{m} is deliberately unregistered — see the module doc's projects.* section"
            );
        }
    }

    /// Final-review I6: the five same-shape siblings Task 7's sweep stopped
    /// short of. `workspace.*` is registered as defense in depth only — see
    /// the module doc and both handlers.
    #[test]
    fn final_review_agent_id_siblings_are_registered() {
        for m in [
            "memory.retrieve_with_trace",
            "memory.list_corrections",
            "dreaming.list_insights",
            "insights.tools",
            "workspace.get",
        ] {
            assert_eq!(treatment_of(m), Some(Treatment::PartitionChecked), "{m}");
        }
        assert_eq!(
            treatment_of("workspace.list"),
            Some(Treatment::ListFiltered)
        );
    }

    /// `trace.list`/`trace.get` are absent from this table on purpose: they
    /// are admin-gated, and the cross-check below would fail if they were
    /// listed. Pin the reason so "missing" can't be read as "forgotten".
    #[test]
    fn admin_gated_trace_siblings_are_deliberately_absent() {
        for m in ["trace.list", "trace.get"] {
            assert_eq!(treatment_of(m), None, "{m}");
            assert!(
                crate::gateway::method_admin::method_requires_admin(m),
                "{m} is absent from SCOPED_METHODS only because it is admin-gated"
            );
        }
    }

    /// Cross-check with `method_admin.rs`: every method THIS table claims
    /// must be a member-daily-open surface there (never admin-gated) — a
    /// method cannot simultaneously be "operator only" and "per-user
    /// filtered for members", and if it moved to `ADMIN_PREFIXES` this
    /// table's claim about it would be stale.
    #[test]
    fn every_scoped_method_stays_open_to_members_in_method_admin() {
        for (method, _) in SCOPED_METHODS {
            assert!(
                !crate::gateway::method_admin::method_requires_admin(method),
                "{method} is claimed by SCOPED_METHODS but method_admin.rs \
                 requires operator privilege for it — the two tables disagree"
            );
        }
    }
}
