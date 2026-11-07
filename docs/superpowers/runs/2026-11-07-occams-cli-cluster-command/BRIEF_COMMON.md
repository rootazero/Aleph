# Common Brief — Aleph Static Review (Occam's Razor, Batches B1–B5)

Repo: `/home/zou/data/workspace/Aleph` (git worktrees per batch, branched from `main` @ `c21e4511c`).
Scope of your batch: see `docs/superpowers/runs/2026-11-07-occams-cli-cluster-command/B<N>.md`.

## What this is

Full-module static code review (**NO diff**) guided by `/rust-occams-razor`. We are hunting for
**real defects and unnecessary complexity**, not style preferences:

1. **Occam's Razor**: removal of unnecessary concepts, redundant abstractions, uncalled helpers,
   dead code, accidental complexity — *without* inventing new abstractions to hide them.
2. **Panic risks**: `unwrap()`, `expect()`, direct indexing `vec[i]`, string byte slicing `&str[..]`
   on UTF-8, integer overflow, `panic!` on a reachable public path.
3. **Concurrency & Async**: blocking I/O inside `async fn` (tokio runtime thread stalls), lock held
   across `.await`, lock-ordering inversion, poisoned-lock handling, cancellation safety, TOCTOU.
4. **Error semantics**: lost error context, swallowed errors (`let _ =`, `.ok()`), fail-open on
   security checks, fail-closed answers consumed as permission.
5. **Invariants & Contracts**: public API stability, error variants/propagation, `Send`/`Sync`,
   ordering + idempotency + retry semantics, serialization/wire formats.

## Domain contracts for these modules (do not violate)

- **`src/cluster`** is admission- and identity-critical: enroll / admit / deregister / revoke must be
  idempotent, deregistration must be **sticky** (a deregistered node stays refused), a node must never
  adopt another node's identity, ambiguous name resolution must **fail closed** (never guess), and a
  reconnect must not orphan a live connection mapping or evict a live node.
- **Reverse RPC** must fail closed: an unresolved / cancelled / timed-out invoke must never be
  reported as success, and the pending-invoke ceiling must hold on every path.
- **Approval** must fail closed: "no channel", "transport closed" and "JSON-RPC error" are
  *unavailable*, never *denied*; only a real human decision maps to denied/approved.
- **File command jail** is a security boundary: path traversal, symlink escape, TOCTOU between
  resolve and use, size caps, and every `unsafe` block must state its invariant.
- **`src/cli` policy** is the single-instance guard: a held lock must forward to the running server
  over IPC rather than start a second instance.
- **`src/command`** holds **no registry of its own** — every source-specific field is derived from a
  single `UnifiedTool` lookup against the `ToolCatalog`.

## Tooling budget (hard)

- Read files carefully. Read them **fully**, not sampled.
- **Do NOT run cargo, cargo check, cargo test, clippy, or any build command.** Builds are forbidden
  in this phase.
- **Do NOT edit any source file during the review phase.** Only write findings to your findings file.
- Do NOT run `git stash`, `git checkout --`, `git restore`, `git clean`, `git commit`, or `git push`.
- `graphify` is available for orientation (`cd /home/zou/data/workspace/Aleph && graphify query "..." --budget 1200`).
  Use it to find callers and to learn what a symbol is connected to; it is orientation, not proof.

## What counts as a finding (P0..P3)

- **P0**: data loss / security bypass / remote code execution / deadlock / memory corruption /
  identity confusion (a node adopting another node's identity).
- **P1**: logic bug causing incorrect observable behavior; unhandled panic on reachable production
  input; blocking call stalling the tokio runtime thread; broken protocol contract; lost or
  swallowed error that hides a real failure.
- **P2**: robustness defect (index/slice panic on malformed input), resource leak, TOCTOU race,
  missing bound, excessive allocation/clone on a hot path.
- **P3**: maintainability / Occam's Razor debt (dead code, duplicate logic that can drift,
  gratuitous abstraction, misleading comment, single-use helper that adds a name but no meaning).

Every finding MUST include a **concrete failure mode**: what input/state triggers it, and what goes
wrong. A finding without a failure mode is a style opinion — drop it.

### Anti-false-positive rules

- "No reference found" is **not** proof of dead code. Check tests, examples, bins, `#[cfg]`
  feature/target gates, `serde` names, FFI, plugin hooks and generated code before claiming dead.
- Do not report a `clone()` as a defect without naming the hot path.
- Do not report a missing abstraction as a finding.
- A guard held across `.await` is only a finding if the lock is *actually* held across an await
  point in that code path.
- Prefer **no finding** over a speculative one.

## Output contract

Write your findings to:
`docs/superpowers/runs/2026-11-07-occams-cli-cluster-command/findings/<BATCH_ID>.md`

Use this exact structure per finding:

```markdown
### <ID> [P0|P1|P2|P3] <short title>
- Location: <path>:<line> (`symbol_name`)
- Failure mode: <concrete trigger -> concrete wrong outcome>
- Evidence: <the code shape that proves it, 1-3 lines quoted>
- Suggested fix: <the minimal, behavior-preserving fix; state invariants preserved>
- Confidence: <0.0-1.0>
```

At the top of the file write:

```markdown
# Findings — <BATCH_ID> (<Description>)
Files reviewed: <count> (<LOC> LOC)
Counts: P0=<n> P1=<n> P2=<n> P3=<n>
```

And at the end, list files reviewed with a clean status if no findings were found in those files.
