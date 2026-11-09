# Spec C regression gate — rebuild `scripts/spec_c_regression.sh` as an AST gate

**Round date**: 2026-11-09
**Base commit**: the round opened at `2dad43374`; six unrelated
`src/exec` / `src/executor` / `src/export` commits landed in parallel while it
ran (no file this round touches was among them)
**Trigger**: user instruction, 2026-11-09 — "开新一轮修 scripts/spec_c_regression.sh"

---

## Why this round existed

The previous review round (`2dad43374`) closed out with the Spec C gate listed
as an open decision item: the script was **red on `main`**, **not wired into
CI**, and one of its four checks was **decorative**. This round repairs it.

### The gate as it was

`scripts/spec_c_regression.sh` (82 lines, authored 2026-06-24 *before* the code
existed — `docs/superpowers/plans/2026-05-02-memory-evolution-spec-c-cross-process-safety.md:3521-3576`
records the original greps verbatim) declared four invariants and checked them
with four text greps. On `main` it reported:

| check | result on `main` |
|-------|------------------|
| [1/4] SQLite opens route through `utils::sqlite_open` | ❌ 36 hits |
| [2/4] vault / `acp_sessions.json` only touched through `vault_io` / `atomic_io` | ❌ 1 hit |
| [3/4] every CLI subcommand declares a policy | ❌ 29 "unannotated" files |
| [4/4] no leftover `acquire_instance_lock` callers | ✅ (vacuous — see below) |

EXIT=1. Three concrete defects:

1. **A grep cannot tell production code from test code.** Both live in the same
   files: `#[cfg(test)] mod tests { … }` blocks sit inside production modules,
   and ~40 test-only files carry `*_tests.rs` names. The script's filters could
   not fix this: `grep -v "^\s*//"` can never match `git grep -n` output (whose
   lines start `path:line:content`, never whitespace), and `grep -v "tests/"`
   matches *paths* only, so it missed inline `#[cfg(test)]` modules entirely.
   Of the 37 reported hits, **one** was a production violation.
2. **Check [3/4] was satisfiable by a comment.** It grepped for the *text*
   `with_policy\|run_no_lock\|with_policy_owned`. `with_policy_owned` — the
   function the design doc named — **does not exist anywhere in the repo**; it
   appears only in three `//!` comments under `src/bin/aleph-server/commands/start/**`.
   `run_no_lock` matched a pass-through whose body is `f()`
   (`src/cli/policy.rs:104-109`). The check therefore asserted nothing about
   the code, and its scope was wrong: it swept the whole `commands/` tree,
   including the `start/` server-boot subtree, whose singleton lock is taken in
   `main()` *before* `fork()` (`src/bin/aleph-server/main.rs:93-119`).
3. **Check [4/4] was vacuous.** `acquire_instance_lock` no longer exists
   anywhere in `src/` — the design doc said to delete it "in the next spec", and
   it was. The check could never fire, and the invariant it *meant* to protect
   (one owner of the singleton lock) was unguarded.

`src/utils/spec_c_audit.rs` was also still excluded by name in two checks; that
file was deliberately dropped in commit `06cf4a3aa`.

### Decisions taken (with the user)

| question | decision |
|----------|----------|
| vehicle | **A Rust integration test** (`tests/spec_c_invariants.rs`) using the repo's existing `syn` dev-dependency — house pattern: `tests/modularization_import_guards.rs`. `scripts/spec_c_regression.sh` stays as a thin wrapper so the documented path keeps working; CI picks the test up via the existing `cargo test -p alephcore --features test-helpers --test '*' --no-fail-fast` step (no workflow edit). |
| check [3/4] | **Declaration + code agreement**: every CLI subcommand module carries a machine-readable `//! lock-policy: …` line, and the AST must agree with the code in that module. |

---

## The new gate

`tests/spec_c_invariants.rs` parses every `.rs` under `src/` with `syn` 3.0.9
and enforces the four invariants against the **syntax tree**, not the text.

