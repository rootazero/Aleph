# Occam's-Razor static review — `src/cli`, `src/cluster`, `src/command`

Round date: 2026-11-07 · Base commit: `c21e4511c` · Method: `/rust-occams-razor` (full-module
static review, no diff) · Deliverables: this directory + 5 fix commits merged into `main`.

> This file is the round's index. The per-batch detail lives in
> `findings/B*.md` (what the review found) and `fixes/B*.md` (what the fix did with it).

## Scope and partitioning

| Batch | Module | Files | LOC |
|---|---|---|---|
| B1 | `src/cli` | `mod.rs`, `endpoint.rs`, `ipc_client.rs`, `policy.rs` | 1321 |
| B2 | `src/cluster` (registry) | `registry.rs`, `mod.rs` | 1699 |
| B3 | `src/cluster` (enrollment) | `enrollment.rs` | 1210 |
| B4 | `src/cluster` (node) | `node_approval.rs`, `node_runtime.rs`, `node_file_cmd.rs`, `reverse_rpc.rs` | 2113 |
| B5 | `src/command` | `mod.rs`, `parser.rs` | 428 |

Partitions are **file-disjoint**, so each batch got its own git worktree
(`/home/zou/data/workspace/Aleph-wt-rev-<batch>`) and branch off the same base commit, and the
branches merge without conflicts.

## Method: two model tiers, then cross-family adjudication

- **Jev tier (cheap model)** — classification, triage, routing, simple judgements: the per-batch
  *candidate sweep* (what might be wrong) and the *severity/verdict table* over the candidates.
  **This tier ran for every one of the five batches** (B1–B5) and produced the candidate tables
  that each `findings/B*.md` records.
- **Reasoning tier** — the deep review of each batch: the findings with a concrete failure mode,
  the evidence, the minimal fix, and the invariant each fix preserves. **B1, B2, B3 and B5** were
  reviewed by a delegated deep-review agent; **B5's deep review and B4's deep review were done by
  the orchestrator** (B5 because no agent was available when the round's quota was exhausted, B4
  because the delegated agent ran ~75 minutes / 571 tool calls, read past its four-file scope and
  produced no findings file — it was stopped and the batch completed by the orchestrator, which
  read all four files in full).
- **Cross-family adjudication** — every finding set except B4's was then put to a *second model
  family* (via the raw magpie proxy: `88apiclaude/claude-sonnet-5-5` and `88apigpt/gpt-6.1-sol`)
  with the job of **refuting**: "is this real, is the severity right, does the fix plan break
  something?".

> **An honest note on the tiers.** On this machine `~/.claude/settings.json` points
> `ANTHROPIC_DEFAULT_{HAIKU,SONNET,OPUS}_MODEL` at `minimax-cn/MiniMax-M3.1-Flash`, so **all four
> Agent-tool aliases (haiku / sonnet / opus / fable) are served by the same MiniMax model**. The
> "Jev tier" and the "reasoning tier" were therefore separated by **role and prompt** (triage
> without edits vs. deep review with edits), not by two different models. The genuine
> cross-family check came from the **raw magpie API**, which reaches Claude Sonnet 5.5 and
> GPT-6.1-Sol — a different family from the reviewer that produced the findings.

The adjudication changed the outcome in both directions and is recorded per finding:

| Batch | Cross-family verdict | What it changed |
|---|---|---|
| B2 | 10/10 VERIFIED, no severity corrections | 3 fix-plan risks folded into the fix (exhaustive destructuring, oversized-vs-unserializable, the `deregister` return type) |
| B5 | B5-01 OVERREACH, B5-02 REJECT, B5-05 REJECT; B5-03 VERIFIED at P3 | the public `UnifiedTool` fields stayed (they are a cross-crate contract type); the "no reader" claim was re-proved against the Panel crate |
| B1 | P1-1/2/3, P2-1/2/4/5, P3-3, M1 VERIFIED; **P1-4 REJECTED**; D1 OVERREACH → P2; D2 REJECTED | the script-gate finding was downgraded to a report item; the redirect-policy item was lowered to "decide with evidence" |
| B3 | 17/17 VERIFIED (Sonnet, no severity corrections); GPT: P1-01 REJECTED, P1-03 → P2, P2-04 OVERREACH | the re-enrollment item became a documented recovery instead of a change; the fingerprint redesign was ruled out of scope (it needs a schema migration) |
| B4 | **not adjudicated** — the deep review was done by the orchestrator (see the method note) | — |

## The fixes (one commit per batch, merged into `main`)

<!-- FILES -->

