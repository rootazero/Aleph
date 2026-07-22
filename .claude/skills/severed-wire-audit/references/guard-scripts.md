# Guard Scripts — making "no omission" real

Read this for phase 5. A severed wire recurs because the same fact is copied into ≥3
unsynchronized lists with no single source of truth. Documentation does not prevent it;
only enforcement does.

## Contents

- [Two tiers of enforcement](#two-tiers-of-enforcement)
- [The grep-diff guard pattern](#the-grep-diff-guard-pattern)
- [Baseline discipline](#baseline-discipline)
- [A guard is a detector, not a judge](#detector-not-judge)
- [Using the bundled template](#using-the-bundled-template)

## Two tiers of enforcement

Prefer the first; fall back to the second.

1. **Compile-time (best).** One enum/type is the single true source; the other lists are
   *derived* from it, so an unrouted or misspelled entry **fails to compile**. This is the
   only form that is truly omission-proof. Aleph's five compile-time root-fixes:
   - RPC method as a single `enum` true source; lane/rate/client derive from it → an
     unrouted method is a compile error. (7 separate RPC "severed wire" findings were all
     symptoms of this one missing single-source.)
   - A tool-registration completeness test: every `impl AlephTool` NAME vs dispatch arm vs
     catalog must match.
   - A config-reader completeness test: every `[policies.*]` field has a non-test read.
   - Replace a blurry `Option`-returning far-end with a three-state enum (`RearmDecision`).
   - Remove `_ =>` catch-alls on the event emit side so a new variant must be handled.

2. **CI grep-diff guard (good).** When a compile-time source isn't feasible, a script that
   computes `DEFINED − CONSUMED` against a triaged baseline, wired into the test target,
   turns a *new* severance red. This is what the bundled `scripts/wiring_audit.py` does.

## The grep-diff guard pattern

Every guard is the same shape:

```
DEFINED    = { every symbol produced/registered on side A }   # e.g. const NAME = "x"
CONSUMED   = { every symbol dispatched/read on side B }        # e.g. "x" => arm
SEVERED    = DEFINED − CONSUMED
FAIL if    SEVERED − KNOWN_SEVERED  is non-empty
```

Two rules make it robust:

- **Strip test code before scanning** so a `#[cfg(test)]` manual registration doesn't mask
  a production severance (a test that registers `"invalid"` must not make the guard think
  `invalid` is wired). The Aleph guards cut the file at the first `#[cfg(test)]`.
- **Scan the right files on each side.** DEFINED usually globs a directory
  (`src/builtin_tools/**/*.rs`); CONSUMED is often one dispatch file. Getting these wrong
  produces false greens, not false reds — so verify the counts look sane
  (`DEFINED: 162 | DISPATCHED: 168`) on a known-good baseline.

## Baseline discipline

`KNOWN_SEVERED` is the set of *intentionally* severed wires you've already triaged (a
DECIDE you deferred, or a known limitation). The discipline:

- **Fix one → remove one from the baseline.** The baseline only shrinks. When the 2026-07-15
  audit resolved every finding, all three guards' baselines drained to **empty**, and the
  comment was updated to say "a NEW severed entry now means a genuine defect → fix it."
- **A shrinking-only baseline is itself a ratchet** — it cannot silently regrow.
- Document *why* each baseline entry is still there (Aleph kept one: `ToolSafetyPolicy`
  triggers a type-ref false-"consumed" via a dead reader, so it can't be independently
  re-verified — a known limitation, annotated inline).

## Detector, not judge

**The guard tells you a wire is severed. It does NOT tell you whether to CONNECT or CUT.**
That is always a human/read decision (the triage tree). The guard flagged `[policies.metrics]`
and `AiRetrievalPolicy` as inert with equal confidence — but one was a CONNECT (live
consumer of a hardcoded default) and the other a CUT (no consumer). Never let a green guard
imply "everything is wired correctly"; it only means "nothing severed beyond the triaged
baseline".

The flip side is the guard's superpower: **it catches what a semantic/LLM sweep misses.**
`memory.store` and `AiRetrievalPolicy` were both dropped by the LLM audit and caught only by
the mechanical grep-diff. This is why phase 5 is not optional for an "exhaustive" claim.

## Using the bundled template

`scripts/wiring_audit.py` is the Aleph tool-registration guard generalized into a
parametrized function. Configure one `SeamSpec` per seam:

```python
from wiring_audit import SeamSpec, run_audit

TOOL_SEAM = SeamSpec(
    name="tool-registration",
    defined_glob="src/builtin_tools/**/*.rs",
    defined_re=r'const NAME:\s*&(?:\'static\s+)?str\s*=\s*"([^"]+)"',
    consumed_files=["src/executor/builtin_registry/registry/tool_registry_impl.rs"],
    consumed_re=r'^\s*"([a-z][a-zA-Z0-9_]*)"\s*=>',
    known_severed=set(),          # drained baseline
    strip_tests=True,
)

raise SystemExit(run_audit([TOOL_SEAM], root="."))
```

Add more `SeamSpec`s for the RPC, config, and event seams — same function, different
patterns. Wire the script into your test/CI target (`just check-wiring` / `test-all` in
Aleph) so it runs on every change. The script is language-agnostic: only the regexes are
Rust-flavored; swap them for your language's registration/dispatch syntax.
