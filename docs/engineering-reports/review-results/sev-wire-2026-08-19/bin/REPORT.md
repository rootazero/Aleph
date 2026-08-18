# Severed-wire audit — `src/bin/` (2026-08-19 round)

Scope: `src/bin/aleph-server/` (`{main.rs, cli.rs, server_init.rs, daemon.rs, Info.plist, commands/*}`),
strict cross-crate budget.

Method: skill methodology — 7 seam lenses (registration parity, call-vs-handler,
classifier-vs-handler, event emit-vs-subscribe, config-reader, path/route,
stub sweep). Read-first triage per `triage-playbook.md`.

## Module map

The `bin` crate is intentionally thin: parse CLI args, dispatch to
`start_server` / `daemon` / `bootstrap-token` / etc. Each `Command::Variant`
in `cli.rs` has a handler in either `main.rs::async_main` or `commands/`.
There is no plugin-spawning, no business logic — almost nothing for severed
wires to hide behind.

## Findings

### CUT (1)

- **`bin-bootstrap_token-01` CUT (low)** — `src/bin/aleph-server/commands/bootstrap_token.rs:19`
  `pub fn read_token_from_db(db_path: &Path, data_dir: &Path) -> Option<String>`
  was `pub` but has **zero external callers** in the whole workspace.
  Verified by `grep -rn "read_token_from_db" src/ interfaces/ shared/ desktop/`:
  only consumers are at lines 42 / 85 / 96 of the same file (line 42 in
  `handle_bootstrap_token`, lines 85 and 96 in the file-local `#[cfg(test)]`
  tests). Demoted `pub` → `fn` (private to the `commands` module).

## Already-clean surfaces (no action)

The 7 seam lenses produce no other findings. In particular:

- Every `Command::Variant` (BootstrapToken, Daemon, Service, Run, Chat,
  Hub, Plugins, Marketplace, Hooks, Secret, Identity, Plugin, etc.) has a
  matching `Some(Command::X) => ...` arm in `main.rs::async_main`. No
  ghost dispatch arms.
- Sub-action enums (`PluginsAction` / `GatewayAction` / `ServiceAction` /
  `HooksAction` / `SecretAction` / `IdentityAction` / `PluginAction` /
  `MarketplaceAction`) are exhaustively matched; no `unimplemented!()` /
  `todo!()` / stub handlers anywhere in the bin crate.
- Every `register_*_handlers` in `start/builder/` is wired through
  `start_server` (verified via direct call-graph tracing from
  `aleph-server run` → `start::server` → registered handlers).
- Every public `Args` field is read (config, daemon, pid_file, log_file,
  bind, port, force, log_level, max_connections, webchat_dir,
  webchat_port).
- All public helpers in `server_init.rs` (`serve_webchat`,
  `build_run_error_response`, `handle_run_with_engine`,
  `handle_chat_send_with_engine`) are reachable from request handlers.
- All public descriptor generators in `service/descriptors.rs` are used.
- `Info.plist` is embedded by `build.rs` via `-sectcreate __TEXT __info_plist`
  (not a stray file).
- `_workspace_manager` / `_provider_registry` underscore-prefixed params
  in `server_init.rs` are deliberate API-surface holders (signatures kept
  stable for caller wiring, body intentionally empty for now). Not dead
  code — they pin a public API contract.
- The single `TODO` hit in `orchestrator_init.rs:249` is a comment
  referencing a previously-completed task, not an open stub.

## Cross-cutting concerns

None. No `Cargo.toml` or top-level `src/lib.rs` changes required for this
audit.

## Punted / DECIDE

None.

## Almost-cut, kept

- The `pub` surface on `commands::bootstrap_token::handle_bootstrap_token`
  is legitimate — it's reached by the CLI dispatch table in `main.rs`
  via `mod.rs::pub use`.
- `server_init.rs` underscore-prefixed params: deliberately pinned
  API contracts, not severed wires.
