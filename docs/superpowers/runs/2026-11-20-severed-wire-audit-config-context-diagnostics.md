# Severed-wire audit — `src/config`, `src/context`, `src/diagnostics`

**Date:** 2026-11-20 · **Base:** `main` @ `924cd180b` · **Branch:** `wire-audit-fix` (worktree `Aleph-wire-audit`)

Static review of three modules for **severed wires**: a declaration, a knob, a
handler, a variant or a doc claim on one side, and nothing (or nothing that can
act) on the other. No diff was supplied — the whole module surface was read.

---

## 1. Method

| Phase | What happened |
|---|---|
| 1. Scan | 8 read-only `sev-auditor` subagents in parallel, one per seam family: C1 config field→reader parity, C2 config machinery, C3 config drift/stubs/doc claims, CT1 fabric/protocol frames, CT2 compaction pipeline, CT3 budget/retrieval/file-ops, D1 diagnostic check registration, D2 diagnostic consumers. |
| 2. Enumerate | Every candidate carries **both** ends (`producer file:line` + `consumer?`). |
| 3. Triage | Read-first per candidate → `CONNECT` / `CUT` / `DECIDE`. Each candidate was re-verified by path-qualified grep before any cut. `jev_evaluate` produced a calibrated second opinion (see §8). |
| 4. Fix | Surgical edits in the worktree. A symbol is never deleted on the strength of a grep alone. |
| 5. Guard | `scripts/config_context_diagnostics_wire_audit.py` (grep-diff detector) + two new in-tree guards (`guide_doc_drift.rs`, `every_check_module_is_attached_to_an_engine_face`). |

**Tooling notes.** `scripts/wiring_strip.py` is the shared production-text
filter (`strip_tests`) — it removes only `cfg(test)` / `cfg(feature="test-helpers")`
items and is literal-aware. The mechanical sweep that the subagents' greps
replaced (`/tmp/sev-audit/mech_audit.py`) built a word→file index once instead of
running a regex per symbol over every file; the first version timed out at 600 s.

**Coverage gap (declared).** Three C1 attempts died on provider quotas
(kimi 429 quota, minimax 429 token-plan, openai-codex `gpt-5.6-sol` unsupported),
so **`src/config/types/**` field→reader parity was covered by hand**, not by a
subagent — see §5.

---

## 2. Findings and dispositions

`CUT` = the far end does not exist → removed. `NARROW` = the symbol is real but
its visibility was wider than its consumers → visibility tightened.
`CONNECT` = the wire was broken → reconnected. `DECIDE` = product call, routed to
the operator (§4) and not decided by the audit.

### 2.1 `src/diagnostics`

