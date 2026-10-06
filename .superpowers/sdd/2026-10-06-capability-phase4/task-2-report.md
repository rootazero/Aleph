# Task 2 Report — Capability Phase 4 (Gate B)

- Date: 2026-10-06
- Workdir: `/Volumes/TBU4/Workspace/Aleph-capability-phase4`
- Branch: `capability-phase4`
- Package: `alephcore`
- Status: COMPLETE

## Summary

Task 2 (Gate B) layers the behavioural `OwnershipTree` on top of the Task 1
pure-type substrate. The five-deep ownership tree
(`Runtime → Session → Run → Task → EffectClaim`) now has working
`bump` / `revoke` / `dispose` operations, plus the supporting
`register` / `resolve` / `claim` / `claim_state` / `is_revoked` /
`is_disposed` surface the tests pin. The three Gate-B tests are green
and `cargo check -p alephcore` compiles cleanly.

## Files changed

| File | Change |
|------|--------|
| `src/capability/ownership.rs` | Task 1 pure types preserved as-is; appended `BindingKey` / `Binding` / `OwnershipInner` / `OwnershipTree` impl + 3 tests in `#[cfg(test)] mod tests`. |

No other files modified. `git status` before commit:

```
 M src/capability/ownership.rs
```

## Commands run

### 1. Whitespace / diff hygiene

```
git diff --check
```

Result: clean (no output — no whitespace errors, no conflict markers).

### 2. Targeted test (Gate B deliverable)

```
cargo test -p alephcore capability::ownership --no-fail-fast
```

Result:

```
running 3 tests
test capability::ownership::tests::same_id_two_scopes_coexist ... ok
test capability::ownership::tests::bump_invalidates_child_claims ... ok
test capability::ownership::tests::revoke_and_dispose_are_irreversible ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 20704 filtered out; finished in 0.00s
```

All three Gate-B tests pass:

- `bump_invalidates_child_claims` — bumping `Session` invalidates a
  `Task`-bounded child binding's claim (transitions `Active → Unknown`).
- `same_id_two_scopes_coexist` — the same `CapabilityId` registered under
  two distinct `VisibilityScope`s resolves under both independently.
- `revoke_and_dispose_are_irreversible` — revoke removes every binding
  for the id, marks claims `Unknown`, refuses subsequent claims, and
  remains revoked across a second revoke; dispose wipes the lifetime and
  refuses subsequent claims at that scope or below.

### 3. Compile check

```
cargo check -p alephcore
```

Result: `Finished dev profile [unoptimized + debuginfo] target(s) in 1.23s`
— compiles cleanly. **One pre-existing-style warning** (recorded below,
not fixed per Task 2 scope: no implementation edits allowed).

## Warning recorded (carried forward, NOT fixed in this commit)

```
warning: field `visibility` is never read
   --> src/capability/ownership.rs:224:5
    |
221 | struct Binding {
    |        ------- field in this struct
...
224 |     visibility: VisibilityScope,
    |     ^^^^^^^^^^
    |
    = note: `Binding` has a derived impl for the trait `Debug`, but this is intentionally ignored during dead code analysis
    = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `alephcore` (lib) generated 1 warning
```

The `Binding::visibility` field is stored on every registered binding but
is never read after insertion. The brief explicitly forbids modifying
the implementation in this task, so the warning is **recorded here, not
fixed**: resolution currently keys on `format!("{visibility:?}")`
already taken at insert time (see `BindingKey::visibility`); the
`Binding::visibility` field is therefore a redundant copy and a
candidate for `#[allow(dead_code)]` annotation or removal in a
follow-up commit. Decision: leave to a later cleanup task so this
commit remains a pure append to the file as specified.

## Design notes

1. **Storage key** — `BindingKey` is `(CapabilityId, format!("{visibility:?}"))`.
   Two registrations under different `VisibilityScope`s therefore live
   under distinct keys (the `same_id_two_scopes_coexist` test exercises
   exactly this).
2. **Rank order** — `External < Task < Run < Session < Runtime`, so
   `bump(Session)` rewrites every binding whose lifetime is at-or-below
   `Session` (i.e. `Task` and `Run` children get re-issued; `Runtime`
   parent survives). `LifetimeScope` gains `PartialOrd`/`Ord` impls
   deriving from `rank`, leaving the existing pure-type semantics
   intact.
3. **Irreversibility** — `revoke` records the id in a `HashSet` and
   drops every binding under that id; `is_revoked` stays true forever
   and `register`/`claim` refuse new work. `dispose` records the scope
   and drops every binding at-or-below that scope; `register`/`claim`
   refuse new work at-or-below the disposed scope. The "second revoke
   / second dispose is a no-op" assertions in the test lock this in.
4. **Claim invalidation on `bump`** — `bump` clears the binding's
   `claims` set so any future `claim_state` lookup misses the fence and
   returns `Unknown`. The fresh nonce (`inner.nonce.saturating_add(1)`)
   is also stamped as the binding's new `generation`, so a
   hypothetical generation-comparison reader would observe the bump.
5. **Concurrency** — single `Mutex<OwnershipInner>` around all state;
   `FencingToken` is `Copy` so `HashSet<FencingToken>` membership tests
   stay cheap. No `RwLock` — the operations are short and write-heavy
   (`bump` mutates every binding).

## Diff scope

```
$ git diff --stat
 src/capability/ownership.rs | (Task 2 append)
```

Only `src/capability/ownership.rs` is touched. No `census/`, `plan/`,
`spec/`, `harness/`, or other capability file modified. No `Cargo.toml`
change.

## Commit

```
git add src/capability/ownership.rs .superpowers/sdd/2026-10-06-capability-phase4/task-2-report.md
git commit -m "feat(capability): add ownership tree with scope separation (Gate B)"
```

## Not done (by design)

- `Binding::visibility` dead-code warning suppression / field removal —
  separate cleanup commit; the Task 2 brief forbids implementation
  edits.
- `CapabilityBackend::generation()` still returns `OwnerGeneration(0)`
  placeholder (carried over from Gate A's fix round); wiring it to read
  from `OwnershipTree`'s real monotonic nonce is a follow-up
  integration task.
- Schema fingerprint computation (`[0u8; 32]` placeholder) — describe /
  projection path, later task.
- Backends for the other 8 capability kinds — deferred, non-invocable.
- `Zahir` concrete impl — trait + transport types landed in Task 1;
  wiring belongs to a later task.
