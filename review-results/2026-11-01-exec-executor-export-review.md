# Rust Review Report — `src/exec`, `src/executor`, `src/export`

**Modules**: `src/exec/` (15 files, ~6.5k LOC), `src/executor/` (22 files, ~14.6k LOC),
`src/export/` (6 files, ~1.2k LOC) — ~22.3k LOC in scope
**Date**: 2026-10-31 → 2026-11-01
**Branch**: `main` (fixes committed directly, no PR)
**Mode**: static review, no diff scope (full-module review)

## Method

1. **Machine baseline** — the 2026-10-29 full-workspace `rust-doctor` scan
   (`.review/rust-doctor-full.json`) was reused as the machine baseline. The three
   modules are byte-identical to that scan (last change: `src/exec` 2026-10-28,
   `src/executor` 2026-10-25, `src/export` 2026-10-18), so the baseline is valid
   for them. It reported **100 diagnostics** in scope — `src/exec` 54,
   `src/executor` 37, `src/export` 9 — all structural (`oversized_unit`,
   `complex_function`, `duplicate_function_body`, `near_duplicate_function_body`)
   or clippy lint hits (`indexing_slicing`, `expect_used`, `unwrap_used`,
   `string_slice`, `rc_buffer`, two unreasoned `allow` attributes). The
   `rust-doctor` CLI itself is not installed on this machine and was not fetched
   from the network; the cached scan is the baseline.
2. **Human/agent review** — **11 parallel reviewer batches**, one per file group,
   each reading the real source and grepping the whole workspace for every claim.
   7 of the 11 first-wave runs were lost to provider 429s; the fix was to make
   the delegates write their report to a file **incrementally** (read + append per
   finding) and reply with a 5-line summary, and to re-run the two batches that
   still lost their output on a different provider model. 62 findings, each with
   `file:line` evidence, a before/after and a confidence.
3. **Triage** — every candidate was routed through **Jev**: a calibrated
   severity re-rating (`choice` over P0/P1/P2/P3) and a routing decision
   ("safe to apply in this pass?" — `noul`). Findings below 0.5 on the routing
   axis were deferred with a reason; a P0/P1 finding whose fix is behaviour-
   preserving was applied even below the threshold, with the reason recorded.
4. **Fix** — applied in **six isolated git worktrees** (one writer each), then
   merged into `main` and committed there.
5. **Verification** — a single unified `cargo check -p alephcore --all-targets
   -j 1` after the whole set was in place, with the parallelism and the linker
   held down to keep the build inside the machine's memory budget (the
   `--all-targets` unit-test target alone peaks at ~15 GB of RSS on this host).

**Jev was used for**: severity re-rating of every candidate finding; the
"safe to apply in this pass" routing for every candidate; the defer decision
with its reason. **Jev was not used for root-cause analysis, for reading code,
or for any code decision** — those went to the MiniMax-M3 reviewers and to the
final human review. The Jev endpoint returned HTTP 451 (region) for the last
three candidates (`src/exec/manager.rs`), which were adjudicated by the
orchestrator against the same policy and are marked as such below.

## Summary

| Disposition | Count |
|---|---|
| Fixed (merged into `main`) | ~60 |
| Deferred (owner / product / public-API decision) | ~18 |
| Rejected after review (machine findings re-checked by the reviewer) | 8 machine findings |

## Fixed

### `src/exec` — the security boundary

- **[P1] The module doc denied the on-disk allowlist that `AllowAlways`
  creates.** `AllowOnce`/`AllowSession` live in memory; `AllowAlways` is
  persisted to `~/.aleph/approval-grants.json` and survives a restart. An
  operator reading the header concluded that no standing grant could outlive
  the process — and never looked for the file to revoke one.
- **[P1] A parser-rejected command tripped a `debug_assert!` on a live path.**
  The parser refuses redirection, subshells, background jobs and newlines, so
  `echo hi > out.txt` reaches `ExecApprovalManager::create` with
  `analysis.ok == false`; the `debug_assert` panicked in every debug/dev/test
  build (eleven existing tests had never passed). The card is still raised and
  the human still decides — the failed analysis is now logged, not fatal.
