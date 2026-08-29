# Module: `interfaces/cli` (review 2026-08-29)

## Summary

- **Files**: 53 (52 `.rs` + `Cargo.toml`; `build.rs` is a trivial VERSION-file re-export)
- **LOC**: ~6 200 lines of Rust across `src/commands/` and `src/output/`
- **Issues**: 10 total (0 critical / 2 high / 6 medium / 2 low)
- **R4 Interface layers are pure I/O**: PASS — every command is a thin argv → JSON-RPC translator; the only local I/O lives in dev-only plugin scaffolding (`plugin_cmd.rs`), `daemon` process management, and `open` browser launch, all of which are shell-side I/O rather than business logic.
- **Wiring completeness**: PASS — every `Commands` / `*Action` variant in `cli_args.rs` has a matching dispatch arm in `main.rs`; tests in `main.rs` guard against clap definition drift and preview surfaces that have no handler.

This review covers the CLI reference client only (`interfaces/cli`). `interfaces/tui` and `interfaces/webchat` were not examined.

## High-Confidence Issues

### [High] `run_follow::follow_run` swallows run failures

- **Location**: `src/commands/run_follow.rs:149-154` (before fix)
- **Trigger condition**: any run that ends in `StreamEvent::RunError` — provider failure, tool execution error, safety block, gateway-side abort — causes `aleph ask` and `aleph chat-control send --stream` to exit `0` and (in human mode) print only the error text to stderr.
- **Expected behavior**: a failed run is a failed command; the CLI should return a non-zero exit code and propagate the root cause so scripts and CI can react.
- **Actual behavior**: `StreamEvent::RunError { error, .. }` broke the event loop but `follow_run` returned `FollowOutcome { final_text }` unconditionally; both callers (`ask.rs`, `chat_cmd.rs`) ignored the outcome's provenance.
- **Suggested fix**: make `follow_run` return `CliResult<FollowOutcome>`; on `RunError`, return `Err(CliError::Other(error))`. Callers propagate with `?`.
- **Decision**: FIXED in this round. `follow_run` now returns `CliResult<FollowOutcome>`; both `ask` and `chat-control send --stream` propagate the error.

### [High] `doctor --fix` AI repair run failure is not surfaced as a command failure

- **Location**: `src/commands/doctor.rs:313-316` (before fix)
- **Trigger condition**: `aleph doctor --fix` launches an `agent.run` repair; if the agent run itself errors (e.g. provider down, safety block), the repair loop printed `Repair run error: {error}` and then returned `Ok(())`. The surrounding verification pass then ran and, if the original required checks were still failing, exited `2` — but only because the underlying problem was still present, not because the repair attempt failed.
- **Expected behavior**: a repair run that cannot complete should make `doctor --fix` fail with the run's error.
- **Actual behavior**: the `RunError` event was consumed locally and discarded; the function returned success.
- **Suggested fix**: capture the run error, close the client cleanly, then return `Err(CliError::Other(error))`.
- **Decision**: FIXED in this round.

### [Medium] `channels list` silently renders an empty table on malformed responses

- **Location**: `src/commands/channels_cmd.rs:34-36` (before fix)
- **Trigger condition**: `channels.list` returns a body that does not deserialize into `Vec<ChannelInfo>` (e.g. a wire-shape change). `serde_json::from_value(...).unwrap_or_default()` turns the error into an empty `Vec`, so the command prints `No channels configured` and exits `0`.
- **Expected behavior**: a malformed response should propagate as an `INVALID_PARAMS`-style CLI error so the wire drift is visible.
- **Suggested fix**: replace `unwrap_or_default()` with `map_err(|e| CliError::Other(format!("invalid channels.list response: {e}")))?`.
- **Decision**: FIXED in this round.

### [Medium] `cron list` silently renders an empty table on malformed responses

- **Location**: `src/commands/cron_cmd.rs:38-40` (before fix)
- **Trigger condition**: same pattern as `channels list`: `cron.list` response deserialization failures are silently dropped.
- **Expected behavior**: fail loudly on a malformed response.
- **Suggested fix**: same pattern as `channels list`.
- **Decision**: FIXED in this round.

### [Medium] `memory dreaming` silently treats malformed responses as "no dream daemon"

- **Location**: `src/commands/memory_cmd.rs:237` (before fix)
- **Trigger condition**: `dreaming.list_insights` returns a response that does not deserialize into `DreamSchedulingStatus`. The code used `unwrap_or_default()`, which produces the "no dream daemon in this server process" message even when the daemon is present but the wire shape has drifted.
- **Expected behavior**: a malformed response should error; the "no daemon" message should only appear when the server actually reports an absent daemon.
- **Suggested fix**: replace `unwrap_or_default()` with `map_err(...)?`.
- **Decision**: FIXED in this round.