| # | invariant | how it is decided |
|---|-----------|-------------------|
| 1 | no raw SQLite open | flags a call whose callee path ends in `Connection::open` / `open_with_flags` / `open_with_flags_and_vfs`; `Connection::open_in_memory` is allowed (no cross-process concern); `src/utils/sqlite_open.rs` is the sanctioned owner |
| 2 | protected files are written atomically | flags a raw `fs` **write** op (`write`, `create`, `remove_file`, `remove_dir`, `remove_dir_all`, `rename`, `copy`, `set_permissions`) whose target path contains `"secrets.vault"` / `"acp_sessions.json"`, or a `let`-binding tainted by such a literal. Plain **reads** are allowed — atomic-rename makes a plain read safe |
| 3 | every CLI subcommand declares its lock policy | the subcommand set is derived from the AST of `commands/mod.rs` (`start` excluded: its lock is taken in `main()`); each module must declare `//! lock-policy: none \| with-policy \| run-no-lock` **and** the declaration must agree with the code in **both** directions (`with-policy` ⇒ a `with_policy`/`try_with_policy` call, `run-no-lock` ⇒ a `run_no_lock` call, `none` ⇒ neither — and a policy call the declaration does not cover is a violation too) |
| 4 | one owner of the singleton lock | the lock file-name literal may appear only in `src/utils/instance_lock.rs`; `instance_lock::try_acquire` may be called only from `src/bin/aleph-server/main.rs` and `src/cli/policy.rs`; the deleted `acquire_instance_lock` must not come back |

**Test code is identified structurally**, in three independent ways: a
three-valued `cfg` evaluation (`cfg(test)` / `cfg(all(test, unix))` ⇒ test;
`cfg(not(test))` ⇒ not test; `cfg(any(test, …))` ⇒ not test), a `#[test]` /
`#[tokio::test]` attribute, and a file declared under a `#[cfg(test)] mod X;`
(resolved with Rust's module-directory rule — for `dir/name.rs` the children
live in `dir/name/`, for `dir/mod.rs` they live in `dir/`). Doc comments are
`#[doc = "…"]` attributes, so prose can never trigger a literal rule.

### The `//! lock-policy:` declaration

One source of truth per module — the declaration replaces the old prose
("Spec C policy: **NoLock**") rather than duplicating it. The rationale prose
is kept; only the duplicated statement of the fact was removed.

| class | modules |
|-------|---------|
| `run-no-lock` | `bootstrap_runtime`, `gateway`, `plugins`, `resume` |
| `with-policy, run-no-lock` | `secret` |
| `none` | `bootstrap_token`, `doctor`, `hooks`, `identity`, `node`, `pair`, `prompt_size`, `sandbox_debug`, `service`, `update` |

`none` is the honest answer for the ten: none of them takes the singleton
lock. `doctor`'s `StaleLockCheck` repair writes to the data dir but only after
checking that the lock is free; `pair` deliberately performs one INSERT into
`security.db` (WAL + busy_timeout) so it works with or without a live daemon
(`src/bin/aleph-server/main.rs`); `hooks` writes `~/.aleph/shell-hooks-allowlist.json`,
outside the data dir; `service` writes user units; `node` is the cluster runtime
and touches no data dir.

### Production violations the new gate found

Twelve raw SQLite opens in production code, in ten files — invisible to the old
grep because they sat in test blocks and test files, and because the grep
counted all 37 hits without distinguishing them:

* **read-write** ⇒ `open_sqlite_safe` (WAL + `busy_timeout=5000` +
  `synchronous=NORMAL` + `foreign_keys=ON`): `src/bin/aleph-migrate/export.rs:87`,
  `src/bin/aleph-migrate/index.rs:32`, `src/bin/aleph-migrate/verify.rs:28` and
  `:122`, `src/context/retrieval/content_index.rs:127`, `src/hub/cache.rs:147`,
  `src/memory/store/markdown/store_impl.rs:62`.
* **read-only** ⇒ `open_sqlite_readonly` (`READ_ONLY|NO_MUTEX` + `busy_timeout`;
  a read-only open cannot set `journal_mode`): `src/diagnostics/checks/cache_health.rs:49`,
  `src/diagnostics/checks/cache_hit_rate.rs:59`,
  `src/diagnostics/checks/fold_economics.rs:285` and `:331`,
  `src/gateway/interfaces/imessage/db.rs:162`.