| id | wire | form | disposition |
|---|---|---|---|
| D1-01 | `HealthCheck::title()` — required trait method, 28 impls, zero production consumers (the renderer prints the finding's own `title`) | name drift | **CUT** — trait method + 28 impls removed |
| D1-02 | `DiagnosticEngine::run()` — public wrapper; all 3 production call sites use `run_with_filter` | no consumer | **CUT** — 6 test call sites rewritten |
| D1-03 | `DiagnosticEngine::new()` doc claimed "(used by tests)" — it is the **only** production constructor | doc-claim drift | **CUT** — doc corrected |
| D1-04 | test fixtures used the id `core/stale-lock`; the real id is `core/instance-lock` → `--skip core/stale-lock` silently skipped nothing | client ghost | **CONNECT** — 3 fixture ids corrected |
| D1-05 | `PluginsActivatedCheck::from_records()` — deliberate `Source::Live`/`Source::Records` seam, reachable only from tests | visibility | **NARROW** — `pub` → `pub(crate)` |
| D1-06 | `impl Default for DiscordChannelHealthCheck` shadowed `from_default_path()` | stub far-end | **CUT** |
| D1-07 | no test asserted that every implemented check module is attached to an engine face | registration parity | **CONNECT** — new guard test |
| D1-08 | `providers/connectivity` registers only when config **and** vault exist → a degraded boot shows no row at all (siblings report UNKNOWN) | registration parity | **DECIDE → report-only** |
| D1-09 | `diagnostics.run` fix/only/skip params: the only in-repo client sends none | client ghost | **DECIDE → report-only** |
| D1-10 | `media/codecs` is counted in `checks_run` on non-Linux but yields no row | stub far-end | **DECIDE → report-only** |
| D1-11 | handler doc claimed the RPC serves "the aleph doctor CLI, **the Panel**" — no Panel caller exists | doc-claim drift | **CUT** — doc corrected |
| D1-12 | `aleph doctor --json` emits no severity/repaired/timedOut; `aleph-server doctor` exits 1 on any problem, `aleph doctor` exits 2 only on required failures | shape drift | **DECIDE → report-only** |
| D2-01 | `report_to_checks` did `from_value(...).ok().unwrap_or_default()` → a malformed daemon report became **"N checks passed"** | false-green | **CUT→CONNECT** — now fails closed |
| D2-02 | cache-health dropped malformed alarm rows with `continue` → **"No cache degradation alarms"** | false-green | **CUT→CONNECT** — now reports the unreadable rows |
| D2-03 | the presence guard only covers `checks/`, and its walker can omit unreadable files | classifier parity | **DECIDE → report-only** |
| D2-06 | `Finding.tags` bypassed the redaction chokepoint (`redacted()` masked title/detail/hint/repair, not tags) | false-green | **CONNECT** (operator decision) — tags now go through `redact_secrets`, + test |
| D2-07 | `TAG_CHECK_TIMEOUT` / `TAG_CHECK_PANIC` have no tag-specific consumer | no consumer | **DECIDE → report-only** |
| D3-01 | `browser_ignored_fields` discarded the config-parse error (`Err(_)`) — the module's own `presence_discipline` guard (already in the tree, already red before this audit) names the pattern: a discarded error cannot be told apart from an absence | fail-closed spelling | **CONNECT** — the error is bound and travels with the finding via `check::unknown_finding` |

### 2.2 `src/config`

| id | wire | form | disposition |
|---|---|---|---|
| C2-01 | 5 routing-rule mutators on `Config` (`add_rule_at_top`, `remove_rule`, `move_rule`, `get_rule`, `rule_count`) — zero callers | no consumer | **CUT** |
| C2-02 | `ReloadImpact::Inert` + `INERT_SECTIONS` (empty) — the verdict is documented but no section is inert | inert config | **KEPT** (operator decision: keep the inert concept) |
| C2-03 | `ConfigUiHints` — always empty, its only transport discards the payload | no consumer | **KEPT** — a 2026-08-16 decision to hold the wire shape stable |
| C2-04 | `merge_builtin_rules` — an intentional no-op hook (AI-first mode) | stub far-end | **KEPT** — the no-op *is* the design |
| C2-05 | `Config::validate()` doc claimed two checks it does not perform (provider references, API keys) | doc-claim drift | **CUT** — doc corrected |
| C2-06 | `general.browser` — the browser module says `Live`, the shared classifier says `Restart` | classifier parity | **DECIDE → report-only** |
| C2-07 | the live-apply guard test covered `LIVE_SECTIONS` but not `LIVE_SUBSECTIONS` | registration parity | **CONNECT** — guard extended |
| C2-08 | `Config::new()` — one test caller | no consumer | **CUT** — test uses `Config::default()` |
| C2-09 | `config_handlers::handle_get` — a whole `config.get` handler that is **never registered** (the RPC is bound to `handle_get_full_config`) | registration parity | **CUT** — handler + 2 tests removed |
| C2-10 | `ReloadMode::should_hot_reload` — no production caller; the module already records it as a deliberate half-finished selector | reserved | **KEPT** — `reload_mode` in `config.toml` does nothing today (§4) |
| C2-11 | a test calls `config.set_tool_permissions`, an RPC the server does not have | client ghost | **KEPT** — the list is a *name* list, not a call |
| C2-12 | hot-reload doc claimed `config.get` serves the watcher snapshot | doc-claim drift | **CUT** — doc corrected |
| C3-01 | the Panel's ACP toggle sent `key`; the server reads `section` | name drift | **CONNECT** — Panel now sends `section` |
| C3-03 | `behavior.typing_speed` severed in **both** directions (the Panel required a field the server retired) | client ghost | **CUT** (operator decision) — the Panel knob is gone; the reveal rate is a Panel-local constant |
| C3-04 | `docs/guides/general.md` documented 26 dead keys, and `policies` scalars were written under the wrong table | doc-claim drift | **CUT** — guides corrected (see §6) |
| C3-05 | "auto-reloads via fswatch, no restart needed" — false | doc-claim drift | **CUT** — corrected |
| C3-06 | `reload_impact` travelled as `{kind,hint}`, as a bare verdict string, and as a per-field map — **three shapes under one key** | shape drift | **CONNECT** (operator decision) — one shape now: `{"kind", "hint"}` (+ optional `by_field`) |
| C3-07 | the model-facing docs described an "inert" reload verdict the server can never produce | doc-claim drift | **KEPT** with C2-02 |
| C3-08 | `AgentModelRef::model_str` — no production consumer | no consumer | **CUT** |
| C3-09 | the Panel's Memory page rendered a `vector_db` knob that cannot exist | client ghost | **CUT** (operator decision) |
| C3-13 | `typewriter.rs` cited a config function that does not exist | doc-claim drift | **CUT** |

### 2.3 `src/context`

| id | wire | form | disposition |
|---|---|---|---|
| CT1-01 | `ContextOperation::{Describe,Status,Shutdown}` cannot complete a legal exchange | no consumer | **DECIDE → report-only** |
| CT1-02 | `ContextOperation::{Restore,Search}` handled but never emitted | reserved | **DECIDE → report-only** (CONTEXT_PLUGINS.md reserves them for Core-owned flows) |
| CT1-03 | `ContextLifecycleEvent::SessionShutdown` never emitted | reserved | **DECIDE → report-only** (reserved host entry point) |
| CT1-04 | `ContextContinuation` accepted, then discarded | no consumer | **KEPT — first triaged as CUT, then corrected**: the extension adapter deserializes the guest's JSON into `ContextTransformResult` and the host re-serializes it, so the field is part of the plugin-facing canonical JSON (see §9) |
| CT1-05 | the plugin result's `strategy` / `quality` / `degraded` are read by the runtime only as `response.output` | no consumer | **KEPT — same correction**: `strategy` and `quality` are asserted by the extension adapter's canonical-JSON round trip; the runtime's *policy* not to use them is report-only |
| CT1-06 | the wider namespace/effect grants (`Project`, `CrossSession`, network egress) have no transport path | no consumer | **DECIDE → report-only** (cutting plugin-contract enum variants is a manifest break) |
| CT1-07 | `ContextPersistencePolicy` validated but never consumed | no consumer | **DECIDE → report-only** |
| CT2-01 | `FoldStrategy::{AutoLlm,AutoDeterministic,SessionMemoryReuse}` never constructed | reserved | **KEPT** — durable-record vocabulary; unknown words must still decode |
| CT2-02 | `FoldTrigger::{PressureEscalation,Preventive,ModelTool}` never connected | reserved | **KEPT** — same reason |
| CT2-03 | manual compaction bypasses the deterministic preservation/carrier splice → `/compact` retires source events without re-attaching preserved turns or plan/file/image carriers | far-end bypass | **DECIDE → report-only** (operator: do not touch the compaction core) |
| CT2-04 | session split bypasses the same splice → a pressure-driven split drops the newest image and the plan/file ledgers | far-end bypass | **DECIDE → report-only** |
| CT2-05 | session split bypasses the registered fabric/plugin summary dispatch | far-end bypass | **DECIDE → report-only** |
| CT3-01 | `NativePreflightRegistry::len()` / `is_empty()` — zero callers | no consumer | **CUT** |
| CT3-02 | `ContextFabricBuilder::native_preflight_registry` — zero callers | no consumer | **CUT** |
| CT3-03 | `native_preflight_stage` — test-only stage-injection seam | visibility | **NARROW** — `#[cfg(test)]` |
| CT3-04 | `ContextBudget::last_pressure()` — test-only accessor | visibility | **NARROW** — `#[cfg(test)] pub(crate)` |
| CT3-05 | the preflight log carried only `tokens_freed`; `owner` / `degraded` / `skipped_observers` were computed and dropped | no consumer | **CONNECT** — the log now carries the attribution |
| CT3-06 | `ContentIndex::{open_in_memory,search,len}` — test-only wrappers | visibility | **NARROW** — `#[cfg(test)]` |
| CT3-07 | the `"path"` → `"file_path"` precedence was implemented twice (`file_ops` and the cheap pass) — they agree today | name drift | **CONNECT** — one owner (`file_ops::raw_path`) |
| CT3-08 | two preflight stages shared the name `historical_image_stripping` → the log could not say which ran | name drift | **CONNECT** — the user stage is now `historical_user_image_stripping` |
| CT3-09 | 4 `ContextBudgetConfig` fields have no TOML key (`token_estimate_ratio`, `fresh_tail_count`, `circuit_breaker_max`, `max_splits`) | inert config | **DECIDE → report-only** |
| CT3-10 | `publish_message_tokens_as` is called by the runner but not by the subagent spawner → `context_breakdown` returns `messages: None` for subagent sessions | far-end bypass | **DECIDE → report-only** |
| CT3-11 | the tool-result retrieval index is deleted at process exit → no cross-restart retrieval | reserved | **DECIDE → report-only** |

### 2.4 `src/config/types/**` (hand-covered, see §1)

| id | wire | form | disposition |
|---|---|---|---|
| C1-01 | `SmartRecallConfig.score_threshold` — the Phase 2 trigger **counts** results (`min_primary_results`); the score was never compared | no consumer | **CUT** + tolerated-and-retired |
| C1-02 | `GenerationConfig.smart_routing_enabled` — no smart-routing mechanism exists | no consumer | **CUT** + tolerated-and-retired |
| C1-03 | `AgentDefaults.bootstrap_max_chars` / `bootstrap_total_max_chars` — deliberately **rejected** at parse time, but `docs/guides/agents.md` documented one of them | doc-claim drift | **KEPT** (the rejection is the design) — the **guide** was corrected |

The subagent's own variant sweep reported 7 "unreachable" enum variants; all 7
were re-verified as **live** (`EmbeddingPreset::Jina/Mistral`,
`MemoryInjectionMode::Hybrid`, `ResourceUnit::ActiveCalls/PtySessions`,
`CustomPiiSeverity::Medium`) — the sweep searched for `.Variant` while Rust
reaches variants as `::Variant`. Recorded here because a guard must not inherit
that mistake.

---

## 3. What changed

- **78 files** changed (+577 / −879) and **3 files** added (947 lines): the audit
  report, the guard script and the guide-drift guard test. The `skills`
  submodule pointer moves to the already-pushed `Aleph-skills` commit
  `4f4cc61`.
- `src/diagnostics`: the `title()` trait method and its 28 impls; `DiagnosticEngine::run`;
  the `Default` impl of the Discord health check; fail-closed alarm/doctor-report
  handling; `Finding.tags` redaction; the bound config-parse error in
  `browser_ignored_fields`; the new registration-parity guard test.
- `src/config`: 5 routing-rule mutators; `Config::new()`; the unregistered
  `config.get` handler; 2 dead config fields; the `reload_impact` wire shape
  (`ReloadImpact::wire` + `ReloadImpact::strongest`); the live-apply guard
  extension; doc corrections.
- `src/context`: 4 visibility narrowings; 3 dead-symbol cuts; the preflight
  attribution log; the single-sourced path precedence; the stage-name fix.
- `src/gateway`: `config.get` producers now speak one `reload_impact` shape.
- `interfaces/webchat` (Panel): the `typing_speed` knob, the `TypingSpeedSection`
  component, the `vector_db` Memory knob, the `cps` signal and the ACP `key`→
  `section` fix; 2 locale keys restored/removed.
- `docs/guides/*`: 6 guides corrected against the live schema.
- `skills/self/SKILL.md`: the retired `smart_recall.score_threshold` knob.
- New guards: `scripts/config_context_diagnostics_wire_audit.py`,
  `src/config/tests/guide_doc_drift.rs`.
## 4. Operator decisions taken (2026-11-20)

1. **Compaction core** (CT2-03/04/05) — *report only, no code change*.
2. **Reload surface** (C2-06 / C3-06 / C3-07) — *unify the shape, keep `inert`*.
   The three shapes became one `{"kind","hint"}`; `browser_config` adds
   `by_field`; the inert verdict stays.
3. **Diagnostics** (D2-06 tags, D2-07 tag consumers, D1-12 exit codes) — *fix the
   redaction chokepoint, report the rest*.
4. **Panel drift** (C3-03 `typing_speed`, C3-09 `vector_db`, C3-12 guard scope) —
   *delete the Panel's dead switches*; guard coverage stays report-only.

Open product questions left for a human (not decided here): D1-08, D1-09, D1-10,
D1-12, D2-03, D2-07, C2-06, C2-10 (`reload_mode` in `config.toml` does nothing
today), C2-11, C3-11, C3-12, CT1-01, CT1-02, CT1-03, CT1-06, CT1-07, CT2-03,
CT2-04, CT2-05, CT3-09, CT3-10, CT3-11.

## 5. Guards installed

| guard | what it detects |
|---|---|
| `scripts/config_context_diagnostics_wire_audit.py` | 20 CUT invariants (the removed symbols stay removed), 10 LIVE invariants (the reconnected wires stay connected), 7 RESERVED invariants (the deliberately dark wires stay dark). |
| `src/config/tests/guide_doc_drift.rs` | every deployed guide's section/key must exist in the live config schema; the header shape (array-of-tables vs table) must match. Self-defence counters: ≥8 guides, ≥15 sections, ≥40 keys must actually be scanned. |
| `src/diagnostics/mod.rs::every_check_module_is_attached_to_an_engine_face` | every `pub mod` under `checks/` must be constructed on some engine face — the D1-07 registration-parity gap. |

The guide guard already paid for itself on the day it was written: it found the
`policies` scalars written under the wrong TOML table and the `cron`
`max_concurrent_jobs` key that the server never had.

## 6. Guide drift fixed

`docs/guides/general.md` (26 dead keys + the `[policies]` table shape),
`docs/guides/cron.md` (`max_concurrent_jobs` → `max_concurrent_agents`,
`history_retention_days` added), `docs/guides/agents.md`
(`bootstrap_max_chars` removed — the server rejects it), plus
`docs/guides/{overview,providers,generation}.md` corrections.

## 7. Verification

- `scripts/config_context_diagnostics_wire_audit.py` → **PASS**.
- One unified, memory-controlled build round (4 jobs, shared 194 GB target dir,
  `lld`):
  - `cargo check -p alephcore -p aleph-panel --all-targets -j 4` → **exit 0**,
    zero warnings in `alephcore`.
  - `cargo check -p aleph-panel --target wasm32-unknown-unknown -j 4` → **exit 0**
    (3 pre-existing `aleph-panel` warnings, untouched by this work:
    `method epoch is never used`, `variant Acquire is never constructed`,
    `methods acquire_pty_topic* are never used`).
  - `cargo check -p aleph-cli --all-targets -j 4` → **exit 0**.
- Tests: `cargo test -p aleph-cli` → **252 passed, 0 failed**;
  `cargo test -p alephcore --lib` for the three guards
  (`every_check_module_is_attached_to_an_engine_face`,
  `every_deployed_guide_section_and_key_exists_in_the_config_schema`,
  `redacted_masks_every_free_text_field`) → **3 passed, 0 failed**.
- Every cut was re-verified by a path-qualified grep before the edit; the
  mechanical "no production consumer" sweep was re-run after the first audit
  round and showed that **all 91** of its "dead" symbols were in fact alive
  (consumed inside their own module, via a `serde(default = "…")` attribute, or
  as a field type) — a reminder that `pub` in a `pub` module can be consumed
  through the module path.
- The two new guards each found a defect in **themselves** on their first run
  (a `checks::` path prefix split by the word scanner; an array-of-tables
  element resolved to the row instead of the row's field) — both fixed before
  the final run.

## 8. Where Jev was used

`jev_evaluate` was used **only for classification**, never for reading code,
causal reasoning, or editing:

| batch | questions | use |
|---|---|---|
| D1 | 12 | disposition per candidate (CONNECT / CUT / DECIDE) |
| C2 | 13 | disposition per candidate |
| C3 | 13 | disposition per candidate |
| CT3 | 11 | disposition per candidate |

The audit's own verdict won in every case where the two disagreed; Jev's value
was calibration and coverage, not authority. **The one override**: D1-05 —
Jev said `CONNECT` (0.20), the audit says `NARROW`, because
`PluginsActivatedCheck::from_records` is a deliberate `Source::Records` seam
that a test needs; the defect was its *visibility*, not its existence.

Read-first triage (the candidate text, both ends of the wire, the actual
consumer) was done by hand; the *reasons* recorded in §2 come from that read,
not from the classifier.

## 9. What was NOT done

- **A cut that was reverted.** CT1-04/CT1-05 (`ContextContinuation`,
  `ContextQuality` and the `strategy`/`quality`/`degraded` fields of the
  plugin-facing `ContextTransformResult`) were first triaged as CUT and the
  fields were removed. The extension adapter's canonical-JSON round trip
  (`src/extension/context_adapter.rs:154` deserializes the guest's JSON into
  the struct, the host re-serializes it) then failed: `strategy` and `quality`
  are part of the contract every plugin sees. The cut was reverted, the guard
  now pins the two fields as LIVE, and the finding is re-triaged as a **policy**
  question — the runtime does not use the plugin's declared strategy/quality
  when it chooses a budget — which is reported, not fixed.
- **A pre-existing test failure that this round fixed.** The subset run over
  `config` / `diagnostics` / `context` reported
  `diagnostics::checks::presence_discipline::no_check_answers_a_stat_error_with_absence`
  failing on `src/diagnostics/checks/browser_ignored_fields.rs`
  (`Err(_) => {`). That construct is in `HEAD` (this audit had only removed the
  `title()` impl from that file) and the check is a live, unbaselined guard that
  was already red before this work. It is now green (D3-01): the error is bound
  and travels with the finding.
- **No compaction-core change** (CT2-03/04/05) — operator decision, reported only.
- **No change** to the `diagnostics.run` fix/only/skip surface, the doctor exit
  codes, the timeout/panic tag consumers, the `providers/connectivity`
  registration, or `media/codecs` on non-Linux — reported only.
- **No change** to the plugin contract's `ContextNamespace` / `ContextPersistencePolicy`
  variants, `ContextOperation::{Restore,Search}` or `ContextLifecycleEvent::SessionShutdown`
  — removing them is a manifest-level break, not a cleanup.
- **No change** to `ReloadMode::should_hot_reload` / `reload_mode`, `ConfigUiHints`,
  `merge_builtin_rules`, `FoldStrategy` / `FoldTrigger` vocabulary, or the
  `bootstrap_max_chars` rejection — each is a deliberate design decision.
- **No fix** for the still-open `review-results/config-context-batch` DECIDE list
  (D1..D21), notably the legacy `ToolsConfig` / `McpConfig` surface (D4) — it
  needs a product sign-off, not an audit.
- **No new runtime test** for the compaction/split carrier splices, and no
  `cargo test` run over the whole suite — only the touched modules
  (`config` / `diagnostics` / `context` / `cli`) were run.
- **The C1 scope was not audited by a subagent** — the three attempts died on
  provider quotas, so it was hand-covered; a second pair of eyes has not seen it.
- The `skills/self/SKILL.md` correction went to the **`Aleph-skills` submodule**
  (commit `4f4cc61`, pushed), which the superproject now references — a
  cross-repo change that follows the repo's own convention for retired config
  keys.
