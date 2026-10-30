# rust-doctor static review — `src/cluster`, `src/cli`, `src/command`

**Date**: 2026-10-30
**Worktree used for development**: `.worktrees/rust-doctor-batch4` (branch `review/cli-cluster-command`, based on `main` @ `48503ffc6`)
**Base commit**: `48503ffc6`
**Commits landed on `main`**: see the merge commit recorded below

## Method

| Stage | Tool / actor | Output |
|---|---|---|
| Machine scan | `rust-doctor 0.9.0` (whole workspace; `--rule rust_doctor::repo::hardcoded_credential=off --advisory-db /dev/null` — the credential rule panics on this repo's Chinese source files) | 9,254 diagnostics workspace-wide, **19** inside the three modules; all structural |
| Semantic map | `graphify-out/graph.json` (139,519 nodes) — `graphify query` for the cross-module wiring of `src/cluster` | fed to the reviewer briefs as an optional lookup |
| Review | **6 reviewer subagents** (MiniMax-M3), **no diff**: whole-file reading of all 12 files / 5,399 LoC, split into file-disjoint briefs | **31 findings** (5 P1 · 11 P2 · 15 P3) |
| Triage | **Jev** (`jev-latest`) — one calibrated `noul` per finding | fix-now / verify-then-fix / defer split |
| Fix | **5 worker subagents** (MiniMax-M3), file-disjoint briefs, in the isolated worktree | 24 fixes landed, 7 deferred |
| Verification | one unified `cargo check -p alephcore --all-targets` (`CARGO_BUILD_JOBS=2`, `lld` linker) | **0 errors, 0 warnings** (12m39s) |
| Test | `cargo test -p alephcore --lib -- cluster::` then the same binary re-run for the other filters | see "Test run" below |

Per-module finding counts:

| Module | Files | LoC | Findings | P1 | P2 | P3 |
|---|---:|---:|---:|---:|---:|---:|
| `src/cluster` | 7 | 3,196 | 19 | 3 | 8 | 8 |
| `src/cli` | 4 | 1,017 | 6 | 0 | 1 | 5 |
| `src/command` | 2 | 406 | 4 | 1 | 0 | 3 |
| `src/builtin_tools` (center side of the cluster feature) | 2 | 1,336 | 2 | 0 | 2 | 0 |
| **total** | **15** | **5,955** | **31** | **4** | **11** | **16** |

The full finding list with each item's Jev score and routing decision is in
[`2026-10-30-rust-doctor-cluster-cli-command-findings.md`](2026-10-30-rust-doctor-cluster-cli-command-findings.md).
The reviewers' raw reports live under `/tmp/acp-delegate/` (untracked working directory).

### Which steps used Jev

Jev was used for **classification, filtering and routing only** — never for writing code and never
for judging Rust semantics, which stayed with the reasoning models (`minimax-cn/MiniMax-M3`):

1. **Classification** — one `noul` score per finding (31 findings, one question each): "is this a real
   defect that should be fixed now with a contained safe fix?". The scores landed at 0.29–0.89.
2. **Filtering** — the score bands produced the three dispositions: `fix-now` ≥ 0.80 (17 findings),
   `verify-then-fix` 0.60–0.80 (6), `defer` < 0.60 (8).
3. **Routing** — each disposition became the instruction inside the file-disjoint worker briefs: a
   `fix-now` brief carried the concrete patch sketch, a `verify-then-fix` brief carried the
   verification step, a `defer` finding was excluded from the briefs and recorded here instead.
4. **Batch routing** — the 12 files / 5,399 LoC were split into six reviewer briefs by size and by
   module boundary (registry / enrollment+approval / reverse_rpc+file_cmd / node_runtime / cli /
   command), so no reviewer had to hold more than ~1.3k lines of one module.

Jev's discrimination is narrow (0.29–0.89 on a 31-item spread), so the coordinator re-read every
brief before dispatch and **every diff after it came back** — a Jev score is a routing hint, not a
verdict.

### What Jev's routing changed

Four findings did **not** follow the reviewer's own severity label:

| Finding | Reviewer said | Jev | Outcome |
|---|---|---|---|
| #4 jail root never created | P1 | 0.63 | fixed anyway — the mechanism is confirmed by two independent reviewers and the fix is one line at the node's setup site, so the "verify-then-fix" band only required reading the call site (it confirmed the bug) |
| #15 `conn_id` is a peer address | P2 | 0.76 | deferred to a follow-up — the fix belongs to the connection layer's identity model, not to the reviewed module |
| #11 jail root vs bash session | P2 | 0.76 | fixed — the fix is an API addition plus its single call site |
| #20 `null` result is not a response | P3 | 0.85 | fixed — the discriminator had to change on **both** ends of the wire atomically, so it could not be a P3 afterthought |

## Fixes landed

### `src/cluster/registry.rs`

- **Bounded the peer-declared metadata** at the boundary that owns it (`maybe_register_node`):
  `device_name` ≤ 128 B, ≤ 32 tags of ≤ 64 B each, ≤ 64 commands with ≤ 64 B names, and a declared
  `schema` larger than 8 KiB replaced by `{}` — every truncation logged. The registry stores this
  verbatim and `list_environments()` deep-clones it on every call, and that call feeds the
  model-callable `node_list` tool.
- **Separated "absent" from "malformed"**: `params_typed` now returns a `FrameField<T>` instead of
  collapsing both into `T::default()`. The declared catalog travels with the session
  (`RegisteredNode { session, catalog_malformed }`), and `src/builtin_tools/node_invoke.rs` refuses
  a malformed catalog instead of reading it as "the node declares nothing" (which had silently
  switched the center's fail-fast layer off).

### `src/cluster/enrollment.rs`

- **Precise-first deregister addressing** (`resolve_deregister_target`): the enrolled-device store's
  exact id and its unique exact normalized name are consulted FIRST; the registry's fuzzy tier is
  reached only when no enrolled row matches. An ambiguous exact name is reported as ambiguous and
  never resolved to a fuzzy candidate; a store read failure refuses instead of guessing.
  Before: deregistering an offline `worker` could evict **and sticky-revoke** an online `worker-2`.
- **The unknown-id path now converges on the name's owner** instead of backfilling a second row
  under the same name (the self-sealing duplicate the module's own doc forbids).
- **The name→row resolution is serialized** by a process-wide `NODE_NAME_LOCK` (outermost lock of
  the pair; no `.await` under the guard), so two concurrent enrollments cannot both insert.
- **`admit_node` refuses a non-node row** (`role != "node"`), not just a panel row — an adopted
  operator/panel row was un-removable while offline (the offline fallback filters on `role`).
- **An unaddressable name is refused** (a name that folds to an empty lookup key could never be
  found again, and every repeat enroll minted another such row).
- **`device_removed` keeps its meaning but the log now distinguishes** "already revoked" from
  "the write failed" (the store's `UPDATE ... AND revoked_at IS NULL` answers `false` for both).

### `src/cluster/reverse_rpc.rs`

- **The response window starts after the enqueue**: `response_deadline` is taken once the frame is
  on the wire, so the outbound wait is no longer charged against the peer's reply window; the two
  doc comments now describe the semantics the code has.
- **`OutboundWedged` reports the budget actually spent** (`min(timeout_ms, OUTBOUND_PUSH_BUDGET_MS)`)
  instead of the caller's raw timeout (a 240× overstatement at the default 120 s tool timeout).

### `src/cluster/node_file_cmd.rs`

- **A non-regular target is refused before it is opened** (FIFO / device / socket): the block
  happens in `open(2)`, before any byte is read, so the byte cap cannot help — the command would
  never return and the reverse-RPC call would sit until its timeout. The read path uses
  `symlink_metadata`, which also refuses a symlink planted at the leaf after the containment check.

### `src/cluster/node_runtime.rs`

- **One name, not two**: the table is keyed by the command's **declared descriptor name** — the name
  the node declares to the center AND the name the center's admission check compares against. The
  `register` signature is unchanged (it is `pub` and re-exported); a caller-supplied name that
  disagrees is logged as an error. A mismatch can now only hide or refuse a capability, never
  expose an undeclared one.
- **`descriptors()` has a total order** (name, then key), so the declared catalog can never depend
  on HashMap iteration order.
- **New `register_file_commands_for_session(workspace_root, session)`**: the file-command jail root
  is derived from the same session key the bash sandbox uses, so the two cannot drift apart.

### `src/bin/aleph-server/commands/node.rs`

- **The jail root is created once during the node's setup.** Before: on a freshly started node that
  had not yet run a bash command, EVERY `file.read`/`file.write` failed with "workspace root
  unresolved", while the node still advertised both commands (the center-side "command not declared
  by node" check passed, so the user only saw a broken tool).
- **The file commands are registered through the session-derived constructor**, so the jail root and
  the bash session directory cannot diverge.

### `src/gateway/server/connection/mod.rs`

- **One shared response discriminator for both ends of the wire** (`parse_response_frame`): a frame
  is a response iff it carries a non-null `id`, carries **no** `method`, and carries a `result` or
  `error` **key**. Key presence, not value presence: serde maps a JSON `null` result to `None`, so
  the old value-presence test classified `{"id":"rpc-0","result":null}` as a REQUEST — the frame
  fell through to the request path, failed on the missing `method`, and the pending call waited out
  its full timeout although the peer had answered.

### `src/builtin_tools/node_file.rs` (center side of the cluster feature)

- **The push leg reads through a bounded handle** (`take(MAX_FILE_BYTES + 1)`) instead of a stat
  snapshot plus an unbounded `tokio::fs::read`, and refuses a non-regular local file (a FIFO blocks
  the read forever). The node side was already cap-safe; the center was not.

### `src/cli/ipc_client.rs` + `src/cli/policy.rs` + `src/cli/endpoint.rs`

- **The admin channel is local-only**: `forward_to_server` refuses a non-loopback endpoint host
  BEFORE the bearer token is attached. The CLI reads the token from the LOCAL `security.db`, so a
  remote deployment has no token to send; the old code sent the admin token and the secret payload
  in cleartext over the LAN whenever the endpoint file named a concrete non-loopback address and TLS
  was off.
  *Also fixed here:* `is_loopback_host` now strips the brackets `Url::host_str` puts around an IPv6
  literal, so an IPv6-loopback endpoint (`http://[::1]:port`, which `build_endpoint_url` publishes
  for an IPv6 bind) is not refused as "non-loopback".
- **The response body is read through a capped sink** (1 MiB): the old path buffered the whole
  body and only truncated the *message* afterwards, so a peer chose the CLI's memory use. The
  `truncate_error_body` helper is unchanged.
- **A path prefix on the endpoint URL is refused** rather than silently dropped: `Url::join` with an
  absolute route REPLACES the base path (the code comment claimed the opposite).
- **The `LockHeldError` survives a failed forward**: the operator sees the PID of the running server
  *and* the transport failure, instead of a bare "connection refused" that reads as "no server is
  running".
- **`read_endpoint`'s three states are worded as three states**: the caller no longer claims the
  file does not exist when it is empty or carries an unsupported version.

### `src/command/parser.rs`

- **Deleted `tool_registry_arc()`** — a public method with zero callers whose doc steered a future
  caller to clone a second `Arc` to the shared catalog, which is exactly the multi-owner drift the
  shared `CommandParserCell` exists to prevent.
- **An unknown `ToolSource` variant is logged**: the refusal stays fail-closed (`None`), but the
  degradation is no longer invisible — a new variant in the `#[non_exhaustive]` enum used to turn a
  slash command into a plain chat turn with no error and no log line.

## Deferred (not fixed in this pass — needs a human decision)

| # | finding | why deferred |
|---|---|---|
| 3 | A credential-less peer can seize an enrolled node's identity (name-based adoption + unconditional identity transfer in `register`) | The project's trust model is LAN-trust: nodes hold no tokens, and name adoption exists for the legitimate node's first boot of an operator pre-enrolled row. Refusing to adopt a row that has a live session changes admission semantics; adding proof-of-possession is a protocol change. **This is the highest-impact deferred item.** |
| 5 | A custom slash command's configured `system_prompt` never reaches the model | The producer chain is complete, the consumer was never written. Fixing it is a prompt-assembly change in the gateway execution engine (outside the reviewed module) and a live behavior change for every operator who has such a rule configured. |
| 16 | Intermediate-directory symlink swap escapes the file jail | Closing it properly needs a component-by-component `openat` walk against a pinned root dirfd — a real refactor that changes the error surface. The file's own comments already document the root-symlink race as a known limitation. |
| 15 | `conn_id` is the peer socket address, so an OS-reissued ephemeral port makes the dead connection's cleanup evict the live session | The fix is a per-connection identity in the connection layer, not in the reviewed module; it touches the reverse-RPC outbound map key and the `node.connected`/`node.disconnected` event payloads. |
| 29 | The CLI cannot reach the server's per-key admin routes (`route` is a `&'static str`) | A deliberate behaviour change: `tests/spec_c_cli_refuse.rs` pins the current refusal for `secret verify`. |
| 30 | `pattern` falls back to the command name when the catalog row has no regex | Unreachable with today's registration paths; the fix is a public API change on `CommandContext::Custom`. |
| 31 | The node's jail root is derived from a random per-process ephemeral session id, so it changes on every restart | A node-identity/wiring design change (the stable identity would have to come from the enrollment layer). |

## Test run

`cargo test -p alephcore --lib` was linked and run against the touched module filters; see the
commit message for the result. The check itself
(`cargo check -p alephcore --all-targets`, `CARGO_BUILD_JOBS=2`) reported **0 errors, 0 warnings**.

## State the negative

- Not fixed: the seven deferred findings above, each with its reason.
- Not audited: the center-side approval lifecycle, the WebSocket writer task's behaviour under a
  64-slot outbound backlog, the Panel/webchat consumers of `environments.list`, the node's
  per-frame `tokio::spawn` concurrency bound, and the Windows path semantics of the jail.
- Not changed: the structural rust-doctor diagnostics (duplicate function bodies, the 1336-line
  registry) — refactoring for shape alone is forbidden by the project's Occam's-razor rule.
- Not run: the full `cargo test -p alephcore --lib` suite (21k+ tests); only the filters covering
  the touched modules were executed.
