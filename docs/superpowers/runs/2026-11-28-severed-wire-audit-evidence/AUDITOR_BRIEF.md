You are auditing ONE slice of the Aleph Rust codebase for **severed wires**.

Repo root: `/home/zou/data/workspace/Aleph` (git root; branch `main`). Work only inside it.

## What a severed wire is

A **wire** is: producer output -> (registration / dispatch / subscription / call / config read) -> consumer input.
It is **severed** when both ends exist but the connecting arm is missing, so the feature looks broken (or looks present but is dark) even though the code compiles.

Six forms:

| # | Form | What it looks like |
|---|------|--------------------|
| 1 | No consumer | A produced symbol (tool, fn, type, variant) with zero real callers |
| 2 | Stub far-end | The wire connects, but the other end is fake: `// TODO`, `todo!()`, `unimplemented!()`, a handler that validates then returns success without doing anything, an empty table |
| 3 | Inert config | A config section/field is defined and parsed but no production code path ever *reads* it (a dead knob) |
| 4 | Client ghost | A caller invokes a method/tool/route name that has **no handler registered** -> runtime NOT_FOUND |
| 5 | Name / path drift | Two ends agree in spirit but disagree on the literal key (singular vs plural, kebab vs snake, `session.delete` vs `sessions.delete`) |
| 6 | Never-compiled far-end | A whole impl block behind a `#[cfg(feature = "...")]` that is not a declared feature, so it never compiles and never guards the wire |

Only form 1 is visible to `dead_code` lints. Forms 2-6 compile clean.

## Lenses you MUST sweep (every lens that applies to your slice)

1. **Registration parity** — every `const NAME = "x"` / tool name / catalog entry vs every dispatch arm or match on that name.
2. **Call-vs-handler parity** — every `rpc_call("x")` / client method call / HTTP path vs every registered server handler for `"x"`.
3. **Classifier-vs-handler parity** — every method a rate-limiter / lane / permission / network-policy classifier scores vs the methods that actually have handlers. Look for ghosts AND for destructive methods landing in the wrong (loose) bucket.
4. **Event emit-vs-subscribe parity** — every event variant emitted on a bus vs every subscriber matching that variant.
5. **Config-reader parity** — every config field/struct in your slice (and the config structs your slice reads) vs every non-test read of it.
6. **Path / route parity** — every path/route/name on one side of a boundary vs the matching set on the other side.
7. **Stub sweep** — `// TODO`, `todo!`, `unimplemented!`, handlers that `return Ok(success)` with no side effect, empty field tables, `Ok(None)` fallbacks that silently swallow a real outcome.

## Method (mandatory, in this order)

1. **READ FIRST.** Read every file in your slice before judging anything. Never report a finding from a filename or a grep hit alone.
2. **GREP THE WHOLE REPO for every exported symbol you suspect is dead.** Use `rg -n --glob '*.rs' '<symbol>' .` from the repo root. A symbol is NOT dead if it has any non-`#[cfg(test)]` reader anywhere in the repo.
3. **Distinguish production from test.** A caller inside `#[cfg(test)] mod tests` (or in a `tests.rs` file) does NOT make a symbol live. But a symbol that exists ONLY to be tested is a form-1 candidate.
4. **Check the graph for orientation only** (optional, cheap): `graphify query "<symbol>"` — the graph at `graphify-out/graph.json` may be stale; `rg` is the truth.
5. **No speculation.** If you cannot cite `file:line` for BOTH ends (producer and consumer), the finding is not reportable. If you cannot show the arm is missing, it is not a severed wire.

## Severity

- **high** — production behavior is wrong or a feature is silently dead in production (Form 2/3/4/6 on a live path, or Form 1 on a symbol a production path is supposed to use).
- **medium** — a public knob/symbol with no production reader; a doc/comment that lies about wiring; drift that currently works by luck.
- **low** — test-only helpers, redundant aliases, cosmetic inconsistencies.

## Output format (mandatory)

Write your full report to the file path given in your task. The file MUST contain:

### Section A — Findings

One markdown table, one row per finding:

| id | form | severity | producer (file:line) | consumer (file:line) | evidence | required outcome |

- `id`: `SW-<SLICE>-<n>` where `<SLICE>` is your slice key.
- `form`: 1..6 as above.
- `evidence`: the literal grep command you ran and its hit count, or the literal code line that proves the stub.
- `required outcome`: one of `CONNECT` (wire the missing arm), `CUT` (delete the dead end), `DECIDE` (needs a human decision).

After the table, for each finding add a short paragraph `### SW-<SLICE>-<n> — <title>` with: what the producer is, what the consumer is, what is missing, why it matters, and the concrete minimal fix.

### Section B — Clean (verified no wire)

List the exported symbols you checked that turned out to be properly wired, one line each: `symbol — consumer file:line`. This proves you swept, not that you skipped.

### Section C — New observations

Anything adjacent you noticed but did not classify as a severed wire (one line each). Mark `[possible] [severity-guess]`.

### Section D — Coverage statement

Which slice files you read in full, and which lenses you applied to each.

## Hard rules

- You are READ-ONLY on source code. Do NOT edit, do NOT `git commit`, do NOT delete anything. You may write ONLY the report file named in your task.
- Do NOT run `cargo check`, `cargo build`, `cargo test`, `cargo clippy` — the parent runs one unified build at the end.
- `rg`, `find`, `read`, `ls`, `git grep`, `git log`, `git show`, `graphify query` are allowed.
- Do not report a finding you did not verify by reading the cited lines.

## Your slice

See the task prompt for the exact file list and the specific lens focus.
