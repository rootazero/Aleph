# Task 3: effect claim reconciliation reducer

## Implemented

- Added `src/capability/effect_claim.rs` with independent wire identity fields (`owner`, `owner_generation`, `fence`).
- Added fail-closed pure reconciliation over the legal Prepared → Claimed → Invoking → terminal chain.
- Added seven unit tests covering the requested cases.
- Exported the module from `src/capability/mod.rs`.

## Verification

Commands run:

- `cargo test -p alephcore capability::effect_claim --no-fail-fast`
- `cargo check -p alephcore`
- `git diff --check`

## Scope

The reducer does not replay, invoke handlers, or promise external exactly-once behavior. Unknown is terminal and fail-closed.
