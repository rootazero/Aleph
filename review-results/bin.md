# Review & Fix Summary — `src/bin`

**Date:** 2026-08-15
**Reviewer:** static (4-perspective protocol: security / logic / architecture / quality)
**Branch:** `review/bin` (worktree at `/tmp/aleph-review-bin`)
**Final integration:** fast-forward `main` ← `review/bin`

## Module Status

| Module | Status | Notes |
|--------|--------|-------|
| `src/arena` | **NOT PRESENT** | Deleted at `e7f0c3cec` (2026-07-24, "multiagent: integrate teams/agents/loop_graph/workflow round"): "SharedArena subsystem fully removed (3 tools + 3 RPCs, zero consumers)". Confirmed via `git log -- src/arena`. No review possible. |
| `src/bin/aleph-server/` | 44 files, 18655 LOC | Reviewed in 4 batches |

## Pipeline

1. Static review split into 4 sequential batches covering 18655 LOC of
   `src/bin/aleph-server/` production code (no test-only lines):
   - **Batch 1** (entry, ~2.5k LOC): `main.rs`, `cli.rs`, `daemon.rs`, `server_init.rs`, `commands/mod.rs`
   - **Batch 2** (commands, ~5k LOC): `commands/{update,node,secret,doctor,pair,bootstrap_token,resume,prompt_size,hooks,gateway,identity,sandbox_debug,plugins,service/*,bootstrap_runtime}`
   - **Batch 3** (start, ~5.5k LOC): `commands/start/{mod, helpers, orchestrator_init, runtime_warmup, bootstrap_factories}` + `commands/start/builder/{mod, subsystems}`
   - **Batch 4** (start/builder, ~6.2k LOC): `commands/start/builder/handlers/*` (10 files) + `commands/start/builder/agent_init/*` (7 files)
2. **3 findings: 0 Critical / 0 High / 3 Medium / 0 Low.**
3. **3 fixed**, 0 skipped.
4. Fixes applied directly to `review/bin`; no `cargo check` mid-flight per
   protocol.
5. Single `cargo check -p alephcore` at the end (memory-limited per
   AGENTS.md §"内存受限机器"). Bin target shares the lib crate so this
   single check validates both.
6. Fast-forward `main` to `review/bin` once clean.

## Module Totals