The invariant-2 violation the old gate reported (`src/gateway/handlers/projects.rs:2474`
writing `secrets.vault`) was **test code** — the module's `#[cfg(test)] mod tests`
starts 1,200 lines earlier. The production vault/acp invariant already held; only
the script's filter was broken.

Two supporting changes fell out of the same investigation:

* `src/utils/sqlite_open.rs` — the pragma order was `journal_mode=WAL` **before**
  `busy_timeout=5000`; the WAL conversion needs an exclusive lock, so a busy
  database failed the conversion instantly. `busy_timeout` now comes first.
* `src/utils/instance_lock.rs` — `HOLDER_FILENAME` is now `pub(crate)` and
  `src/diagnostics/checks/stale_lock.rs` imports it instead of re-declaring the
  literal `"aleph.lock.pid"`, so the lock file name has exactly one owner in the
  tree (which is what invariant 4 asserts).

### Two semantic consequences of the helper swap

* `open_sqlite_safe` sets `foreign_keys=ON`, which the raw opens did not. The
  two stores that gained it are FK-safe by insert order — the markdown store
  inserts `agents` → `workspaces` → `sessions`, the migrate tool does the same —
  and the sibling SQLite memory store (`src/memory/store/sqlite/mod.rs:112`)
  already ran with `foreign_keys=ON` and the same schema, so the markdown store
  now matches it rather than diverging.
* The read-only diagnostics (`cache_health`, `cache_hit_rate`, `fold_economics`)
  previously *created* an empty database when the file was missing; a read-only
  open fails instead. The behaviour the user sees is unchanged, because each of
  those checks guards the open with `Presence::of(...)` and answers "no
  database yet" without opening anything.

### Bugs the first run found

The gate was debugged against the tree, not written and trusted. The first
standalone run failed two of the four tests and reported eleven SQLite hits
that were not violations at all — every one of them test code:

| bug | symptom | fix |
|-----|---------|-----|
| `#[cfg(test)]` was never detected | the `cfg` matcher keyed on the *list's* path name, so `#[cfg(test)]` (a list, not a path) fell through to "not test" ⇒ every inline test module was scanned as production | a `cfg` arm with `any` semantics (`cfg(a,b)` means a **and** b, so it implies test iff one predicate does) |
| the module-directory rule was wrong | `mod resource_matrix;` inside `src/gateway/channel_registry.rs` resolved to `src/gateway/resource_matrix.rs` instead of `src/gateway/channel_registry/resource_matrix.rs` ⇒ the `#[cfg(test)]` declaration never attached and the test-only file was scanned | `module_dir()`: for `dir/name.rs` the children live in `dir/name/`, for `dir/mod.rs` / `lib.rs` / `main.rs` they live in `dir/` |
| the walk ran once per rule | 129 s per run, because each of the four tags re-walked every file | one candidate filter (a text superset of every AST trigger) and one walk per candidate, collecting all tags at once ⇒ 34 s |

The five *real* violations the corrected gate then reported were the five
read-only opens listed above — they had been hidden in the noise.

### Proof that the gate fires

A gate that cannot go red is decoration, so every invariant was verified by
injecting a violation and observing the failure:

| injected | gate result |
|----------|-------------|
| `rusqlite::Connection::open(path)` in a production file | [1/4] FAILED — "`Connection::open` bypasses `utils::sqlite_open`" |
| `std::fs::write(vault_path, …)` where `vault_path` is tainted by `"secrets.vault"` | [2/4] FAILED — "`write` writes a protected file" |
| a new `pub mod probe_cmd;` whose file carries no declaration | [3/4] FAILED — "no `//! lock-policy: …` declaration" |
| a module declaring `none` while calling `run_no_lock` | [3/4] FAILED — "declares `none` but calls the lock-policy API (run_no_lock)" |
| a module declaring `with-policy` while also calling `run_no_lock` | [3/4] FAILED — "calls `run_no_lock` but does not declare `run-no-lock`" |
| the literal `"aleph.lock"` outside `instance_lock.rs` | [4/4] FAILED — "the singleton lock file name may only be written in src/utils/instance_lock.rs" |

All probe files were removed and the patched files restored afterwards.

---

## Files changed