| Commit | Batch | What it changed |
|---|---|---|
| `3a897b32d` + `9af972f3f` | B1 `src/cli` | the admin forwarder now refuses a route that carries an authority (`//host/…`, `/\host/…`) and re-checks the **joined** URL's scheme/host/port before the bearer token is read; the client is built with `.no_proxy()` (so `HTTP_PROXY`/`ALL_PROXY` cannot capture the token) and follows **no** redirect; the endpoint record's writer identity is proved (PID alive **and** start time matching) before the token is read; the shared-token read is ordered; the stale-holder message and the error-body cap name were corrected; a failed permission re-assert no longer deletes a valid endpoint file |
| `5f751aa9e` | B2 `src/cluster/registry.rs` | the flag-dropping `resolve` is gone (one resolver: `resolve_with_catalog`); `NodeMatch` carries the malformed-catalog flag, so the tools refuse a peer with an unparsable catalog; `node_list` reads a `NodeSummary` instead of deep-copying up to 64 schemas per node; the schema-size check counts bytes while it writes (oversized ≠ unserializable); `Environment.status` is a typed `EnvironmentStatus` instead of a stringly `&'static str`; `deregister` returns the evicted session |
| `279ee9d66` | B3 `src/cluster/enrollment.rs` | an unreadable device store is **never** an admission (one `StoreUnavailable` outcome for the read failure and the mint-write failure); an ambiguous first boot is refused instead of minting a third ghost row; deregistration **revokes first** — a failed write is an error, not a success; a repeat deregistration is idempotent; the presented id is length-bounded; the store writes the same bounded name as the registry; the store calls moved to `spawn_blocking`; the operator errors are `INVALID_PARAMS` and the store faults are `INTERNAL_ERROR` |
| `e17681c88` | B5 `src/command` | the slash envelope's two reader-less copies (`system_prompt`, `pattern`) are gone — `CommandContext::Custom` is a unit variant; the parser's false claims ("one process-wide parser", "allowed_tools is read from the skill file") are corrected; the public `UnifiedTool` fields stay, but their docs now say they have no reader |
| `97c213b76` | B2 (late) | the unreachable `ResolveError::NodeNotFound` variant is deleted — the id `match_id` returns is a key of the same map the lookup reads, under the same guard |
| `76d9ff6dc` | B4 `src/cluster/{node_approval,node_runtime,node_file_cmd,reverse_rpc}.rs` | the node file jail's **read** side now opens with `O_NOFOLLOW` exactly like the write side (the comment that claimed a read-side counterpart described a guard that did not exist); both sides also open with `O_NONBLOCK`, so a FIFO planted at the leaf fails instead of parking a worker thread in a blocking `open(2)`; two tests pin the two outcomes the guard distinguishes |
| `7063bb33e` | B2 (late) | the two `tests/` helpers that polled the registry were migrated off the removed `NodeRegistry::resolve` — the unified `cargo check --all-targets` caught what the in-crate test sweep missed |

## Deliberately NOT done (and why)

<!-- NOTDONE -->

- **The `/ws` intake cap (`ws.max_message_size`)** — the WebSocket route sets no cap while the browser-viewer route caps at 8 KiB, but the two are *retention* bounds, not *intake* bounds; changing them is a gateway-wide decision, not a cluster one.
- **The fingerprint derivation** — a presented id is truncated to 16 characters before it becomes a fingerprint. Changing that needs a schema migration, so the bound was added and the derivation left alone.
- **`UnifiedTool::routing_system_prompt` / `routing_regex`** — they are `pub` fields of a cross-crate contract type, so deleting them is a public-API change that was not authorised. The internal copies are gone; the fields now carry a doc that says they have no reader.
- **The `spec_c_regression.sh` gate** — the script is red on `main` (checks 1 and 2 fail on ~10 production `Connection::open` call sites and one vault write) and its check 3 is satisfiable by a comment (`with_policy_owned` exists nowhere). Repairing checks 1–2 means touching ten files outside the reviewed modules; check 3 was left alone rather than half-repaired. Reported instead of silently rewritten.
- **The parent-directory swap window in the node file jail** — the **leaf**-level half is now closed (both the read and the write side open with `O_NOFOLLOW`); the **parent-directory/root** window stays open and is still documented as such in the source (closing it needs a pinned root fd and `openat`, which is a different mechanism). A half-fix would have made the hole look closed.
- **The non-loopback gateway/admin endpoint contract** — the CLI refuses a non-loopback endpoint host; the daemon publishes one. That is a deliberate fail-closed posture, not a defect to fix.

## Verification

<!-- VERIFY -->

**One unified check, at the end of the round**, on the merged `main`:

```
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo check -p alephcore --all-targets -j1
```

- **Exit code 0, 0 errors.** 25 warnings, all pre-existing dead-code warnings
  (`src/extension/projection.rs`, `src/utils/source_scan.rs`, `src/bin/aleph-server/commands/start/builder/subsystems.rs`,
  the two test helpers) — the round added **no** new warning.
- **Memory-capped for this 20 GiB / no-swap box:** one cargo at a time behind a `flock` on a file
  descriptor held for the whole run (`/home/zou/data/workspace/.r11-cargo.lock`, outside the repo),
  `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_{DEV,TEST}_DEBUG=0`, `-j1`.
- **What the check caught:** the two `tests/` helpers (`tests/cluster_reverse_rpc.rs`,
  `tests/cluster_node_runtime.rs`) that still called the `NodeRegistry::resolve` the B2 batch
  deleted — the in-crate test sweep had missed them. Fixed in `7063bb33e`; the same check then
  passed. This is the "evidence before deletion" rule catching a deletion whose evidence was
  incomplete.

### Not verified in this round

- **`cargo test` was not run** — the round was scoped to a static review plus one unified check.
  The tests this round touched were adapted by hand (their assertions' meaning preserved) but
  **not executed**.
- **`cargo clippy` was not run** — the local toolchain disagrees with the repo's committed
  formatting (see below), so a clippy pass would have produced noise the round could not act on.
- **The Panel / WASM crate was not built** — `interfaces/webchat` does not depend on `alephcore`,
  and the round's wire changes were proved to have no reader outside the crate (the slash-mode
  JSON never leaves alephcore; `EnvironmentStatus` has one in-crate consumer).
- **Formatting:** the base tree already has **99 rustfmt-dirty files** under this box's
  `rustfmt 1.10.0-stable` (CI runs `cargo fmt -p alephcore -- --check`), so the tree was
  deliberately **not** reformatted. Every file this round touched was checked with
  `rustfmt --edition 2021 --check` individually, and the two files that were newly dirty were
  brought back to clean.

