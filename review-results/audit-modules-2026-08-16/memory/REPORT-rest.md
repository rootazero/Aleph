# Memory Module — Audit Report (non-notes/non-store)

**Scope:** `src/memory/` excluding `src/memory/notes/` and `src/memory/store/`.
**Date:** 2026-08-16
**Files reviewed:** 158
**Total LOC:** ~49,446
**Lenses:** seam (severed-wire), logic, architecture
**Methodology:** Read-only. Cross-checked producer/consumer pairs against
`graphify` semantics, grep-diff of `DEFINED − CONSUMED` for tags/handlers,
and the recent fix history (`git log -25 -- src/memory/`).

> Recent fixes (intentionally NOT re-reported):
> `325125bcd` assembler hydrate-budget-at-loop, `060400ec9` feedback floor
> wiring, `eeb49ecd7` project_scope read_dir errors, `c13040aa2` 7-day
> constant, `9257baf47` bounded JoinSet for pre-compress, `dffc065ba` reader
> fan-out narrowing, `a38fa1e4a` partition alignment, `1821999d1` fetch_limit
> truncation, `732d27cf3` AggregateRoot sever, `84abb9569` repeat_n,
> `dfa53eb71` scratchpad durable list, `aa4f63b27` prune_orphan batching,
> `2b1813429` `..` title rejection.

---

## Critical / High findings

### [High] src/memory/streaming_scrubber.rs:54-61 — `<memory>` fence from `render_markdown_v1` is NOT in `DISCARD_TAG_PAIRS`
**Category:** seam
**Confidence:** High
**Description:** `MessageAssembler::new()` (gateway/message_assembly/assembler.rs:42)
builds its scrubber with `DISCARD_TAG_PAIRS`, which is the const at
`src/memory/streaming_scrubber.rs:54-61`:

```
&[
    ("<memory-context>", "</memory-context>"),
    ("<think>", "</think>"),
    ("<thinking>", "</thinking>"),
    ("<thought>", "</thought>"),
    ("<antthinking>", "</antthinking>"),
    ("<completion-check>", "</completion-check>"),
]
```

