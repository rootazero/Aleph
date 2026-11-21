# Severed-wire audit — `src/exec`, `src/executor`, `src/export`

**Date:** 2026-11-21 · **Base:** `main` @ `addb5d17d` · **Worktrees:** `wire-exec`,
`wire-secrets`, `wire-executor`, `wire-export` (branch of the same name, one per
repair slice)

Static review of the execution-approval, builtin-tool-registry and document-export
surfaces for **severed wires**: a declaration, a knob, a handler, a variant or a
doc claim on one side, and nothing (or nothing that can act) on the other. No diff
was supplied — the whole module surface was read.

**Outcome:** 4 repair slices (exec, secret catalogs, executor, export) merged into
`main` as `e2e57c8ad`, `804f778e2`, `b2f6ffa74`, `f414644b7`, plus the guard commit
`a8dc6a59e` and the compile repair `d386c6da0`. **39 of 45 distinct findings
repaired** (12 CUT, 1 CUT narrowed to a test-only affordance, 26 CONNECT — two of
them doc/shape-only corrections); **6 findings reported without a repair** because
they need a product, public-API or placement decision.

---

## 1. Method

| Phase | What happened |
|---|---|
| 1. Scan | 9 read-only `sev-auditor` subagents in parallel, three per slice, one per seam lens: registration/path-naming parity, producer→consumer parity, classifier/priority drift, emit↔subscribe parity, call↔handler parity, stub sweep. |
| 2. Enumerate | Every candidate carries **both** ends (`producer file:line` + `consumer?`), produced by a path-qualified grep plus a read of the surrounding code. |
| 3. Triage | Read-first per candidate → `CONNECT` / `CUT` / `DECIDE`. `jev_evaluate` produced a calibrated second opinion on the disposition of the exec and executor slices (§8). |
| 4. Fix | Surgical edits in one worktree per slice. A symbol is never deleted on the strength of a grep alone. |
| 5. Guard | Three new grep-diff detectors, one per slice (`scripts/{exec,executor,export}_wire_audit.py`), wired into `just check-wiring` next to the sibling audit scripts. |
| 6. Verify | One unified `cargo check -p alephcore -p aleph-panel --all-targets` on the merged `main` after all slices were done. |

**Coverage gap (declared).** The provider's concurrency limit killed several
children mid-run (`429 … Token Plan 速率限制 (2062)`, and later a weekly quota on
`kimi-coding`). The waves were re-run per slice until every lens had produced a
report; no lens was left uncovered, but the **export slice was re-triaged
read-before-write without a Jev batch** (§8).

**Model routing.** Per the operator's instruction, reasoning-heavy work went to a
reasoning model (MiniMax-M3 / k3.1-pro / deepseek-flash) and classification,
filtering and routing went to Jev (§8).

**Repair-worker outcomes.** The executor and export repair workers were **cut off
mid-task** (30-minute subagent ceiling, then a `429` token-plan stop) with their
edits on disk but uncommitted. Both diffs were reviewed line by line, finished by
hand, and committed from the worktree.

---

## 2. Findings and dispositions

`CUT` = the far end does not exist → removed. `CONNECT` = the wire was broken →
reconnected. `DECIDE` = product / public-API / placement call → reported, not
decided by the audit.

The nine auditors raised **53 candidate mentions**; several lenses reported the
same wire under different ids, so the tables below merge them into **45 distinct
findings**.

### `src/exec` — 19 candidates, 15 repaired

