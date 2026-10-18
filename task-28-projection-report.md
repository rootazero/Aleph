# Task28 projection-only test repair

## Status

`DONE_WITH_CONCERNS` — the requested test-only repair is complete, with runtime/compile validation intentionally not performed.

## Scope and SHA256

Only these allowed paths were written by this repair:

- `src/capability/projection_host.rs` — test-only changes in `#[cfg(test)]`; SHA256 after rustfmt: `f8006d31954ea40322a6dc9d000e5a8ca66772a9cc5e45fcd7f94f6df9ade57e`
- `task-28-projection-report.md` — this report

The four unrelated Task28 source files were not modified. No production code, accessor, API, admission token, or replay bypass was added.

## Actual change

Replaced the old `already_running_invocation_is_not_cut` test, which revoked before directly invoking a captured admission wrapper and therefore did not prove an already-started canonical call, with a bounded real-dispatch scenario:

- Added a private `BlockingHandler` test fixture using an `AtomicUsize` call counter and `Notify` permits for loss-free entered/release coordination.
- Added a private caller helper that performs facade `resolve(Reference)`, facade `validate_lease(BackendLease)`, then real canonical dispatch through the same live registry using `LoopToolRegistry::bind_canonical_registry`, `McpRegistryTool::from_registry_entry`, `ScopedToolService`, `approval::with_call_identity`, and `service.execute`.
- The first call must signal entry from inside the handler before the test revokes the same ownership id.
- The captured lease and a fresh facade resolve are asserted to fail with `ResolveError::Revoked` after revoke.
- A second call through the same helper is rejected at fresh resolve before handler entry; the counter remains `1`.
- Releasing the first handler allows the in-flight call to return the original `{"tool":"a"}` output; the counter remains `1`.
- The outer scenario and each wait are bounded with Tokio timeouts; no sleep/yield ordering is used.

This test demonstrates composition of live lease validation with real canonical dispatch. It does not claim that `ScopedToolService` independently checks `OwnershipTree`.

## Direct verification performed

- `rustfmt --edition 2021 --config skip_children=true src/capability/projection_host.rs` — passed.
- `git diff --check` — passed.

## NOT TESTED AFTER REPAIR

Per instruction, no Cargo, rustc, or test command was run after the repair. The edited test therefore has no post-repair compile or runtime result in this report.
