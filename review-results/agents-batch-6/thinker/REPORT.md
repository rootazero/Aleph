# Severed-Wire Audit — `src/thinker`

- **Batch:** agents-batch-6
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)
- **Scope:** 69 `.rs` files, 20,781 LOC (read-only)

## Summary

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 0 |
| medium   | 2 |
| low      | 2 |
| **total**| **4** |

| Decision | Count |
|----------|-------|
| CONNECT  | 0 |
| CUT      | 2 |
| DECIDE   | 2 |

This module is unusually well-guarded: `prompt_contract.rs` (`reachable_layers`,
`scaffold_bytes_ratchet`, `dynamic_tail_bytes_ratchet`, `no_sentence_is_stated_twice`,
`stable_prefix_*`), the `default_layers()` registration block, and a long history of
documented CUTs have already removed the classic severed wires. The four findings below
survive the guards because they live at seams the guards do not cover: a config struct
read by nothing, a struct field the layer reads but nothing writes, a derived method
that lies about its own contract, and write-only metadata.

---

## Findings

### [MEDIUM] src/thinker/memory_context_provider/mod.rs:17 — `MemoryContextConfig.max_facts` and `similarity_threshold` are dead config fields

- **Category:** architecture
- **Decision:** CUT

**Description:** `MemoryContextConfig` declares three fields. Only `max_output_chars`
is ever read (`memory.rs:77` derives the memory-injection token budget from it). Grep
across the whole tree finds **zero** reads of `config.max_facts` or
`config.similarity_threshold`, and the single production construction site
(`handlers/memory.rs:392`) calls `MemoryContextConfig::default()`, so both fields are
hard-wired to `5` / `0.3` and never influence retrieval. The real retrieval knobs live
on the ripple assembler's own config (`max_facts_per_hop`, `similarity_threshold`),
not here. This is an inert-config (form 3) severed wire.

**Suggested fix:** Delete `max_facts` and `similarity_threshold` (shrinking the struct
to `max_output_chars` or replacing it with a const). Note that `max_output_chars` is
read but also never configured non-default; fold that into the same cleanup.

---

### [MEDIUM] src/thinker/context.rs:394 — Sub-agent "Run id" prompt line is severed end-to-end

- **Category:** architecture
- **Decision:** DECIDE

**Description:** `OperatingEnvelopeLayer` renders `- Run id: \`{id}\`` from
`ResolvedContext.run_id` (`operating_envelope.rs:102-105`), and the field is documented
as "sub-agent dispatch ONLY … set by sub-agent dispatchers". But no production code ever
assigns `ResolvedContext.run_id` — grep for `run_id` in `harness_bridge/prompt_build.rs`
returns nothing, and the only `Some(...)` assignment in the tree is a unit test
(`operating_envelope.rs:394`). The twin producer field `TurnEnvelope.run_id`
(`context.rs:183`) is likewise always `None`: the sole `TurnEnvelope` construction site
(`run_loop/inner.rs:1362`) hardcodes `run_id: None`, and `prompt_build.rs` bridges
`envelope.parent → resolved_context.envelope_parent` but has no corresponding
`envelope.run_id → resolved_context.run_id` bridge. Net effect: the feature looks
implemented (two documented fields + a rendering layer + a test) but renders nothing in
any production prompt — the "两端都在，连线断" shape.

**Suggested fix:** This is a product call, not a silent CONNECT. To revive it you must
mint a run id in sub-agent dispatch, set `TurnEnvelope.run_id`, and add the missing
bridge in `prompt_build.rs` (mirroring the `envelope_parent` line). To cut it you delete
the two fields, the layer's `run_id` bullet, and the test. Present the trade-off rather
than picking.

---

### [LOW] src/thinker/context.rs:254 — `TurnEnvelope::is_empty()` omits `memory_mode`

- **Category:** logic
- **Decision:** DECIDE

**Description:** `is_empty()` checks `exec_tier`, `session_mode`, `cwd`, `serving_model`,
`parent`, `run_id`, and `response_language` — but not `memory_mode` (`context.rs:207`),
the one remaining `Option` field. A `TurnEnvelope` carrying only
`memory_mode: Some(Off)` reports `is_empty() == true` even though it changes the prompt
(memory is suppressed and the envelope feeds `memory_muted` via `injects_memory()`).
The doc comment explicitly promises "True iff every field is `None`". Today the method
has no production caller (only `context.rs` tests exercise it), so the impact is latent,
but it is a landmine for the fast-path the doc advertises.

**Suggested fix:** One line — add `&& self.memory_mode.is_none()`. Alternatively delete
the method if the advertised fast-path never materializes.

---

### [LOW] src/thinker/identity_files.rs:69 — `IdentityFile.truncated` / `original_size` are write-only

- **Category:** quality
- **Decision:** CUT

**Description:** `IdentityFile.truncated` (line 69) and `original_size` (line 71) are
populated in `IdentityFiles::load` (`identity_files.rs:136-165`) and asserted in
`identity_files.rs`'s own unit tests, but no layer, serializer, or production path reads
them. Grep for `.truncated` / `original_size` outside `identity_files.rs` finds only
test fixtures constructing the struct and unrelated types (sandbox/tool outputs). No
prompt layer surfaces a "this file was truncated" notice, and `IdentityFile` is not
serialized (`IdentityFileStatus` is the separate UI type). These are the module's own
documented anti-pattern in reverse: data whose only reader is a unit test.

**Suggested fix:** CUT the two fields and their write-site bookkeeping. If a
truncation notice is genuinely wanted, render it in `IdentityFilesLayer` instead of
carrying dead metadata.

---

## Negative / not-done

- Not audited: non-`.rs` assets (`soul_archetypes/templates/*.md`, prompt fixtures).
- Cross-module producers/consumers were verified by grep but not re-read in full
  (e.g. `harness_bridge::prompt_build`, `subagent_spawner`, `metering.rs`, `compactor.rs`).
- The phase-5 grep-diff guard (`scripts/wiring_audit.py`) was not installed or wired into
  CI; the existing `reachable_layers` test is the module's structural guard and was
  verified to still cover the 37 registered layers.
- No CONNECT findings: every layer gate and `PromptConfig` field traced to a production
  writer or a `CONDITIONALLY_SILENT` entry, so no missing wire was load-bearing enough to
  reconnect.
