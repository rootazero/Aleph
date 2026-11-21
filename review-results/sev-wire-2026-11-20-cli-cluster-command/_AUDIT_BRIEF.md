# Severed-wire audit brief (A1 / A4)

Audit a Rust module in the repo `/home/zou/data/workspace/Aleph` for **SEVERED WIRES**: a producer
whose value no consumer ever reads, a stub whose far end does not exist, an inert configuration knob,
a client the server never answers, a renamed path, or a far end that is never compiled.

**READ-ONLY**: no edits, no commits, no cargo.

## Tools
- `graphify explain "<Symbol>"` (CLI at `/home/zou/.local/bin/graphify`; the code map lives in the
  repo under `graphify-out/`) is a fast lookup for a symbol's definition site, degree, call edges and
  paths. Use it to navigate, but still read the code.
- `rg -n "sym" src/ interfaces/ desktop/ crates/ tests/` finds consumers. **Always pass a path** —
  `rg`/`grep` without a path silently returns empty. Never run `find /`. Never run cargo.

## PART A — re-verify the prior findings
For each one, answer **OPEN / FIXED / PARTIALLY FIXED / REFUTED**, with exact `file:line` evidence
for BOTH ends (the producer side and the consumer side).

## PART B — find NEW severed wires
Per candidate pick one hypothesis:
- **CONNECT** — one missing wire to add.
- **CUT** — delete the dead producer AND its orphaned consumers/tests/docs.
- **DECIDE** — needs a human product/architecture call.

A painless severed wire (long severed, zero production pain, the existing error path already covers
it) is a CUT candidate. A live caller plus real production pain is a CONNECT candidate.

## Seam forms
1 no consumer at all · 2 stub whose far end does not exist · 3 inert configuration knob ·
4 client that the server never answers · 5 name/path drift · 6 far end never compiled.

## Lenses
registration parity · call-vs-handler parity · classifier-vs-handler parity · event emit-vs-subscribe
parity · config-reader parity · path/route parity · stub sweep.

## Non-negotiable rules
- **Read before you write.** If you cannot find a live consumer, the default hypothesis is **CUT**
  and you must list the exact grep patterns you ran and their result.
- Separate **production** callers from **test** callers. Anchor every claim to `file:line`.
- Mark anything you are unsure about as **UNCERTAIN** instead of speculating.
- No style nits, no rustdoc typos, and never re-report a prior finding as "new".

## Output shape (per candidate)
```
### <ID> - <title>
- Form: 1|2|3|4|5|6
- Producer: <path>:<line> - <symbol>
- Consumer: <path>:<line> - <symbol>   (or "no consumer found (grepped: <patterns>)")
- Live caller search: <command + result>
- Symptom: <what an operator observes, or "no symptom - covered by the existing error path">
- Hypothesis: CONNECT|CUT|DECIDE
- Confidence: high|medium|low
```

Write the **FULL** report to the output file named in your task, then return a summary of **at most
40 lines** (the prior-verdict table plus the candidate list with hypotheses). Keep the returned
summary short so the run does not time out.