### [Medium] `daemon stop` on non-Unix returns success after printing an error

- **Location**: `src/commands/daemon.rs:303-313` (before fix)
- **Trigger condition**: running `aleph daemon stop` on Windows or any other non-Unix platform.
- **Expected behavior**: the command should fail because signal-based stopping is unsupported.
- **Actual behavior**: the function printed an error message to stderr and then returned `Ok(())`, so the CLI exited `0`.
- **Suggested fix**: return `Err(CliError::Other("Daemon stop is only supported on Unix systems".to_string()))` in the `#[cfg(not(unix))]` block.
- **Decision**: FIXED in this round.

### [Medium] `aleph info` silently degrades when `system.info` or `providers.list` fails

- **Location**: `src/commands/info.rs:48-56`
- **Trigger condition**: `system.info` or `providers.list` returns an error (e.g. method not found on an older daemon, partial outage). The command calls `.unwrap_or(serde_json::json!({}))` for both, prints whatever is available, and exits `0`.
- **Expected behavior**: at minimum, a total failure of these calls should be reported; treating it as an empty object makes a broken or mismatched daemon look healthy.
- **Actual behavior**: errors are swallowed and rendered as missing sections.
- **Suggested fix**: propagate the first non-health RPC error rather than substituting an empty object. If partial degradation is intentional, emit the error to stderr and exit non-zero when both optional calls fail.
- **Decision**: REPORTED ONLY — this is a deliberate "graceful fallback" design; changing it to a hard failure would break the common case of checking health on a minimal/older daemon. A follow-up could add an explicit degraded warning.

### [Low] Secret and provider API-key values are visible in process listings when passed as flags

- **Location**: `src/commands/secret_cmd.rs:43-44`, `src/commands/providers_cmd.rs:308-314`
- **Trigger condition**: `aleph secret set OPENAI_API_KEY --value sk-...` or `aleph providers add ... --api-key sk-...`. The flag value is part of `argv`, so any user with `ps` visibility can read it.
- **Risk**: credential leakage to other users/processes on the same host and to shell history.
- **Mitigation**: both commands already default to a hidden TTY prompt (`rpassword`) and the provider command documents the `ALEPH_PROVIDER_API_KEY` env var. There is no way to hide a command-line argument from `ps`; the only real fixes are (a) remove the `--value` / `--api-key` flags entirely, or (b) add a runtime warning when they are used.
- **Decision**: REPORTED ONLY — changing the flag surface is a UX/product call.

### [Low] `aleph open` on Windows passes the URL through `cmd /C start` unquoted

- **Location**: `src/commands/open_cmd.rs:87-93`
- **Trigger condition**: a gateway URL containing shell metacharacters (e.g. `&`) could be interpreted by `cmd` as a command separator or redirection.
- **Risk**: on Windows, a malicious or accidentally-crafted server URL can execute unintended commands.
- **Mitigation**: quote the URL argument or use `ShellExecute`/`start` via a native launcher. The current implementation matches common cross-platform `open` patterns but is the weakest platform arm.
- **Decision**: REPORTED ONLY.

### [Low] `aleph config edit` does not split `$EDITOR` into command and arguments

- **Location**: `src/commands/config_cmd.rs:107-109`
- **Trigger condition**: a user sets `EDITOR="code --wait"` or similar with arguments.
- **Risk**: `Command::new(&editor).arg(&config_path)` will try to execute a binary literally named `code --wait`, which fails.
- **Mitigation**: split `EDITOR` on whitespace and use the first token as the program, appending the config path as the final argument.
- **Decision**: REPORTED ONLY — one-line fix, but it changes the `EDITOR` contract and is not a correctness bug.

## Per-perspective findings

### Security

- **No shell injection in command-passing paths**: `aleph sandbox run`, `aleph plugin pack`, and `aleph plugin init` do not invoke a shell; arguments are passed as separate `Command` args. `aleph hooks add --command` forwards the string to the daemon over JSON-RPC; execution happens server-side under the hook dispatcher.
- **Path traversal**: `identity set --file`, `session export --output`, and `plugin install <path>` accept filesystem paths but do not canonicalize or restrict them. These are operator-controlled paths; no privilege escalation is possible from the CLI itself. The one exception is `ALEPH_SERVER_BIN`, which `daemon start`, `sandbox run`, and `doctor` use to locate and execute the server binary — a malicious env var would run attacker code, but the env var is operator/trusted.
- **Subprocess trust**: `doctor.rs` and `plugin_cmd.rs` invoke `node`, `npm`, `rustup`, `curl`, and the server binary by name/PATH. These are trusted toolchain binaries; no untrusted user input is interpolated.
- **Credential handling**: as noted above, flag-passed secrets are visible in `ps`. The interactive prompt path is sound.

