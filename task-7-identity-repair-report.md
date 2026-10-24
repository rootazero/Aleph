# Task 7 — Bounded identity repair / 限定类型身份修复

**Status / 状态：bounded repair verified; Task 7 NOT ACCEPTED / 本次修复验证通过，不构成 Task 7 整体验收。**

## 1. Root cause and authority / 根因与权威

- Root: `.`; branch: `refactor/modularization-p0`; HEAD: `fac11b6de66b7051f9aa102428170f4fd6c59cfe`.
- Supplied read-only diagnosis inspected: `/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/acp-delegate/del_mv978ieq_pdwz.out`. No subagent was dispatched by this repair.
- Original failed run: `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-150800-task7-core-check-b1b2.log`: `cargo check -p alephcore --lib --bins`, probe exit 0, `available_bytes=6639403008 required_bytes=4294967296 ALLOW`, cargo exit 101, 11 errors (5 E0308 and 6 E0609), no timeout or surviving descendants.
- Verified original HEAD `src/providers/adapter.rs:10` imports `crate::tool_metadata::ToolDefinition`; original `RequestPayload<'a>` has ten fields and borrowed tools. Current `src/tool_metadata/types/definition.rs:20` reexports `aleph_slot_api::metadata::ToolDefinition` (LLM schema `parameters`). Current `src/providers/adapter.rs:14` reexports API `RequestPayload`.
- The migrated API `crates/aleph-slot-api/src/ai.rs:22` instead imported `crate::tool::ToolDefinition` (execution schema `input_schema`). Thus the 11 failures arose at existing metadata callers and provider `.parameters` consumers, not from a missing third DTO. Source-review §3 and §10 explicitly require the Task2 metadata identity.
- Correction / 更正：the original identity-preservation assertion in `task-7-phase-a-report.md` §3 was wrong and has been replaced with the exact distinction and this report link. Historical Phase A counts/results remain historical, not fresh evidence.

## 2. Exact changes and trade-off / 实际修改与取舍

Only these deliverable files were edited/created by this repair:

1. `crates/aleph-slot-api/src/ai.rs`: change `use crate::tool::ToolDefinition;` to `use crate::metadata::ToolDefinition;`; add `request_payload_borrows_nonempty_metadata_tools` at line 631.
2. `task-7-phase-a-report.md`: correct the erroneous original-identity assertion in §3.
3. `task-7-identity-repair-report.md`: this evidence report.

The new API unit test builds a nonempty local metadata tool array with a literal `parameters` JSON schema, assigns it to a ten-field `RequestPayload` literal, and passes its borrowed slice through `RequestPayload::with_tools`. Typed assignments through the existing `crate::MetadataToolDefinition` alias and `crate::metadata::ToolDefinition`, slice pointer equality, length/name and literal schema assertions verify the same borrow and schema survive without conversion.

Production code before `#[cfg(test)]` was compared byte-for-byte with the pre-edit snapshot after allowing exactly the import replacement. All ten fields, lifetimes, default, exact `with_tools` signature/body and all other production bodies are unchanged. No provider adapter, harness, gateway or Core source file was changed by this repair; no third DTO or conversion was added.

Trade-off / 取舍：retain the two existing same-named types with distinct owners rather than redesigning their names or semantics. Future maintainers must continue distinguishing metadata `parameters` from execution `input_schema`.

## 3. Fresh verification / 本次验证证据

Direct scoped formatting and whitespace checks:

```sh
rustfmt --edition 2021 --config skip_children=true crates/aleph-slot-api/src/ai.rs
rustfmt --check --edition 2021 --config skip_children=true crates/aleph-slot-api/src/ai.rs
git diff --check
```

All exited 0. The root diff check covers tracked changes; API `ai.rs` is currently untracked and is separately covered by direct rustfmt. No workspace formatting was run.

Exactly two Cargo invocations, serial and foreground, in this order; outer bash tool timeout **960s each**. Both use the unchanged guard with `--timeout-s 900`, a fresh enforcing probe (minimum **4294967296** bytes), and `CARGO_BUILD_JOBS=2`:

```sh
python3 -B .superpowers/sdd/2026-10-14-modularization-p0-implementation/guarded-cargo.py --timeout-s 900 task7-identity-repair-api-lib-20261015-151437-35050 -- test -p aleph-slot-api --lib
python3 -B .superpowers/sdd/2026-10-14-modularization-p0-implementation/guarded-cargo.py --timeout-s 900 task7-identity-repair-core-lib-bins-20261015-151443-35217 -- check -p alephcore --lib --bins
```

| Run / 验证 | Fresh probe | Cargo result | Timeout / descendants | Full log / 完整日志 |
|---|---|---|---|---|
| API lib tests | exit 0; available 7302660096 bytes; required 4294967296; ALLOW | exit 0; **262 passed, 0 failed, 0 ignored, 0 filtered out**; new `ai::tests::request_payload_borrows_nonempty_metadata_tools ... ok`; elapsed 2.4s | `timed_out=False descendants_remaining=0` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-151437-task7-identity-repair-api-lib-20261015-151437-35050.log` |
| Core lib + bins check | exit 0; available 7154221056 bytes; required 4294967296; ALLOW | exit 0; elapsed 117.2s; original 11 errors absent; check only, **no Core tests executed** | `timed_out=False descendants_remaining=0` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-151443-task7-identity-repair-core-lib-bins-20261015-151443-35217.log` |

Core still emits **14 unused-import warnings**. Their complete warning-detail block was compared with the failed baseline and is identical. No warning cleanup was attempted. No unexpected failure occurred, so the stop-on-first-unexpected-failure condition was not triggered.

A pre-edit snapshot covered 7658 tracked/untracked nonignored paths; existing-file hashes changed only for the permitted `ai.rs` and Phase A report. The staged binary-diff SHA-256 remains `eaaaaa3129a40f7b53eea2619f9cca26c02d2ac5c2812e0ba8c01c6e4af94705`. Existing unrelated modifications and staged entries were preserved. Guard-generated logs are evidence artifacts, not additional source edits.

## 4. Explicit negatives and remaining limits / 未做与剩余边界

- No extra RED/mutation Cargo run was performed: followed the requested test-then-check sequence after applying the bounded repair. The regression test is freshly green, but this report does not claim an observed RED/GREEN cycle.
- No API→Core backedge was added. API-local typed assignments prove the metadata module/alias identity; an executable old-Core↔new-API identity test cannot live in this API module without that backedge and was not added elsewhere. Core production call-site compatibility is freshly compiler-checked, not a runtime TypeId assertion.
- No provider request/schema runtime regressions, Core unit/integration tests, external consumer/example tests, full workspace tests, clippy, cross-platform checks, benchmarks, runtime wiring or full Task 7 acceptance were performed/claimed.
- No changes to manifests, dependencies, provider adapters, harness, gateway, guard/probe, other source files, or unrelated work. No subagents, staging, unstaging, commit, reset, push, install or graph operation.