- **[P1] The approval `reason` rode out unredacted and unbounded.** The
  `ApprovalAction` struct promises to carry the *redacted* summary and never
  the raw arguments; the sibling `reason` (authored by a gate rule or a plugin
  hook's `PermissionDecision::Ask`) went onto the card payload and into the
  plain-text fallback untouched. It is now masked and capped once, in the
  module that owns the card; the audit identity still keys on the raw reason so
  the ledger does not move.
- **[P1] A prompt persisted for durable retry was reported as "never
  delivered".** The channel registry persists a definitely-not-delivered message
  for background retry *and still returns the error*; the bridge treated the
  error as a loss and retired the pending entry, so the human's later tap
  resolved nothing. The bridge now asks the registry for the reported
  disposition.
- **[P1] The `ApprovalRequested` mirror was published before delivery and the
  failure paths published no terminal frame**, so every mirrored surface (Panel
  bell, R5 banner) kept a parked card forever.
- **[P1] The redaction rules could not see a JSON key.** The key-adjacent rules
  wanted the separator right after the key name, so the closing quote of a JSON
  key sat in the way: `{"password":"…"}`, `{"api_key":"…"}`, `{"token":"…"}` and
  `{"aws_secret_access_key":"…"}` were never redacted. Those rules also rebuilt
  the payload, so a redacted JSON document was no longer a document — the key,
  the separator, its spacing and the quotes are now captured and re-emitted.
- **[P1] The URL-userinfo class ran across whitespace and stopped at the first
  `@`**: it ate the port and the path of a credential-free URL, and left the
  tail of a password that contains `@` in clear.
- **[P1] The token/authorization value alphabet excluded the base64
  characters**, so `token=abc+def/…` and `Authorization: Basic <base64>`
  survived untouched.
- **[P1] The PEM rule needed its END marker**, so a diff hunk carrying only the
  header and the first key lines shipped the key body in clear — the reachable
  case, because `mask_file_change` masks the hunk lines.
- **[P1] The operator's replacement was used as a regex template** although the
  config documents it as plain text, so a `$` in it was expanded away.
- **[P2] A dropped `custom_blocked` rule was invisible**: the skip count the
  kernel's own doc asked the factory to surface could not leave `from_config`,
  so a rule that never compiled never fired. The drops are now reported in one
  aggregated warning.
- **[P2] `RiskLevel::Danger` was documented as "requires explicit approval"**
  while its only producer and consumer let the command run with a warning.
- **[P2] An empty `allowed_decisions` set rendered a card with no buttons** —
  not even "Deny". The boundary now normalises it to the narrowest answerable
  set, which can only narrow a grant.
- **[P2] `session_listings` snapshots were never removed on a successful
  resolve**, so the map grew one `Vec` per session that ever hit an ambiguous
  listing — and nothing sweeps it.
- **[P2] The session-grant cascade stamped the answered card's clamped
  decision onto its siblings** without re-clamping to each sibling's own
  offered tiers, so a sibling's record could state a tier its card never
  offered. Only the outcome layer neutralised it — two independent mechanisms
  that had to stay true forever.
- **[P3] The audit hook's `tokio::spawn` was guarded by audit-log presence, not
  by runtime presence**, so a future sync caller with the audit log installed
  would panic with "there is no reactor running" inside a resolve path.
- **[P3]** `full_set` is documented as the legacy wire backfill set, not a live
  decision set; the `approve:<id>:<decision>` vocabulary is derived once
  instead of five times; the unreachable checks and the comments that
  contradicted the code were corrected; `CommandSegment::executable()` (no
  production caller) was removed; `CommandAnalysis::not_a_command()` is used
  instead of a hand-rolled re-derivation.

### `src/executor` — the tool registry

- **[P1] The memory read side derived its partition from the base id
  `"default"`** while the write side used the real agent id, so the documented
  fail-open path read a partition the write side never wrote. The base id is
  now derived once.
- **[P1] The optional tools were gated on a different dependency than the one
  their handlers need**: `flag_user_correction`/`governance_metrics` (raw
  memory store), `local_voice` (the Config handle) and `session_search` (the
  GatewayContext) were advertised with a schema although every call failed.
- **[P1] The Context-mode gate removed only the schema metadata** — the
  retrieval tools stayed built and dispatchable, so the model could still call
  them with guessed arguments. The gate is now at the registration boundary.
- **[P1] `note_manage` fell back to a shared, predictable temp directory**
  (`/tmp/aleph/memory/note`) for durable, potentially private notes when the
  configured directory could not be resolved. It now fails closed.
- **[P1] `is_builtin_tool_name` reported 21 live registry tools as "not
  builtin"**, because the run loop offers the model the registry map while the
  predicate only knew the static catalog.
- **[P1] The description-literal census could only see const literals inside
  impl blocks**, so a trait-side description owner was invisible to it; the
  RPC-method census scanned one of four model-facing description tables.
- **[P2] The fallback `SessionManager` was derived twice and fired silently**
  with no event bus and no raw memory writer; the Hub-family deny list was not
  pinned to the catalog; the dispatch census covered 3 of 5 registration
  shapes, so 11 registered tools sat outside the advertised-versus-dispatchable
  guard; `inject_delivery_route` stamped an empty channel route, and `Some("")`
  overrode a configured channel id downstream; the current-session-key
  resolution was copied six times and had already drifted; the three partition
  accessors were 84–91 % identical copies.
- **[P2] The bash/code_exec description taught the model to send `timeout`
  while the schema it was shown named a different field** (schemars is blind to
  the serde alias); the `web_fetch` construction arm hardcoded the default
  SSRF policy, whose allow/block lists are empty; the registration log counted
  26 browser tools where 29 are registered and named a tool that is registered
  nowhere; the schema helper failed open to an empty object schema and was
  duplicated in two files; `register_optional_tools` re-derived the injection
  mode that the config already carries; `heartbeat_report` was registered
  twice; the catalog's `requires_config` flag was read by nobody.
- **[P3]** The memory-partition doc block moved to the function it documents;
  the `#![allow(unused_imports)]` that suppressed nothing was removed; the
  unreachable "not yet wired" arm was removed; `find_matching_close` no longer
  reads comments as code; a doc comment no longer calls a same-crate consumer
  "cross-crate"; `session_complete`/`memory_reflect` use the fallback agent id
  they are given.

### `src/export` — the document exporter

- **[P1] `render_markdown` called the HTML writer once per parser event**, so
  the writer's state was thrown away for every event: table body cells
  rendered as header cells, the column alignment was dropped and image `alt`
  text was emptied. The document now goes through one writer.
- **[P2] The export's byte ceiling counted raw artifact bytes** and ignored the
  transcript text and the 4/3 base64 inflation, so a document could exceed the
  store's 50 MiB limit and be rejected *after* it was built in full. The
  exporter now counts what is actually written, against the store's own limit.
- **[P3] `escape_text` and `escape_attr` were two copies of one escaping
  rule** — they are now derived from one rule, with the attribute invariant
  stated.

## Deferred (with the reason)

| Item | Why |
|---|---|
| `src/exec/decision.rs` — the `ApprovalRequest` alias collides with the wire enum of the same name | public-API removal; four importers and a contract test |
| `src/exec/socket.rs` — the module keeps the name of a transport it no longer contains and hosts another crate's tests | cross-crate test relocation needs an owner |
| `src/exec/parser.rs` — the never-supplied `env` parameter and the dead PATH-resolution branch | public-API change, or a producer that changes resolution behaviour |
| `src/exec/secret_patterns.rs` — the exec tables are a strict subset of `crate::secrets::vendor_patterns` | *which* catalog is the single source of truth is an owner decision; a doc pointer now records the gap |
| `src/exec/masker.rs` — a three-line PEM replacement lands in one diff hunk line | diff line-count semantics are a product decision |
| `src/exec/approval/channel_bridge.rs` — the capability path bypasses the registry's durable queue | the capability API is the only inline-approval path for some channels; a per-channel audit is needed |
| `src/exec/approval/channel_bridge.rs` — no re-announce on the attended path | the reminder cadence is a product decision |
| `src/exec/manager.rs` — the 2847-line file, the 841-line impl, the 155-line `resolve_for_session` | size findings rejected as maintainability hazards: the split is not mechanical |
| `src/executor/.../constructor/mod.rs` — `with_config` is 1444 lines ending in a ~290-line struct literal | a table-driven rewrite is not mechanically derivable |
| `src/executor/.../definitions.rs` — `create_tool_boxed` is a 347-line table with no production caller | keep-and-guard versus delete is a public-API decision; the doc now says what is true |
| `src/executor` — the Context-mode tool set, the team member cap, the SOUL.md write race | product decisions, and one finding lives outside the reviewed module set |
| `src/executor/.../definitions.rs` — the RPC census and the description census | applied; the broader coverage gaps above are separate |
| 8 machine findings | re-checked by the reviewer and **rejected** with code evidence (`live[0]` cannot panic, `expect("just checked live")` is an enforced same-lock invariant, the size findings are not hazards, the near-duplicate function bodies no longer exist) |

## Verification

- `cargo check -p alephcore --all-targets -j 1` — **0 errors**, run over the whole
  fix set (14m23s), and re-run after the last batch (15m06s). The only warnings
  are 12 pre-existing dead-code warnings in
  `src/gateway/server/panel_asset_route.rs`, untouched by this pass. A final
  `cargo check -p alephcore --lib -j 1` confirmed the committed tree (5m47s,
  0 errors) after the rustfmt pass.
- `rustfmt --edition 2021 --check` is clean on every touched file (three files
  drifted after the workers' edits and were reformatted).
- The build ran with the parallelism pinned to one job and `CARGO_BUILD_JOBS=1`,
  because the `--all-targets` unit-test target alone peaks at ~15 GB of RSS on
  this 19 GB host; the first attempt with `-j 2` ran two rustc processes
  (8.7 GB + 6.6 GB) and had to be restarted.
- **Not run**: `cargo test`, `cargo clippy`, and the workspace's other crates
  were not built or tested. The fixes add unit tests (redaction rules, the
  approval cascade, the empty decision set, the failed-analysis card, the
  markdown table rendering, the export budget); they are type-checked by the
  `--all-targets` check but were not executed.

## Notes

- The three modules were reviewed as a whole, not as a diff: every machine
  finding in scope was confirmed or rejected with code evidence by the batch
  that owned the file.
- `src/exec/manager.rs` was reviewed twice (the first run lost its output to a
  provider rate limit and was re-run on a second model). It found **no
  fail-open**: every timeout, closed-channel and undeliverable path converges on
  "no decision" or an explicit denial.
- The batch briefs, the per-batch reports, the parsed findings and the fix
  instructions are kept in `.review/rd-2026-10-31-exec-executor-export/`.