But the production render style is `MarkdownV1` (default for
`RenderStyle` in `src/memory/assembler/render.rs:13-20`), which emits
`<memory>...</memory>` as the outer fence and slot tags `<user_profile>`,
`<session_recent>`, `<relevant_notes>`, `<feedback>`, `<raw_fragments>`,
`<nudges>` as inner fences (see `render_markdown_v1` at render.rs:36-62
and the `slot_tag` constants at render.rs:111-119). The module's own
docstring (streaming_scrubber.rs:38-42) even says "The default tag set is
`<memory>` (Aleph's `render_markdown_v1` envelope)" — but that is only
true for the `Default` impl, not for the production `MessageAssembler`.

The terminal-answer sanitize (`src/gateway/reply_emitter/sanitize.rs:64`)
strips `memory-context` and the think/* family in `OTHER_STRIP_TAGS`, but
NOT `<memory>` nor any of the MarkdownV1 inner slot tags. So if the model
echoes the framing back verbatim, the user sees raw `<memory>...` and
`<user_profile>...` markers in their reply.

The `assembler/render.rs:163` comment acknowledges the issue
("Escape closing-tag sequences in user content so a recalled memory cannot
prematurely close the surrounding `<memory>` or slot fences") but
`render_item_markdown` only escapes the inner content; the slot tags
themselves remain open to model echo.

**Suggested fix:** Add the MarkdownV1 outer fence and the six inner slot
fences to `DISCARD_TAG_PAIRS` (or restructure so the scrubber consumes
the live `RenderStyle` instead of a hardcoded set), and extend
`OTHER_STRIP_TAGS` in `sanitize.rs` to match. The existing
`discard_tag_pairs_memory_context_matches_canonical_fence` test
(streaming_scrubber.rs:430-439) already pins one fence to its source of
truth; extend that pattern to all of them.

---

### [High] src/memory/embedding_manager.rs:25-30 / src/memory/dreaming/* — Dreaming stages are the unwired side of the embedding-queue producer
**Category:** seam
**Confidence:** High
**Description:** The embedding manager's `pending` queue has exactly one
producer and one consumer pair in production: `notes/ingest/ingestor/batch.rs:230`
pushes via `em.push_pending(...)` and `:241` flushes via
`em.flush_pending(...)`, all inside the compound ingestor's batch tail.
The struct's own docstring at `embedding_manager.rs:25-26` claims
"Producers (ingest tail, dream stage tails) are wired in B5.2" but no
dream stage calls `push_pending`.

10+ dream stages modify notes and never re-enqueue for embedding:
- `note_decay::stage.rs` → `write_note_raw` (note_decay.rs:460)
- `note_consolidate::merge` → `write_note` (note_consolidate.rs:571)
- `note_synthesis::apply` → `write_note` (note_synthesis.rs:120)
- `note_lint::apply` (note_lint.rs:778)
- `note_review::apply` → `write_note` (note_review.rs:141, 183)
- `note_weave::apply` → `append_to_note` (note_weave.rs:498)
- `mention_weave::apply` → `write_note` (mention_weave.rs:273, 287)
- `goal_lessons_promote::apply` → `append_to_note` (goal_lessons_promote.rs:100)
- the file `mod.rs:2498` in dreaming/mod.rs also writes notes
- the file watcher (`src/memory/notes/watcher.rs`, out of scope but
  interacts) also does not push to the queue

After every dream cycle, those notes' vectors are stale or missing
until an operator manually invokes the `memory.reembed` RPC
(reembed.rs:55, "Designed to be triggered manually via RPC, not at
startup"). Recall degrades silently — the same vector is now computed
from a different text but the LLM is not told.

**Suggested fix:** Either (a) wire a background flusher task in
`embedding_manager` that drains the queue on a tick (the "wired in B5.2"
the docstring promises) and have every stage tail call `push_pending`;
or (b) remove the queue entirely and re-embed on the read path. Option
(a) matches the docstring's promise and the existing API.

---

## Medium findings

### [Medium] src/memory/session_compactor/mod.rs:137, summary_source.rs:135, session_search_summary/end_hook.rs — three `extract_depth` copies with different semantics
**Category:** logic
**Confidence:** High
**Description:** Three functions parse `aleph://session/{sid}/d{depth}/{seq}`:
1. `mod.rs:137` — `path.split("/d").nth(1)` (first `/d`)
2. `summary_source.rs:135` — `split('/').find_map(|s| s.strip_prefix('d').and_then(|r| r.parse()))` (first d-prefixed segment)
3. `end_hook.rs:34-44` — `path.rfind("/d")` (LAST `/d`)

The third is the most defensible (explicitly so: "Use the LAST `/d` so
a session id containing `/d` can never shadow the real depth segment").
The first is the one the live `prepare_history` path uses for depth-based
sorting (prepare_history.rs:104-105) and depth labelling (line 151) — so
a session id containing `/d` is misordered relative to the depth axis
it relies on for token-budget injection. The summary_source copy is
production code too: called from `SessionSummarySource::reassemble_with_summaries`
(summary_source.rs:71-72) to pick the highest-depth summary first.

**Suggested fix:** Promote the `rfind`/`+ 2 + split_once` version
(end_hook.rs:34-44) to `session_compactor::extract_depth` and delete
both copies. Its docstring already articulates the correct invariant.

---

### [Medium] src/memory/embedding_provider.rs:115-150 — `call_api` has no retry, no backoff, no circuit breaker
**Category:** logic
**Confidence:** High
**Description:** `RemoteEmbeddingProvider::call_api` performs a single
HTTP request. Any non-2xx response, network error, or non-JSON body
becomes an `AlephError::config(...)` that propagates back to
`EmbeddingManager::flush_pending` (embedding_manager.rs:222-232), where
the behaviour is "log warn, drop the batch, next reembed_all will catch
up".

A single 503 from the remote provider — OpenAI rate-limit, SiliconFlow
transient outage, Mistral auth hiccup — drops an entire ingest batch's
embeddings. The `embed_batch` retries internally only on the wave
shape (4 concurrent batches), not on the wire. There is no `tokio::time::sleep`
and no `Retry-After` parsing. Combined with the severed-wire finding
above, a transient failure on the dream-tail path means vectors stay
absent permanently (reembed is manual).

**Suggested fix:** Wrap the `request.send().await` in a small retry
loop with exponential backoff, respecting `Retry-After` headers. Move
the per-batch flush_pending to mark items for retry (push back to queue
front, not drop) on transient errors. Reserve `AlephError::config` for
configuration problems (4xx) and use a transient error class for 5xx
that the caller can distinguish.

---

### [Medium] src/memory/embedding_signature.rs:21 — `provider_id` part of signature uses the user's config id, not the preset
**Category:** seam
**Confidence:** Medium
**Description:** `embedding_signature(p, m, d)` is
`format!("{p}:{m}:{d}")`. `provider_signature` calls
`provider.provider_id()` which returns `config.id` (e.g. `"openai"`,
`"openai-prod"`, `"openai-dev"`). The preset has its own id
(`EmbeddingPreset::OpenAi` / `Ollama` etc., see resolver.rs:25-37) that
is not part of the signature.

Two `EmbeddingProviderConfig` rows with different `id` fields but the
same preset, model, and dimension will produce *different* signatures,
but the resolver's `EmbeddingLocality` map (resolver.rs:60-65) treats
them as identical. Result: a swap from `openai-prod` to `openai-dev`
on the same backend forces a full `reembed_all` even though the actual
weights are unchanged. Conversely, two providers with id `openai` and
`openai` (loaded from two different machines) write the same signature
even if they actually point at different weights — the compare test at
embedding_signature.rs:67-72 acknowledges this ("two providers serving
the 'same' model name are not guaranteed to produce identical weights")
but doesn't separate the cases.

**Suggested fix:** Sign the preset as well, e.g.
`format!("{preset}:{p}:{m}:{d}")`. The unit cost is one extra
`provider_id` lookup at record time; the savings is avoiding a full
reembed when the operator just renames a provider.

---

### [Medium] src/memory/insights.rs — no live consumer for `aggregate_tool_failures` outside its dream-stage call site
**Category:** seam
**Confidence:** Medium
**Description:** `insights.rs` is re-exported at `mod.rs:91` as
`aggregate_tool_usage`, `ToolBreakdown`, `ToolUsageReport`. The
tool-signal-sink module's module doc (tool_signal_sink.rs:18-21) says
there are "exactly TWO readers" of `RawMemorySource::ToolInvocation`
rows: `aggregate_tool_usage` (admin RPC) and `aggregate_tool_failures`
(nightly `tool_failure_distill` stage). Confirmed by grep:
- `aggregate_tool_usage` is called from `gateway/handlers/...` for the
  admin RPC.
- `aggregate_tool_failures` is called only from
  `dreaming/stages/tool_failure_distill.rs` (one site, 1 import).

So far this is not a severed wire — both ends exist. The risk is that
the gateway RPC and the dream stage each independently do
`raw_memory_store.get_unprocessed_raw_memories` queries that
double-read the same rows (no cooperative watermark). If one consumer
marks rows processed and the other then sees an empty set, the
`tool_failure_distill` will silently stop seeing any failures.

**Suggested fix:** Audit whether `mark_raw_as_processed` is called on
both paths and whether it covers `ToolInvocation` rows. If it does,
`tool_failure_distill` will be starved of new rows; if it doesn't,
both readers see the same rows forever. Add a `processed_by` watermark
column or a per-consumer cursor.

---

### [Medium] src/memory/extensions/first_party.rs:16 / envelope_relevance_floor — registered with floor=0.0
**Category:** architecture
**Confidence:** High
**Description:** `agent_init/mod.rs:319-322` registers the first-party
extension:
```
reg.register(Arc::new(EnvelopeRelevanceFloorExtension::new(0.0)));
```
The comment at agent_init/mod.rs:317-318 says "Registers the POC
first-party extension (no-op at floor=0.0) to prove end-to-end
plumbing. Real floor could be plumbed from config later."

The extension's `floor=0.0` means it never filters anything — the
envelope always passes through. This is a no-op producer/consumer pair
in the live path; the registration consumes a slot in the registry and
the per-call dispatch (insert_helper.rs:11-40) adds latency (extension
chain walk + CaptureDecision evaluation) for zero observable behaviour.
The comment is explicit that the real floor is unwired.

**Suggested fix:** Either (a) plumb the floor from
`[memory.orientation]` or `[memory.extensions] envelope_floor`
config and drop the `0.0` default; or (b) gate the registration behind
`if config.envelope_floor > 0.0` so the empty chain is only assembled
when there is something to do. Per R10 (YAGNI), option (b) is the
pragmatic cut until a real value is needed.

---

### [Medium] src/memory/proptest_enums.rs:111-130 — exhaustive uniqueness test misses 4 of 15 variants
**Category:** logic
**Confidence:** High
**Description:** `arb_note_type` (lines 25-42) generates 15 variants:
`Preference, Plan, Learning, Project, Personal, Tool, Other, SubagentRun,
SubagentSession, SubagentCheckpoint, SubagentTranscript, Lesson, Skill,
Reference, Transcript`. The deterministic `note_type_as_str_values_are_unique`
test (lines 111-130) lists only 11:
```
Preference, Plan, Learning, Project, Personal, Tool, Other, SubagentRun,
SubagentSession, SubagentCheckpoint, SubagentTranscript
```
Missing: `Lesson, Skill, Reference, Transcript`.

The proptest version covers all 15, so the test passes today. But the
drift shape is: a future contributor adds variant `Foo`, updates
`arb_note_type` (one line), forgets to update the deterministic list
(line 13, "it just needs to be updated too" — easy to miss), and the
deterministic test will silently keep passing. If two new variants
collide on `as_str()` the proptest will catch it, but the deterministic
test is the one whose name says "all variants" — and it lies.

**Suggested fix:** Replace the hand-listed `all_variants` with a
`const fn` that enumerates the enum, or replace the deterministic
test with a derived `strum::EnumIter` walk, or guard with
`assert_eq!(all_variants.len(), 15, "add new variants to this list too")`
so a missing entry is a loud failure.

---

## Low findings

### [Low] src/memory/embedding_manager.rs:25-26 — docstring claims a background flush task that does not exist
**Category:** architecture
**Confidence:** High
**Description:** The `EmbeddingManager::pending` field doc reads:
> Producers push via `push_pending`; the background flush task drains
> in batches via `flush_pending` (wired in B5.2).

The background flush task is not present. The only consumer of
`flush_pending` is `notes/ingest/ingestor/batch.rs:241` (the ingest
tail), invoked synchronously after a push loop. There is no
`tokio::spawn` in `EmbeddingManager`, no field referencing a task
handle, and no test for a flush happening without a push. The "B5.2"
marker in the comment matches the B5.2 marker at
`agent_init/mod.rs:185-186` ("Long-lived embedding manager (B5.2): hoisted
so the compound ingestor's embedding queue has a real
producer/consumer instead of the manager being constructed locally and
dropped."), which is a different concern.

**Suggested fix:** Either implement the background task the docstring
promises, or rewrite the docstring to describe the actual
producer/consumer (push in stage tails, flush at the ingest tail — the
next-iteration appenders never reach it). This finding is downstream
of the High finding above; whichever path is taken there, the
docstring must follow.

---

### [Low] src/memory/dreaming/strategy.rs:58 — strategy file has only 58 lines, most are re-exports
**Category:** architecture
**Confidence:** Medium
**Description:** `src/memory/dreaming/strategy.rs` is 58 lines (per
`wc -l`), and grep finds only `DreamStrategy` enum + `selector.rs`
dispatch. The actual list of stages per strategy lives in
`dreaming/mod.rs::from_strategy` (~100 lines of `vec![Box::new(...)]`
construction). The strategy file is the right place for it, not
the orchestrator — putting it there would shorten `from_strategy` and
make the three strategies diffable.

**Suggested fix:** Move the `vec![Box::new(...)]` lists into
`strategy.rs` as `impl DreamStrategy { fn stage_list(&self, cfg, policy) -> Vec<Box<dyn DreamStage>> }`
and have `from_strategy` call through. Mechanical, low risk, improves
the diff signal between the three strategies.

---

### [Low] src/memory/note_retrieval/relation_surface.rs — read but rarely produces; check against active caller set
**Category:** seam
**Confidence:** Low
**Description:** `note_retrieval` has 5 submodules. The `relation_surface`
path is queried for every retrieval but the `retrieve_multi_agent` path
shadows it in the common case (assembler/gather.rs:127-137). For
non-multi-agent retrievals, the `relation_surface` surface is
exercised but the `Candidate::id` it produces is a relative path that
the rerank prompt re-uses. Worth a code-grep to confirm there is no
empty list path that the LLM never sees.

**Suggested fix:** Run a grep-diff of `relation_surface::` callers vs
`relation_surface::*` definitions; flag any function whose only caller
is a test. Out-of-scope for confidence >80% but worth a follow-up
pass.

---

### [Low] src/memory/ripple/{mod,task,config}.rs — `RippleConfig::default()` and `RippleTask::default()` divergence
**Category:** architecture
**Confidence:** Low
**Description:** `RippleConfig` is small (3 fields) and the integration
test at `integration_tests/mod.rs:28-39` constructs one with non-default
values that match the production field defaults. The signature
`ripple::RippleConfig` is also re-exported via `mod.rs:97`. The
`RippleTask` struct in `task.rs` is the task description, separate
from the config. The wire between them (`RippleTask::config`?) is
worth a check — not enough evidence to flag at confidence >80%, but
the audit template for this lens asks for a grep-diff:
`DEFINED = RippleConfig fields` − `CONSUMED = every read of those
fields`. From a quick scan the read sites appear to cover every field,
but the `RippleTask` default in `task.rs` is not clearly tested.

**Suggested fix:** Add a one-line test for `RippleTask::default()`
shape, mirroring the existing `RippleConfig` test.

---

## Notes (no finding, but worth recording)

- `src/memory/session_resume/reader.rs` and `writer.rs` are exemplary
  — the partition/scope_id wire is consistent (writer stamps via
  `snapshot_partition`, reader filters on it), retention is bucketed
  the same way reads are filtered, and legacy un-partitioned files
  fail closed (don't adopt the base partition, which would be
  indistinguishable from a leak). The `W2 leak` test at
  reader.rs:222-256 documents the exact failure mode this prevents.
- `src/memory/assembler/feedback_floor.rs::load_many` is the right
  pattern — it joins the floor across every partition
  (`session_read_ids`) and applies `FLOOR_CAP` once after merging.
  The comment at lines 64-81 documents the original bug.
- `src/memory/dreaming/mod.rs::from_strategy` is well-structured, the
  `GLOBAL_ONLY_STAGES` carve-out is documented at length
  (lines 318-355), and `tool_failure_distill`'s partition-aware fix
  is correctly noted as a 2026-08-09 correction.
- `src/memory/streaming_scrubber.rs` itself is well-engineered — the
  state machine is sound, the UTF-8 boundary handling is correct
  (max_partial_suffix_ascii_ci walks the buf backwards, but the
  comment at line 79 documents why this is byte-safe given the
  ASCII-only tag constraint), and the test at line 415-440 pins the
  canonical fence. The only issue is which fences the const at line
  54 names.
- `src/memory/loom_concurrency.rs` is a 5-test loom model for
  singleton-init, lock race, atomic timestamp, counter accuracy, and
  provider hot-swap. The models are 2-3 threads each, which is loom's
  expected scope. No findings.
- `src/memory/explain.rs` is a 51-line type definition module whose
  `FactExplanation` is fed by `MemoryTimeTraveler::explain_fact`
  (events/traveler.rs, wired). Single reader, single writer, no issue.
- `src/memory/namespace.rs` was correctly cleaned up (the old
  `Guest`/`Shared` variants removed, the misleading module doc
  replaced with a "this is not an isolation layer" warning pointing
  to `project_scope.rs` and `gateway::visibility`). Exemplary
  follow-through on a previous audit decision.
- `src/memory/project_scope.rs` is exemplary: every derivation
  (scoped_agent_id, read_scope_ids, scoped_or_base, session_write_id,
  session_read_ids, profile_floor_id, list_note_corpora,
  list_scoped_agent_ids, partition_is_shared_room) has a docstring
  explaining the invariant it enforces and a test pinning it. The
  failure modes the doc strings name (W2 leak, org vs personal,
  base vs scoped) are exactly the modes the prior audit caught.