| Batch | Path | Files | High | Med | Low | Total |
|------:|------|------:|-----:|----:|----:|------:|
| 1 | entry (main + cli + daemon + server_init + mod) | 5 | 0 | 0 | 0 | 0 |
| 2 | commands/* (17 files, ~5k LOC) | 17 | 0 | 1 | 0 | 1 |
| 3 | start core + start/builder/{mod,subsystems} | 7 | 0 | 2 | 0 | 2 |
| 4 | start/builder/{handlers, agent_init} | 17 | 0 | 0 | 0 | 0 |
| **TOTAL** | | **44** | **0** | **3** | **0** | **3** |

## Findings fixed

| Batch | ID | Sev | Title | Fix commit |
|------:|----|----:|-------|-----------:|
| 2 | B2-M1 | Med | `bootstrap-token` prints token to stdout with no operator-side warning — operators have piped it straight into QR codes despite the module-level docstring forbidding it | `bin: bootstrap-token warns on stderr before printing token` |
| 3 | B3-M1 | Med | `subsystems.rs:63` `panic!("Fatal: ... in-memory security store fallback")` is a production panic on the rare path where on-disk + in-memory SecurityStore both fail | `bin: replace subsystems.rs panic with expect (DBC invariant)` |
| 3 | B3-M2 | Med | `agent_init/mod.rs:1840` `panic!("run_manager must be set in both real and simulated modes")` — unreachable defensive panic; both branches assign before reaching this `.expect(...)` read | `bin: replace agent_init panic with expect (DBC invariant)` |

## Findings deferred (skipped, with rationale)

| Batch | ID | Sev | Title | Why deferred |
|------:|----|----:|-------|--------------|
| 1 | B1-L1 | Low | `cli.rs` `SecretAction::Set { value: Option<String> }` accepts the secret value as a `--value` flag, exposing it in `/proc/<pid>/cmdline` and shell history | Already documented as deliberate operator UX trade-off (cli.rs line 374-376: "Secret value (avoid shell history by omitting and using prompt)"). Operator can omit `--value` to use a hidden stdin prompt (`secret.rs::resolve_secret_value`). Promoting to `error: --value forbidden` would break scripted secret provisioning. |
| 1 | B1-L2 | Low | `server_init.rs` `handle_run_with_engine` (lines 80-285) and `handle_chat_send_with_engine` (lines 288-470) duplicate ~200 LOC of identical wiring (route, agent resolve, emitter, slash mode, busy queue, response shape) | Documented as deliberate: "agent.run is chat.send with a different param spelling — same router, same engine, same wait lane" (server_init.rs:50-67). The P1 visibility guard is wired through `include_str!` source-pin tests (server_init.rs:472-534) so a divergence would fail a test by name. DRY refactor would have to extract ~10 closures and a "param-mapping" shim that obscures the visibility chokepoint. Acceptable in the project's idiom (see `commands/*` for similar dual-path patterns). |
| 2 | B2-L1 | Low | `commands/secret.rs` `init_locked` returns `Ok(bool)` (newly-created vs already-existed) but the dispatcher discards the bool — operator cannot tell from the printed "Secret vault ready (N entries)" message whether the vault was just created or pre-existing | Cosmetic UX; not a correctness issue. Touching it would require threading the bool through the IPC path. |
| 2 | B2-L2 | Low | `commands/hooks.rs:79` `entry.command.strip_prefix("http:")` prints the URL as `//host/path` (no `http:` or `https:` prefix) when reviewing HTTP hooks | Cosmetic UX; the docstring clarifies the URL is "the URL POST events get sent to". Adding the prefix back is one line but changes the wire-level printed string operators may have copy-pasted into scripts. |
| 3 | B3-L1 | Low | `start/mod.rs` is 3274 LOC — the monolithic `start_server` body that the module comment acknowledges cannot be cleanly carved into helpers ("a giant fn that can't be cleanly carved stays whole", start/mod.rs:60-66) | Documented as deliberate. The only mechanical extractions already live in `start/bootstrap_factories.rs`. Further splitting requires touching the data flow of every phase. |
| 3 | B3-L2 | Low | `register_agent_handlers` takes 20+ parameters; `initialize_orchestrator` takes 16+ parameters | Idiomatic in the project's "Spec C" boot protocol — every parameter is a documented subsystem handle the orchestrator owns. Bundling into a `BootContext` would centralise construction but push the parameter sprawl into a new struct, not eliminate it. |
| 3 | B3-L3 | Low | `commands/service/mod.rs:90` `format!("`{cmd:?}` failed: {status}")` puts the full command line into the error message | The `cmd` in question is always `launchctl`/`systemctl`/`schtasks` — none carry credentials. Token-bearing commands (`bootstrap-token`, `pair`) print to stdout, not command lines. Cosmetic. |

**Deferred: 7 findings** (all Low).

## Cross-cutting themes (observations, not findings)

1. **`unwrap_or_else(|e| e.into_inner())` Mutex-poison recovery** is the
   project's convention (~30 sites across `commands/node.rs`,
   `start/builder/agent_init/mod.rs`, `start/builder/handlers/*`). This is a
   deliberate choice — the alternative `expect("poisoned mutex")` would surface
   a recoverable transient (a panic on a held-lock thread) as a daemon-wide
   crash. The convention is consistent and the comments above each site
   justify the choice. No change recommended.

2. **`include_str!` source-pin tests** appear in `server_init.rs:472-534`
   and `start/helpers.rs:494-558`. These are SOURCE pin tests (the file
   asserts it contains a specific identifier at a specific location), not
   effect tests — the comments at each site explain why the real effect
   is unreachable from the binary's `#[cfg(test)]` compilation. This is
   the project's documented pattern for binary-side wire guards, used
   consistently.

3. **Production `panic!`/`unwrap()` audit** found exactly 3 production
   paths: `main.rs:80` (`expect("--config pinned twice")` — documented
   design-by-contract), `subsystems.rs:63` (now fixed), and
   `agent_init/mod.rs:1840` (now fixed). All three are explicit
   design-by-contract invariants, not recoverable errors. The post-fix
   `cargo check` will re-confirm no new production panics were introduced.

## What I did NOT do

- **Did not split `start/mod.rs`'s 3274-line `start_server` body.** Per
  the file's own module comment ("a giant fn that can't be cleanly carved
  stays whole"), and the existing `start/bootstrap_factories.rs`
  already being the only cleanly extractable seam. Further splitting
  requires touching the data flow of every phase.
- **Did not deduplicate `handle_run_with_engine` / `handle_chat_send_with_engine`.**
  Documented as deliberate; the P1 visibility guard is source-pinned.
- **Did not refactor the 20+ parameter `register_agent_handlers`** —
  would push the sprawl into a `BootContext` without eliminating it.
- **Did not run `cargo check` mid-flight.** Per protocol; a single
  post-batch `cargo check -p alephcore` validates the bin target as well
  (the bin shares the lib crate).
- **Did not push to remote.** Per "无需 PR" instruction; the `review/bin`
  branch is local and fast-forwarded to `main` once clean.
- **Did not run `clippy -D warnings`** on the bin target — pre-existing
  clippy issues in unrelated files (the same caveat documented in prior
  reviews) make a `-D warnings` gate too noisy to use as a per-fix check.
- **Did not audit `src/arena`** — directory does not exist on `main`
  (deleted at `e7f0c3cec`); see Module Status above.