| file | change |
|------|--------|
| `tests/spec_c_invariants.rs` | **new** — the AST gate (four `#[test]`s) |
| `scripts/spec_c_regression.sh` | reduced to a thin wrapper over the test; the header now explains why a grep could not work |
| `src/utils/sqlite_open.rs` | `busy_timeout` before `journal_mode` |
| `src/utils/instance_lock.rs` | `HOLDER_FILENAME` is `pub(crate)` (one owner of the name) |
| `src/diagnostics/checks/stale_lock.rs` | imports `HOLDER_FILENAME` instead of re-declaring it |
| `src/diagnostics/checks/{cache_health,cache_hit_rate,fold_economics}.rs` | read-only opens → `open_sqlite_readonly` |
| `src/gateway/interfaces/imessage/db.rs` | read-only open → `open_sqlite_readonly` |
| `src/bin/aleph-migrate/{export,index,verify}.rs` | raw opens → `open_sqlite_safe` (`index.rs` also checkpoints the WAL back into the file it renames into place) |
| `src/context/retrieval/content_index.rs`, `src/hub/cache.rs`, `src/memory/store/markdown/store_impl.rs` | raw opens → `open_sqlite_safe` |
| `src/bin/aleph-server/commands/**` (15 modules) | `//! lock-policy:` declaration; the duplicated "Spec C policy: **NoLock**" prose removed, the rationale kept |
| `docs/reference/SECURITY.md`, `docs/reference/PROCESS_MANAGEMENT.md` | both cited the script as the enforcement of the four invariants — updated to describe the AST gate and the declaration |

## Performance

The first working version took **129 s**: `syn::File` is not `Send`/`Sync`
(`Span` is `!Send`), so the parsed tree could only live in a thread-local cache,
and each of the four test threads re-parsed all of `src/`. The gate was
restructured to hold **plain data** in a process-wide `OnceLock` — the parsed
tree is analysed once per file into `FileAnalysis` (findings, policy calls,
declared submodules, module doc), and the module graph and the test-only fixed
point are computed from that.

The AST walk is also restricted to **candidate files** — 290 of the 2,811
`.rs` files under `src/` contain any token a rule keys on — and each candidate
is walked **once**, collecting all four tags. Result: **34 s** for all four
tests.

## Verification

```
$ cargo check -p alephcore --all-targets -j1     # memory-capped, see below
EXIT=0 — 0 errors, 21 warnings (all pre-existing dead-code)
    Finished `dev` profile [unoptimized] target(s) in 15m 43s
```

The warnings are the repository's pre-existing dead-code baseline (`lib` 4,
`lib test` 2, two test targets 3, `bin "aleph-server"` 12) — nothing this
round added.

Memory-capped per the box's profile (20 GiB, no swap): `flock` on
`/home/zou/data/workspace/.r11-cargo.lock` held on a file descriptor for the
whole run, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `-j1`.

The gate itself was verified with a standalone `rustc --test` build against the
already-compiled `syn` / `proc-macro2` rlibs in `target/debug/deps` — seconds
per iteration instead of a full cargo build. That probe is what found the
three gate bugs listed below, and it runs the same four tests the CI step will
run.

The unified check compiled the **final** version of the gate: the source file's
mtime is 10:18:00 and cargo produced
`target/debug/deps/libspec_c_invariants-40bd094db0a19046.rmeta` at 10:29:20,
inside the same run.

## Deliberately NOT done

* **No CI workflow edit.** The gate is picked up by the existing
  `cargo test -p alephcore --features test-helpers --test '*' --no-fail-fast`
  step (`.github/workflows/aleph-core-ci.yml:338` — the `--test '*'` glob runs
  every integration test target). (The old script was in no workflow at all;
  the test is in one.)
* **No `cargo test` / `cargo clippy` run** — this round was budgeted one
  unified `cargo check`, and the gate itself was run directly.
* **The fingerprint derivation** and the **node file-jail parent-directory
  window** (from the previous round) are untouched — both need changes outside
  the scope of this gate.
* **`Connection::open_in_memory`** is allowed by design: an in-memory database
  has no cross-process story.
* **Plain reads of the protected files** are allowed by design: atomic rename
  makes a torn read impossible, and the read path is what
  `vault_io`/`persistence.rs` already do.