### Logic / state machines

- **Command dispatch state machine**: `cli_args.rs` enumerates every subcommand; `main.rs` exhaustively matches `Commands` and delegates to per-action dispatch helpers. No stale arms or unreachable variants were found.
- **Spinner lifecycle**: `output::spinner::Spinner` spawns a cleanup task and uses `Drop` + `Notify` to clear the line. It is a no-op when stderr is not a TTY. No spinner is constructed in `--json` mode.
- **Streaming Markdown boundary logic**: `stream_md::last_block_boundary` toggles fence state on lines starting with `` ``` `` or `~~~`; mismatched fence markers cause the renderer to hold everything until end-of-stream, which is the safe direction (no split inside a fence).
- **Watch filter**: `watch::Board::admits` binds run IDs to session keys on `RunAccepted` and filters subsequent events correctly. Runs already in flight when `watch` starts are hidden under a filter because their session is unknowable client-side — documented and correct.
- **`run_follow` pin-by-run-id**: foreign concurrent runs on the broadcast bus are filtered out by matching `event.run_id()` against the accepted run; this prevents interleaving.

### Error propagation

- **Most commands**: use `?` consistently and map malformed JSON params with `CliError::Other`.
- **`gateway call`**: always emits raw JSON; the `json_mode` parameter is intentionally ignored (raw RPC is machine-only). Documented in code.
- **Fixed gaps**: `run_follow`, `doctor --fix`, `channels list`, `cron list`, `memory dreaming`, and `daemon stop` now propagate errors correctly.

### Output / rendering

- **Markdown table cells drop inline styles**: `markdown::Renderer::push_styled` is not called for text inside table cells; emphasis/strong inside a table renders unstyled. This is a minor rendering bug but safe.
- **CJK alignment**: `output::width` uses `unicode-width` consistently; `print_table` and `markdown::render_table` pad before applying ANSI escapes, so alignment is correct.
- **Icon fallback**: `icon::use_unicode` honors `ALEPH_ASCII`, `LC_ALL`, `LC_CTYPE`, and `LANG`; defaults to Unicode on modern terminals. Tests serialize env mutations.
- **Color gate**: `theme::use_color` honors `NO_COLOR`, `ALEPH_COLOR`, and TTY detection. Good.

### Architecture (R1–R10)

- **R1**: No platform APIs in core — the CLI uses `#[cfg]`-gated platform launchers only in `open_cmd.rs`.
- **R4**: PASS. The CLI is a thin I/O layer. The largest local-logic module is `plugin_cmd.rs` (scaffolding/validation/packing), which is dev-tool local I/O, not business logic.
- **R7 / R8**: No regex-based intent parsing; `tools::run` filters by substring and `providers_cmd` uses a shared protocol-side ranker.
- **R9**: Every configurable knob is exposed through server-side tools; CLI flags map 1:1 to JSON-RPC params.
- **R10**: Thin harness; the CLI adds no middleware between user and model/server.

## Suggested tests (missing coverage)

1. **`run_follow` returns `Err` on `RunError`**: add a test feeding a `RunError` event and asserting the returned `CliError` contains the run's error message.
2. **`channels list`, `cron list`, `memory dreaming` malformed-response handling**: feed an unparseable JSON envelope and assert the command returns `Err` rather than printing an empty table.
3. **`daemon stop` non-Unix behavior**: compile-time only; the `#[cfg(not(unix))]` arm now returns `Err`, which can be asserted in a unit test on a non-Unix host.
4. **`secret set`/`providers add` flag-value leakage guard**: add a source-level test or clap check that `--value`/`--api-key` lack `hide`? Not enforceable at runtime; better as a security note.
5. **`markdown` table with inline emphasis**: add a regression test for `| *x* |` rendering with SGR sequences inside a padded cell.

## Conclusion

`interfaces/cli` is in good shape for a reference client: the command catalogue is fully wired, the output helpers are well-tested and CJK-aware, and the architecture redlines are respected. The dominant risk class was **error swallowing** — run failures and malformed JSON-RPC responses being rendered as success or empty output. All five found instances of silent failure have been fixed in this round. The remaining findings are security/usability notes that either require product decisions (secret flags, Windows URL launching) or are low-impact (editor splitting, partial `info` fallback).
