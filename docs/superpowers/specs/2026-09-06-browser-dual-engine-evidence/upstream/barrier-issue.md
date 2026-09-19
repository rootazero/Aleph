# Upstream deliverable 2 — issue: every CDP method stalls together behind one global barrier

> **This file is for a human to submit** as an issue on the obscura repository. Aleph does not open
> issues or pull requests against other people's repositories, and this round did not.
>
> **Two different as-ofs, and they must not be blurred.**
>
> - **The measurement** is against **obscura 0.2.2** — the released binary, not a source build — on
>   2026-09-05/06. Source: `obscura-spike-v022.md` §M12/M12b in this directory; the probe is
>   `probes/m12e-dense.mjs`, also in this directory.
> - **Every `dispatch.rs` line number below was re-read on 2026-09-20** in the clone at
>   `/Volumes/TBU4/Github/obscura`, whose working tree is at
>   **`eec047a188cc75b7a1a257397ad84493ee59c091`** (fetched 2026-09-12). They are **not** the
>   survey's numbers: the survey was taken at `72c84ad` and recorded the allowlist as
>   `dispatch.rs:653-717`, which no longer locates it. Attributing re-measured anchors to the
>   survey would be the cheap half of 判据 §1 — a citation that was true when written, pointing at
>   a document that has since moved under it.
>
> ⚠️ **Before submitting, re-read both files and re-run the probe.** Upstream may have moved again
> since 2026-09-12, and the two count tables do not close — see "A discrepancy in the recorded
> counts" at the end. The finding does not depend on either, but an issue whose own arithmetic does
> not close, or whose line numbers do not resolve, hands the maintainer a reason to stop reading.

## Title

`cdp: all methods stall together for 25-44s on a busy page, including Target.getTargets`

## Summary

On a large page, every CDP method this probe touched — `Runtime.evaluate`, `DOM.getDocument`,
`DOM.getBoxModel`, `Page.getLayoutMetrics`, `Target.getTargets` — stops answering at the same moment
and resumes at the same moment. It is not per-call cost and it is not serial queueing: 400 calls
issued on a fixed 500 ms schedule completed at **three instants**, and the number of calls each
method completed at each instant is essentially identical.

## Reproduction

```bash
# 1. Start obscura (0.2.2 release build, render feature)
obscura serve --port=9444 --allow-private-network

# 2. Dense sampler: 80 ticks x 5 methods on a fixed 500 ms schedule, NOT awaiting the
#    previous tick. That last part is the whole measurement — see "Why the first
#    measurement was wrong".
node m12e-dense.mjs ws://127.0.0.1:9444/devtools/browser
```

The probe navigates to `https://en.wikipedia.org/wiki/Rust_(programming_language)`, asserts page
identity (`location.href` contains `wikipedia.org`) before sampling — an earlier run silently
navigated nowhere and measured `about:blank` at 1-3 ms — resolves `<body>` after `loadEventFired`
(the document is not queryable before load on this page, so "resolve before load" is not
achievable), then samples.

## Result

400 samples issued, 0 errors, 0 unfinished. Completion instants, not round trips:

| completion instant (ms since load) | calls finishing there |
|---|---|
| 25,727 – 25,807 | 260 |
| 49,420 | 4 |
| 70,481 – 70,500 | 135 |

Per method, at each instant:

| method | @25.7–25.8 s | @49.4 s | @70.5 s |
|---|---|---|---|
| `Runtime.evaluate` | 51 (+1 at 26,015) | 0 | 27 |
| `DOM.getDocument` | 52 | 1 | 27 |
| `DOM.getBoxModel` | 52 | 1 | 27 |
| `Page.getLayoutMetrics` | 52 | 1 | 27 |
| `Target.getTargets` | 52 | 1 | 27 |

Every call issued between t=0 and t=25,500 completed at t≈25,750 **regardless of when it was
issued**, which is why the RTT column counts down linearly (25,727 / 25,248 / 24,749 / … / 306).
There are two barriers on this page: ~25.8 s starting at load, and a second ~44 s one starting at
t≈26.5 s. The only sample that escaped is one `Runtime.evaluate` issued at t=26,000 — in the 250 ms
gap between the two barriers — which returned in **15 ms** while its four tick-mates were caught by
the second barrier and waited 23.4 s.

A separate run also showed the stall is not one bounded window after load: a connection that sat
**idle for 45 s** paid 19.4 s on its first touch afterwards.

## Why the first measurement was wrong, in case it matters for triage

The first version of this probe awaited each tick's five calls before issuing the next, so a stalled
tick ate 25 s of a 40 s window and each run produced two usable rows. That version reported "19-26 s
per call", which reads like per-call cost and points at the wrong mechanism entirely. The barrier
only becomes visible with **concurrent** sampling. Any triage that reproduces this serially will be
measuring its own waiting.

## What is NOT measured

The binary was not instrumented, so this issue **cannot say which lock it is**. The source survey
pointed at `ctx.v8_lock` and the `is_v8_free_method` allowlist; re-read at `eec047a1` (2026-09-20),
those are `crates/obscura-cdp/src/dispatch.rs:125` and `dispatch.rs:651` — whose first arm is
literally `"Target.getTargets"` at `:654`, and the same predicate also decides at `:767` whether the
per-command watchdog is armed at all. The survey's own figure for the allowlist was `653-717`, taken
at `72c84ad`; it is cited here only to say it has moved. The observation the survey made still
stands: `Target.getTargets` and
`Page.getLayoutMetrics` appear to be *on* that allowlist yet stall anyway — so either the allowlist
is not protecting them in practice at 0.2.2, or the shared resource is something else. The 15 ms
`Runtime.evaluate` sitting beside a 23 s `DOM.getDocument` in the same tick is evidence against both
a pure single-V8-lock story and pure head-of-line blocking on the socket.

## A discrepancy in the recorded counts

The two tables above are reproduced verbatim from the spike record, and **they do not close**:

- the per-method table sums to 51 + 52 + 52 + 52 + 52 = **259** at the first instant, while the
  instants table says **260**;
- the four figures together (259 + 4 + 135, plus the one stray at 26,015) account for **399** of the
  400 samples issued.

The raw series (`m12e-dense.json`) is not in this repository, so the discrepancy could not be
resolved by re-reading; it is one sample either way and the mechanism does not turn on it — the
finding is "five methods, five near-identical per-instant counts, three instants", and a call
miscounted by one leaves every one of those intact. It is written down rather than smoothed over
because a maintainer who adds up the columns will find it, and a number quietly corrected to make a
table close is worth less than a number with its uncertainty attached. **Re-run the probe before
submitting and replace both tables from the fresh run.**

## Why it matters downstream

The per-command watchdog (`OBSCURA_CDP_COMMAND_TIMEOUT_MS`, default 60,000 ms —
`dispatch.rs:766-775` at `eec047a1`) bounds the stall by **terminating the isolate**. A client that wants to
survive the barrier must therefore answer before 60 s and treat the wait as a fact about the engine
rather than an error — which is what we do (30 s per-command timeout, surfaced as a typed "engine
busy" the model can act on). But the barrier also means "issue the geometry call on a different CDP
path to dodge the stall" is not an available move for any client, which is worth knowing before
someone designs around it. That retraction is recorded in the spike itself: a recommendation to
"prefer the CDP-native geometry path to dodge the stall" was written, then withdrawn by this
measurement.
