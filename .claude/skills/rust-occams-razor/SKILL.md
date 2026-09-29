---
name: rust-occams-razor
description: Use when refactoring, simplifying, reviewing, or decomposing an existing Rust codebase while preserving observable behavior, public contracts, concurrency semantics, and performance characteristics.
---

# Simplifying Rust with Occam's Razor

## Overview

Treat simplicity as the removal of unnecessary concepts, not the reduction of line count. Prefer the smallest design that preserves behavior, invariants, ownership clarity, concurrency semantics, and measured performance.

Do not refactor merely because a construct can be made shorter. Every abstraction, split, merge, iterator chain, macro, generic, allocation, and indirection must earn its existence.

## Non-Negotiable Invariants

Before editing, identify the contracts that must remain stable:

- Public API surface, unless the task explicitly permits breaking changes.
- Error semantics: variants, conversion boundaries, propagation behavior, and externally observable messages where relied upon.
- Ownership/lifetime behavior that callers depend on.
- `Send`/`Sync` properties and async cancellation behavior.
- Lock acquisition/release ordering and critical-section boundaries.
- Ordering, timing-sensitive behavior, retry semantics, side effects, I/O, persistence, and protocol behavior.
- Feature-gated and target-specific behavior.
- Serialization/wire/storage formats and compatibility.
- Performance-sensitive allocation and dispatch characteristics unless benchmarks justify a change.

When uncertain whether behavior is observable, preserve it.

## Evidence Before Deletion

Never equate “no obvious reference” with dead code. Before deleting code, check relevant evidence such as:

```bash
cargo check --all-targets --all-features
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Also inspect, where applicable:

- integration tests, examples, benches, build scripts and proc macros;
- `cfg`/feature/platform-specific references;
- exported APIs used by downstream crates;
- dynamic registration, serialization names, FFI, plugin hooks or generated code.

Delete only when evidence is sufficient for the repository context.

## Decision Hierarchy

Apply changes in this order:

1. Remove proven dead code and stale dependencies/imports.
2. Remove accidental complexity and unnecessary indirection.
3. Eliminate unnecessary allocations, clones, conversions and owned parameters.
4. Flatten control flow and make invariants explicit.
5. Consolidate genuine duplication.
6. Split modules only where responsibilities or change boundaries justify it.
7. Introduce abstractions only when they reduce total conceptual complexity.

Prefer a clear local duplication over a premature abstraction.

## Idiomatic Rust Without Dogma

### Ownership and allocation

Prefer borrowed inputs when ownership is unnecessary:

- `&str` over `&String`.
- `&[T]` over `&Vec<T>`.
- references or moves over gratuitous `.clone()` / `.to_owned()`.
- concrete stack values over `Box<T>` when indirection is unnecessary.

Do not change an owned public parameter to a borrowed one merely for style; first confirm API compatibility and caller semantics.

Treat “zero allocation” as a constraint only on relevant paths. Do not trade obvious, maintainable code for speculative micro-optimization. Benchmark hot paths when the trade-off is material.

### Control flow

Prefer `?`, `let ... else`, `if let`, early returns, and focused helper functions when they reduce nesting and make the happy path obvious.

Do not mechanically replace `match`: keep it when exhaustiveness, multiple meaningful branches, or state-machine structure is clearer.

### Iterators vs loops

Do not mechanically convert `for` loops into iterator chains. Use iterators when they make transformation intent clearer; use loops when mutation, early exits, stateful control flow, debugging, or performance is clearer.

Prefer readability over “functional-looking” density. Avoid long iterator pipelines that hide state transitions or error semantics.

### Functions and inlining

Do not inline solely because a helper is called once or is 1–2 lines long. Inline when the helper has no useful name, invariant, reuse boundary, test seam, or abstraction value and locality improves.

Keep a tiny function when its name explains intent or isolates an invariant.

Compiler inlining is a separate performance decision; source-level inlining does not guarantee faster code.

### DRY, generics and macros

Merge code only when duplicated blocks represent the same concept and are expected to evolve together.

Prefer, in order:

1. a normal function;
2. a small generic function when type variation is real;
3. a trait when polymorphic behavior is part of the domain boundary;
4. a macro only when syntax-level repetition cannot be expressed cleanly otherwise.

Never introduce a macro merely to reduce line count. Avoid generic abstractions that increase trait bounds, monomorphization, compile time, diagnostics complexity, or cognitive load without a clear payoff.

### Dynamic dispatch

Do not introduce `Box<dyn Trait>`, `Arc<dyn Trait>`, or other dynamic dispatch merely to unify implementations. Use it only when runtime polymorphism is genuinely required and the cost/ownership model is appropriate.

## Large Files and Module Boundaries

Treat 500 lines as a review signal, not an automatic split threshold. Split when a file contains multiple independently understandable responsibilities, changes for unrelated reasons, or forces excessive navigation.

Good extraction boundaries include:

- domain types and invariants;
- parsing/encoding/serialization;
- conversions (`From`/`TryFrom`) when substantial;
- external adapters such as database, filesystem, network or platform integration;
- orchestration/state-machine logic;
- implementation-specific helpers.

Prefer modern Rust module layouts (`foo.rs` plus `foo/bar.rs`) where practical; use `mod.rs` only when it fits the repository's established convention.

Keep visibility minimal: private by default, then `pub(super)` / `pub(crate)` when justified, and `pub` only for intended external contracts.

Do not create one-file-per-type fragmentation. A module boundary must reduce conceptual coupling, not merely move lines around.

## Refactoring Workflow

### 1. Establish the baseline

Read repository instructions and determine workspace structure, MSRV/toolchain, features, targets, generated code, CI commands and public crates.

Run the repository's existing verification commands before changing code. Record pre-existing failures instead of silently attributing them to the refactor.

For behavior-preserving refactors, add or strengthen characterization/regression tests when important behavior is not already protected. Do not invent new behavior.

### 2. Diagnose

Build an evidence-backed inventory of:

- dead or unreachable code;
- unnecessary allocations/clones/conversions;
- duplicate logic;
- deeply nested control flow;
- oversized functions/modules;
- unclear ownership or lifetime boundaries;
- unnecessary traits/generics/macros/dynamic dispatch;
- Clippy/compiler warnings;
- suspicious lock scope or async blocking;
- feature/target-specific paths at risk.

Separate “must fix” from “possible simplification”. Do not rewrite healthy code for stylistic uniformity alone.

### 3. Plan the smallest safe slices

Order changes so each step is independently reviewable and verifiable. Prefer one semantic concern per patch.

For module decomposition, state the proposed responsibility of each new module and its visibility boundary before moving code.

### 4. Refactor incrementally

After each meaningful slice:

1. format the touched code;
2. compile/check the smallest relevant scope;
3. run focused tests;
4. run relevant Clippy checks;
5. inspect the diff for accidental API or semantic changes.

Do not combine broad renaming, file movement, behavior changes, dependency upgrades, and optimization into one refactor unless required.

### 5. Verify globally

Before claiming completion, run the strongest applicable repository checks, typically including formatting, all relevant feature/target checks, tests, Clippy, docs/tests, and project-specific CI commands.

For concurrency-sensitive changes, explicitly compare lock order, lock lifetime, task spawning, cancellation points, channel behavior and ordering guarantees.

For performance-sensitive changes, compare allocation/benchmark evidence where available. Do not claim a speedup from aesthetics or fewer source lines.

## Required Output During Execution

Keep reporting concise and evidence-based. Communicate:

- **Baseline:** commands run and any pre-existing failures.
- **Findings:** concrete complexity/debt with file/symbol references.
- **Plan:** minimal ordered refactoring slices and module boundaries.
- **Changes:** what was removed, simplified, merged, retained, or deliberately not abstracted.
- **Verification:** exact commands run and their outcomes.
- **Semantic preservation:** relevant API, error, concurrency, ordering, serialization and allocation properties checked.
- **Residual risk:** anything not verified because of environment, platform, unavailable dependency, or missing tests.

Never state that behavior is unchanged merely because the project compiles.

## Anti-Patterns

Reject these shortcuts:

- “Unused according to search, therefore delete it.”
- “Iterator chains are always more idiomatic than loops.”
- “One call site means inline.”
- “Duplicate-looking code means DRY it.”
- “A macro is simpler because it removes lines.”
- “Over 500 lines means split immediately.”
- “Fewer files/functions/types means a simpler design.”
- “`clone()` is always bad.”
- “`Box` or dynamic dispatch is always bad.”
- “Compilation proves semantic equivalence.”
- “Clippy is the design authority.”

## Completion Standard

A successful refactor leaves the codebase with fewer unnecessary concepts, not merely fewer lines. The result should be easier to reason about locally, preserve established contracts, avoid unjustified runtime costs, and pass the strongest available verification suite.