| ID | Finding | Disposition |
|---|---|---|
| B-01 | `ExecApprovalManager::record_originator` (`src/exec/manager.rs`) — no production caller. | **CUT** |
| B-02 | `RegisteredApprovalReceiver::try_recv` — no *production* caller, but six `#[cfg(test)]` call sites depend on it. | **CUT, narrowed** — the inherent impl is now `#[cfg(test)]` (`d386c6da0`) |
| B-03 | `ExecApprovalManager::cleanup_expired` (public-in-crate) is test-only; the locked variant has the real caller. | **CUT** |
| B-04 | `ExecApprovalRecord.decision` / `resolved_by` / `resolved_at_ms` are stamped on resolve and never read (the Panel reads neither). | **CUT** — the two manager tests now assert on `deny_reason` / liveness |
| B-05 | `CommandAnalysis::chains` duplicates data the analyzer already holds. | **CUT** |
| B-06 | `CommandSegment::argv` is stored and never consumed after parsing. | **CUT** |
| B-07 | `CommandResolution::raw_executable` has no reader anywhere in `src`. | **CUT** |
| B-08 | `CustomRiskPattern.reason` is configured by the operator but never reaches the denial text, the logs or the audit reasons. | **CONNECT** — `assess_custom_with_reason` carries it through `src/sandbox/security_kernel_hook.rs` |
| B-09 | A **failed** persistent mutation still installed the new in-memory answer, so the cache showed a grant the file never got. | **CONNECT** — `src/sandbox/exec_approval/grants.rs` publishes the cache only on `Ok` |
| B-10 | A non-`NotFound` read error (permissions, I/O) was cached as an **empty** grant registry, i.e. "no grants" instead of "unknown". | **CONNECT** — read errors now propagate; only `NotFound` means empty |
| C-01 | `SecretMasker` misses OpenAI project keys (`sk-proj-…`) and underscore-bearing OpenAI/Anthropic keys that the leak detector does catch. | **CONNECT** — the two catalogs agree now |
| C-02 | Generic password masking and the shell parser disagree on an escaped quote (`password="abc\"secret"`). | **CONNECT** |
| C-03 | The byte-scrub catalog omits fine-grained/new GitHub tokens and Discord bot tokens. | **CONNECT** — added to `SECRET_PATTERN_SOURCES` |
| C-04 | `mcp/redact.rs` documented a no-engine fallback that does not recognise raw API keys. | **CONNECT** — the fallback now runs the real `SecretMasker` |
| C-05 | `ScanFinding.pattern_name` has no production reader (only the detector's own tests). | **DECIDE** — reported, not cut (§4) |
| C-06 | The high-confidence vendor catalog (`VENDOR_SECRET_PATTERNS`) never reached the human-facing `SecretMasker` surfaces. | **CONNECT** — one owner, both consumers |
| EX-A-01 | Cancelling a run strands an already-rendered Panel approval card: the waiter is dropped, the pending entry disappears, but **no terminal event is published**, and the Panel only refetches on `approval.requested/resolved/expired`. | **CONNECT, reported** (§4, §9) |
| EX-A-02 | The Telegram native `resolve_approval` handle is discarded by the only live caller, and the far end never edits the keyboard anyway. | **CUT, reported** (§4) |
| EX-A-03 | The actor-aware native renderer/authorizer has no production entry point; the live delivery path attaches the keyboard unconditionally. | **CUT, reported** (§4) |

### `src/executor` — 19 candidates, 12 distinct, 11 repaired

| ID | Finding | Disposition |
|---|---|---|
| EX-A-01 / C4 | `team_task_control` and `task_exit_journal` are registered and dispatchable but named by **no** predicate list, so `is_builtin_tool_name` says false and a skill's `allowed-tools:` entry is dropped. | **CONNECT** |
| EX-A-02 | Dynamic `td.name.clone()` registrations are invisible to both source censuses (they only see quoted literals). | **CONNECT** — the registration uses a literal name plus a const assertion tying it to the tool's own `NAME` |
| EX-A-03 / C6 | 21 non-catalog tools reached the Policies screen with an **empty description**: the lookup only knew `BUILTIN_TOOL_DEFINITIONS`, while the descriptions lived in a test-only table. | **CONNECT** — one runtime table, one lookup (`builtin_tool_description`) |
| EX-A-04 / C5 | `capability_projection_diagnostics` was named unconditionally but registered only under an env gate, and the comment documenting it was false on two counts. | **CONNECT** — the comment now says what the code does; the tool is accounted for in the group and description tables |
| EX-A-05 / EXB-06 | `create_tool_boxed` is a second construction table whose only callers are tests. | **DECIDE** — reported (§4) |
| EXB-01 | `skill_list.filter` is implemented by the handler but omitted from the runtime schema. | **CONNECT** — the schema is now derived from `ListSkillsArgs` |
| EXB-02 | The diagnostics handler requires `tool_name` / `plane` / `duration_ms` that its schema never declares. | **CONNECT** |
| EXB-03 / C1 | `BuiltinToolConfig.embedder` is an inert DI knob — the embedder actually comes from the memory runtime. | **CUT** |
| EXB-04 / C2 | `BuiltinToolConfig.orientation` claims a registration and dispatch that never happens. | **CUT** |
| EXB-05 / C3 | `BuiltinToolConfig.profile_synthesizer` is never read. | **CUT** |
| C7 | `BuiltinToolRegistry::memory_context_provider_cell()` has zero callers; the deferred injection it exists for never happens. | **CUT** |
| C8 | `set_memory_reflector` / `set_query_filer` have **test-only** callers, and the dispatch branch they feed is unreachable in production (both production construction sites set `memory_runtime: Some(..)`). | **CUT** — the registry keeps the schema registration, the per-call construction stays in the memory runtime |

### `src/export` — 15 candidates, 14 distinct, 13 repaired

| ID | Finding | Disposition |
|---|---|---|
| C-01 | `session.export_html` lands on the Mutate lane, but the Panel sends no idempotency key — the call is refused before the handler runs. | **CONNECT** — the Panel now sends one (pattern copied from the canvas client) |
| C-02 | `artifact_publish`'s doc points the model at `artifacts.list`, a tool the model does not have, and no model-facing surface handed out artifact ids. | **CONNECT** — the harvest result now carries `_media_artifacts` |
| C-03 | `exported_document_bytes` mixes two unit systems (post-base64 artifact bytes vs raw transcript bytes) while the doc claims one. | **CONNECT** — doc corrected |
| B-01 | Backslash URL variants (`\\evil.com`, `/\/evil.com`) bypass the protocol-relative navigation guard. | **CONNECT** |
| B-02 | A colon anywhere in a relative URL was mistaken for a scheme. | **CONNECT** — a scheme must now *look* like one |
| B-03 | `escape_attr` bypassed the shared escaping core it claimed to share. | **CONNECT** — it composes it now |
| B-04 / EC-03 | Remote Markdown images are emitted as `<img src="https://…">` that **both** CSPs forbid, so they never render. | **CONNECT** — a remote image becomes a link (§4) |
| B-05 | The claimed shared Panel/export URL policy is three independent implementations. | **DECIDE** — reported (§4) |
| EC-01 | The served document's CSP `sandbox` disables every non-image attachment row the export emits (`download` attribute). | **CONNECT** — `sandbox allow-downloads` |
| EC-02 | `timestamp: 0` rendered as `1970-01-01` in every exported transcript holding a legacy row (the write side filtered the sentinel, the read side did not). | **CONNECT** — `rfc3339()` renders it as absent; `instant()` keeps the epoch for ordering |
| EC-04 | "The CSP exists once" is false: two literals define the policy and nothing asserted they agreed. | **CONNECT** — one owner (`CONTENT_SECURITY_POLICY`), the gateway derives from it |
| EC-05 | A figure dropped by the inline budget never reached `skipped_figures`, so the model reported figures the document itself marks "not embedded". | **CONNECT** — `collect_artifacts_with_drops` names the dropped figure and why |
| EC-06 | The documented "whole-document" inline ceiling was enforced as an artifact-only ceiling. | **CONNECT (doc)** — the doc now matches the arithmetic (§4) |
| EC-07 | `page::human_size` was exported to `super` with no consumer outside its file, and is a third spelling of a helper the product already has. | **CUT** — made private |

---

## 3. What changed

### `a0ec89090` — exec audit (7 files, +224/−148)

- `src/exec/manager.rs`: removed `record_originator`, `cleanup_expired` (the wrapper),
  and the `decision` / `resolved_by` / `resolved_at_ms` fields of `ExecApprovalRecord`.
- `src/exec/analysis.rs`, `src/exec/parser.rs`: removed `CommandAnalysis::chains`,
  `CommandSegment::argv` and `CommandResolution::raw_executable`.
- `src/exec/kernel.rs` + `src/sandbox/security_kernel_hook.rs`: the configured
  pattern reason now travels with the verdict — it reaches the denial text and the
  audit reason list.
- `src/sandbox/exec_approval/grants.rs`: a read error is no longer cached as an
  empty registry, and a failed write no longer publishes a cache the file never got.
- `tests/exec_approval_resolve_loop.rs`: adapted to the new analysis constructor.

### `64c5f1278` — secret-catalog audit (5 files, +151/−21)

- `src/exec/secret_patterns.rs`: one owner for the vendor catalog — the masker and
  the leak detector now agree on what an OpenAI/Anthropic key looks like; the
  password rule handles an escaped quote.
- `src/exec/masker.rs`, `src/mcp/redact.rs`: the MCP redaction fallback runs the
  real masker instead of a token pattern.
- `src/secrets/leak_detector.rs`, `src/sandbox/scrub.rs`: GitHub fine-grained and
  Discord bot families added to the byte-scrub catalog.

### `87a39e1b5` — executor audit (17 files, +402/−294)

- `src/executor/mod.rs`: the two team tools are named in `REGISTRY_ONLY_TOOL_NAMES`,
  and `builtin_tool_description` is the single description lookup.
- `src/executor/builtin_registry/definitions.rs`: the registry-only description
  table became runtime data (it was `#[cfg(test)]`), the three missing
  descriptions were added, and the two byte ratchets were re-based on **measured**
  lengths (366 B + 368 B + 1 007 B = +1 741 B for the description ceiling;
  `skill_list` 47 B → 281 B for the schema ceiling).
- `src/executor/builtin_registry/builder/constructor/coord_team_tools.rs`: the team
  tools are registered under a literal name (the source census only sees quoted
  literals) with a compile-time assertion tying the literal to the tool's own
  `NAME` const.
- `src/executor/builtin_registry/config.rs`: the three inert `BuiltinToolConfig`
  knobs are gone (an in-repo public-API change — §4).
- `src/executor/builtin_registry/registry/{inherent,struct_def}.rs`,
  `tool_registry_impl.rs`: the never-injected memory-context cell and the
  unreachable `memory_reflect` fallback are gone; the arm keeps
  `caller_memory_partition`, which the source guard requires.
- `src/executor/builtin_registry/builder/core_tools.rs`: the `skill_list` schema is
  derived from `ListSkillsArgs`, so `filter` cannot drift again.
- `src/builtin_tools/capability_projection_diagnostics.rs`: the schema now declares
  the fields the handler requires.
- `src/gateway/handlers/agents.rs`: the tools schema uses the one lookup.

### `3cfc3b1dc` — export audit (10 files, +944/−148)

- `src/export/markdown.rs`: a stateful sanitising fold — backslash variants,
  scheme-shapedness, `escape_attr` composing the shared core, and a remote image
  becoming a link (with the alt text preserved).
- `src/export/mod.rs` + `src/gateway/server/artifact_route.rs`: one CSP owner; the
  served document derives from it and adds `sandbox allow-downloads`.
- `src/export/collect.rs` + `src/builtin_tools/artifact_publish.rs`: a dropped
  artifact is reported to the model by id and reason.
- `src/export/page.rs`: `human_size` is private.
- `src/gateway/session_store/types.rs`: the `timestamp: 0` sentinel renders as
  absent.
- `src/tools/scoped/artifact_harvest.rs` + `src/tools/scoped/dispatch.rs`: the
  artifact ids reach the model (`_media_artifacts`).
- `interfaces/webchat/src/api/artifacts.rs`: `session.export_html` carries an
  idempotency key.

### `a8dc6a59e` — guards (4 files, +956/−1)

Three new detectors wired into `just check-wiring`:

| Script | Checks | What it holds |
|---|---|---|
| `scripts/exec_wire_audit.py` | 11 | the five CUT checks (record fields, `record_originator`, the receiver poll, the expiry wrapper, the analysis fields), the custom-reason live path, the grant-store error handling, the shared secret catalog, the leak-detector families, the MCP redaction, and the `note_manage` embedder doc (DARK) |
| `scripts/executor_wire_audit.py` | 11 | the 3 config knobs, the memory-context cell, the reflector setters, the description table ↔ name list, the description lookup, the literal team-tool registration, the diagnostics group, the `skill_list` filter, the diagnostics schema, the partition composition, and `create_tool_boxed` (kept, as a DARK marker) |
| `scripts/export_wire_audit.py` | 10 | `human_size` private, one CSP owner, the derivation, `allow-downloads`, the zero timestamp, dropped figures reported, remote images as links, the RPC idempotency key, artifact ids reaching the model, and the deliberate test-only CSP duplicate in `src/security/headers.rs` |

### `d386c6da0` — compile repair (3 files, +63/−18)

The first unified check failed on three error groups **caused by the audit's own
cuts** (§7). Fixed: the approval-receiver poll restored as `#[cfg(test)]`, the two
`&str` const assertions rewritten as a byte comparison (`str`'s `PartialEq` is not
const-stable), the two manager tests re-pointed from the removed `decision` field
to `deny_reason`, and the exec guard's `try_recv` check turned from "must be absent"
into "must stay test-only".

---

## 4. Operator decisions taken

These were choices the audit was not entitled to make silently; each is recorded
with the reasoning.

1. **`EX-A-01` (Panel card stranded on cancellation) — reported, not repaired.**
   The fix is a terminal publication on the cancellation path, which touches the
   approval-event plane across three wait drivers and the Panel subscription. Doing
   it halfway would be worse than leaving it visible. Reported with the exact
   mechanism.
2. **`EX-A-02` / `EX-A-03` (Telegram native approval) — CUT, reported, not
   applied.** Both ends live outside the audited crates
   (`crates/aleph-slot-api`), and the far end of `resolve_approval` never edits the
   keyboard, so *connecting* the existing method would not satisfy the trait
   contract. Cutting the API is a public-API change for another crate.
3. **`C-05` (`ScanFinding.pattern_name`) — DECIDE.** The field is on a public
   result struct; removing it is a visibility decision, not a dead-code cut.
4. **`EX-A-05` / `EXB-06` (`create_tool_boxed`) — DECIDE.** A second construction
   table is either a deliberate public API or a candidate for removal; both are
   product calls. It is marked DARK in the guard so the decision stays visible.
5. **`B-05` (three URL-policy implementations) — DECIDE.** Unifying the Panel's
   and the export's URL policy touches the Panel rendering surface.
6. **`EC-06` — the doc was corrected, not the ceiling.** The stated ceiling and
   the enforced ceiling disagreed; the honest minimum is to say what the code does
   (artifact bytes only), because the 50 MiB store ceiling is enforced by the
   *caller* on the rendered length. Raising or lowering the inline ceiling is a
   product decision.
7. **`EC-03` / `export-B-04` — a remote image becomes a link.** Widening
   `img-src` to `https:` would let every rendered document phone home. Turning the
   image into a link keeps the document self-contained and offline-readable.
8. **`B-02` — the poll was narrowed, not deleted.** Six test call sites depend on
   it; a `#[cfg(test)]` inherent impl keeps the tests honest without leaving an
   unused public surface.
9. **`BuiltinToolConfig` field removal is an in-repo public-API change.**
   `BuiltinToolConfig` is re-exported from `src/executor/mod.rs`; the three inert
   fields were removed because they had no reader anywhere. `alephcore` is not
   published to a registry, so the change is contained — recorded here so it is not
   a surprise in the next audit.
10. **The ratchet ceilings were re-based on measured lengths**, not on the
    worker's arithmetic. The worker's numbers were right, but two of them had to be
    re-derived independently (a Rust-literal-aware measurement for the
    descriptions, a standalone `schemars` project for the schema) before being
    accepted.

---

## 5. Guards installed

`just check-wiring` now runs 11 detectors (was 8):

```
check-domain-wiring  check-discovery-wiring  check-event-wiring
check-exec-wiring    check-executor-wiring    check-export-wiring
```

Each new script is a plain `python3` grep-diff detector in the shape of the
sibling `scripts/domain_wire_audit.py`: it reads the production surface (test-only
items are stripped with the repo's own `scripts/wiring_strip.py`), asserts the
invariants, prints `PASS: <module> wire invariants hold` or
`FAIL: N wire invariant(s) broken` plus one line per breach, and exits non-zero.

Every check was **mutation-tested**: the wire was broken on purpose, the script
had to fail, the change was restored. Ten mutations were run across the three
scripts (a second CSP literal, a removed `allow-downloads`, a reverted
`rfc3339`, a deleted team-tool description, a widened `human_size`, a re-added
`pub decision:`, a removed diagnostics group entry, a re-added
`set_memory_reflector`, a replaced description lookup, a replaced
`collect_artifacts_with_drops`) — all ten were caught.

---

## 7. Verification

- **`cargo check -p alephcore -p aleph-panel --all-targets -j 4` on the merged
  `main`** — the unified check the operator asked for, run after all slices were
  repaired and merged. The **first** attempt **FAILED** with three error groups,
  all of them caused by the audit's own cuts: `try_recv` removed while six test
  call sites still used it, two `&str` const assertions (not const-stable), and
  two tests reading the removed `ExecApprovalRecord.decision`. Those were fixed in
  `d386c6da0` and the check was re-run: **PASS** —
  `Finished dev profile in 15m 14s`, **0 errors, 0 warnings**.
- `python3 scripts/exec_wire_audit.py` → `PASS: exec wire invariants hold`
- `python3 scripts/executor_wire_audit.py` → `PASS: executor wire invariants hold`
- `python3 scripts/export_wire_audit.py` → `PASS: export wire invariants hold`
- The three sibling guards of the previous slice (`discovery`, `domain`, `event`)
  still PASS.
- `git diff --check` is clean; the touched files add no new rustfmt deviation
  (the tree is not rustfmt-clean at HEAD — 174 unrelated files would be
  rewritten — so only the touched files were formatted).

**One pre-existing failure, not caused by this work.** `just check-wiring` as a
whole still fails at `scripts/config_wiring_audit.py`:

```
FAIL: 1 NEW inert policy type(s) beyond the triaged baseline:
  - TimeWindowConfig src/config/types/policies/capability_constraints.rs:24
```

This is the known false positive recorded in memory: the type *is* value-consumed
(`compile_constraints` → `Constraint::time_window`), but the guard only greps by
type name. `scripts/config_wiring_audit.py` was not touched by this slice.

---

## 8. Where Jev was used

Jev (`jev_evaluate`) was used **only for classification and routing**, never for
reading code or writing fixes:

1. **Disposition classification** — the exec and executor candidates were
   submitted to Jev as `choice` questions ("`CUT` / `CONNECT` / `DECIDE`", plus
   the candidate's own evidence summary) and Jev returned a calibrated probability
   distribution. **31 candidate dispositions** were scored this way, in four
   batches (B-01…B-09, B-10 + C-01…C-06, TOOL-A-01…TOOL-B-04, EXEC-A-01…TOOL-B-06).
   One call was rejected for malformed JSON and re-issued.
2. **The export slice was not scored.** Its candidates were triaged
   read-before-write against the code, because the provider's quota was exhausted
   by the time that slice ran. This is a real asymmetry in the method, recorded
   here rather than hidden.

Jev's verdicts were **not** taken as gospel. The overrides are the cases where
the review went against a Jev probability:

| Candidate | Jev | Final | Why it was overridden |
|---|---|---|---|
| `B-02` (`try_recv`) | CUT 0.91 | CUT, narrowed | Right disposition, wrong evidence: the method has six *test* callers, so the cut broke the build. It was cut from production and kept as `#[cfg(test)]`. |
| `B-04` (resolution metadata) | CUT 0.24 | CUT | Jev's confidence was low; the review confirmed no production reader and that the Panel never reads the fields. |
| `B-05` / `B-06` (`chains`, `argv`) | DECIDE 0.18 / 0.22 | CUT | Jev read the risk of removing a public-ish shape; neither field was ever serialised nor read, so there was nothing to decide. |
| `B-07` (`raw_executable`) | CUT 0.28 | CUT | Same reasoning; Jev's number was low but the direction agreed. |
| `EXEC-A-02` (Telegram resolver) | CONNECT 0.77 | CUT (reported) | The far end never edits the keyboard, so connecting the existing method would not fulfil the trait contract. |
| `TOOL-B-04` (`orientation`) | DECIDE 0.24 | CUT | Its doc promises a registration that does not exist — a self-evident fact in the code, not an ambiguity. |
| `C-05` (`ScanFinding.pattern_name`) | CUT 0.14 | DECIDE | Jev read it as dead code; it sits on a public result struct, so removing it is a visibility decision. |

Candidates where Jev was confident and the review agreed: `B-01` (CUT 0.98),
`B-03` (CUT 0.97), `B-08` (CONNECT 0.92), `B-09` (CONNECT 0.96), `B-10`
(CONNECT 0.96), `C-01` (CONNECT 0.96), `C-02` (CONNECT 0.97), `C-03` (CONNECT
0.94), `C-04` (CONNECT 0.97), `C-06` (CONNECT 0.74), `TOOL-A-01` (CONNECT 0.98),
`TOOL-A-02` (CONNECT 0.92), `TOOL-A-03` (CONNECT 0.96), `TOOL-A-04` (CONNECT
0.92), `TOOL-A-05` (DECIDE 1.0), `TOOL-B-01` (CONNECT 0.98), `TOOL-B-02` (CONNECT
1.0), `TOOL-B-03` (CUT 0.66), `TOOL-B-05` (CUT 0.88), `TOOL-B-06` (DECIDE 0.98),
`EXEC-A-01` (CONNECT 0.98), `EXEC-A-03` (CUT 0.92).

---

## 9. What was NOT done

- **No test suite was run.** The brief asked for one unified `cargo check` after
  all slices; the new regression tests are type-checked but **never executed**.
  They need a `cargo test -p alephcore --lib` run (≈20 min) before anyone trusts
  them.
- **No `cargo fmt --all`** — the tree is not rustfmt-clean at HEAD (174 unrelated
  files would be rewritten); only the touched files were formatted.
- **6 findings were reported, not repaired** (§2): `EX-A-01` (the stranded Panel
  card), `EX-A-02` and `EX-A-03` (the Telegram native approval path, across
  crates), `C-05` (`ScanFinding.pattern_name`), `EX-A-05`/`EXB-06`
  (`create_tool_boxed`), `B-05` (the three URL-policy implementations).
- **`EX-A-01` in particular is a live user-visible defect** and is *not* fixed by
  this slice: cancelling a run still leaves the approval card on screen until an
  unrelated event triggers a refetch.
- **The guards are not in CI.** No wire-audit script is in CI today; they are
  wired into `just check-wiring` only. Wiring that family into CI is a separate
  decision.
- **`just check-wiring` still fails** on the pre-existing `config_wire_audit`
  false positive (§7). Not touched, out of scope.
- **No `origin/main` push and no PR** — the slices are merged into local `main`
  only; publishing is the operator's call.
- **The four worktrees** (`wire-exec`, `wire-secrets`, `wire-executor`,
  `wire-export`) still exist on disk.
