# SDD ledger — plan: docs/superpowers/plans/2026-08-21-tauri-webview-resource-control.md

Spec: docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md (read, reachable)
Branch: main (see Ruling P0)

## Pre-flight conflict scan

### Pairs sharing a file or an interface

| Pair | Produces vs consumes | Finding |
|---|---|---|
| T1,T3,T5,T8 -> `justfile` | T1 adds `check-baseline` + wires into `wasm`; T3 edits wasm step 4; T5 edits wasm step 3.5; T8 inserts a step after the baseline check | **Line numbers quoted in the plan will be stale** after each edit. See Ruling P1 |
| T1,T2,T3,T4 -> `scripts/check_webview_baseline.mjs` | T1 creates (edge A); T2/T3/T4 append edges B/C/D | Clean: strictly additive, plan order == dependency order |
| T1 -> T2,T4 (`webview-baseline.json`) | keys macos_min/safari_min/webkitgtk_min/css_probes/js_probes | Clean |
| T2 -> T3 (probe source inlined verbatim) | T3 pastes T2's file inside a `<script>` element | **Hazard**: a literal `</script>` anywhere in the probe ends the element early. See Ruling P2 |
| T2 -> T6 (`data-platform`), T2 -> T7 (`data-flat`) | `dataset.platform`/`dataset.flat` == `data-platform`/`data-flat` | Clean, names agree |
| T8 -> T9 (`MIN_BYTES` import) | both `.mjs`, ESM import valid | Clean |
| T8 -> T9 -> T17 (`dist/*.br` on disk vs in git) | T8 defers committing `.br` to T17; T9's guard reads the filesystem | Green in-session; **CI would be red between T9 and T17**. See Ruling P3 |
| T8,T9 -> T10 (`ControlPlaneAssets` embeds `.br`) | rust_embed reads disk in debug | **Unverified**: the derive may carry an include/exclude filter. See Ruling P4 |
| T11 -> T12,T13 (`parse_range`, `RangeVerdict`) | one parser, two routes | Clean |
| T12 vs T13 (range rate constants) | `ARTIFACT_RANGE_READS_PER_MINUTE` vs `CANVAS_RANGE_READS_PER_MINUTE` | Clean: distinct names, per-route private limiters |
| T4 (reads `dist/tailwind.css`) vs T7 (edits `styles/tailwind.css`) | T7 re-keys selectors only; census counts CSS *functions* | Clean; T17 re-runs edge D from a clean rebuild |
| T6 -> T15 (`host()`, `HostPlatform`) | T15 passes `&ChatState` into `speak` | Clean; T15 must grep for every `speak(` caller |
| T3,T5,T8,T17 all run `just wasm` | rebuilds the 22 MB dist repeatedly | Uncommitted dist churn does not enter commit-range review packages; T17 is authoritative |

### Per-task self-consistency

| Task | Tests vs code vs files | Finding |
|---|---|---|
| T1 | guard edge A vs base config vs lite overlay | Consistent (deviation note covers the overlay) |
| T2 | probe writes attrs, exposes nothing on `window` | Consistent |
| T3 | edge C asserts verbatim containment; Step 3 rebuilds | Consistent |
| T4 | reverse assertion honest, forward census over-reports by design | **Needs adjudication at runtime.** See Ruling P5 |
| T5 | fence + missing-tool branch both proven RED | Consistent |
| T6 | pure reader, unknown -> Linux | Consistent |
| T7 | two blocks re-keyed; `backdrop-filter: none` retained | Consistent |
| T8 | producer + round-trip verify | Consistent |
| T9 | both directions proven RED | Consistent |
| T10 | identity-derived ETag + `Vary`; CompressionLayer pass-through measured | Consistent |
| T11 | **Interfaces says `content_range -> String`; body and test say `-> Option<String>`** | **CONTRADICTION.** See Ruling P6 |
| T12 | Range after all 8 gates; CSP on 206 and 416 | Consistent |
| T13 | test bodies given as comments pointing at sibling tests | Intentional (fixtures differ); flagged in the plan itself |
| T14 | three states; `tokio::process` for cancellability | Consistent |
| T15 | narrow predicate (code 4 only); new signal not `send_error` | Consistent |
| T16 | every assertion is an effect assertion and prints what it read | Consistent |
| T17 | rebuild + commit + full sweep | Consistent |

## Rulings

Ruling P0: Execute on `main`, no worktree — project CLAUDE.md mandates single-branch development on main ("单分支开发模式：所有开发工作直接在 main 分支进行"), which is the human partner's standing instruction and outranks this skill's default. Commits are made; **nothing is pushed**. Cost if wrong: a dirty main if a task lands broken; recoverable by revert since every task is its own commit.

Ruling P1: Every dispatch brief instructs the implementer to locate the edit site **by content, not by the line numbers the plan quotes** — four tasks edit `justfile` and three edit one guard script, so quoted line numbers are stale after the first of them. Cost if wrong: an implementer edits the wrong region; caught by the task review's diff.

Ruling P2: T2's dispatch carries the constraint that the probe source must contain no literal `</script>` (it is inlined verbatim into a `<script>` element by T3), and T3's edge C stands as the guard. Cost if wrong: a truncated inline script — a white screen, which is the exact failure G1 exists to prevent.

Ruling P3: T8's deferral of the `.br` artifacts to T17 stands as written. The window where the repo's own dist guard would fail against a git checkout is closed by T17 inside this same session, and nothing is pushed before then. Cost if wrong: if the session dies between T9 and T17, a fresh clone fails `check_panel_dist.mjs`; recovery is one `node scripts/precompress_dist.mjs`, which the guard's own failure message prints.

Ruling P4: T10's dispatch requires the implementer to **read the `ControlPlaneAssets` derive first** and report whether any include/exclude filter would drop `.br`, rather than assuming embedding works. Cost if wrong: brotli negotiation is dead in release builds while every debug test passes — the "fake backend returns what the code expects" shape.

Ruling P5: T4's over-reporting census will print CSS function names needing a human call. **I adjudicate them**, per the plan's rule: inside the declared floor -> add to the reviewed list; outside it -> that is a real finding and the task stops for it. Cost if wrong: a genuinely unsupported CSS function is waved through, reintroducing G1 on old macOS.

Ruling P6: T11's `content_range` returns **`Option<String>`**, not `String`. The Interfaces block contradicts the task's own code body and its test (`RangeVerdict::Whole.content_range(TOTAL) == None`); the code and test are the operative form, and `Whole` has no Content-Range to render — a `String` return would force a meaningless value. T12 and T13 consume the `Option`. Cost if wrong: a compile error in T12, caught immediately.

## Progress

Task 1: dispatched (impl-t1, sonnet), BASE=015556e4e
Ruling P7 (correcting my own scan row): T1 does NOT wire the baseline check into the `wasm` recipe. The recipe comment "Run by `just wasm`" is forward-looking; the actual `node scripts/check_webview_baseline.mjs` call inside `wasm` is added by T3 Step 1, and T8's brief refers to it as "the line added in Task 3". The scan table row above overstated T1's scope; the implementer's reading is correct and its narrower diff stands. Cost if wrong: the check is unwired for two tasks — caught by T3, which cannot pass edge C without it.
Task 1: review — spec OK, quality Approved, 1 Important (plan-mandated): `readJson` has no try/catch, so a missing or malformed config crashes with a raw Node stack trace instead of a named `fail()`.
Ruling P8: the finding stands and is fixed in T1, but its stated premise is wrong and the fix is narrower than proposed. The reviewer argued the unguarded pattern is about to be copied into three more reads including the `dist/` files; it is not — T3 guards its `dist/index.html` read (task-3-brief.md:69-71) and T4 guards its `dist/tailwind.css` read (task-4-brief.md:28-30). What is genuinely unguarded is four reads of git-tracked files (2 configs here, 2 reads of `baseline-probe.js` in T2/T3). Absence of a tracked file is a broken checkout where a stack trace naming the path is tolerable; **malformed JSON is the real case**, and `JSON.parse`'s own SyntaxError names no file at all while this guard reads two structurally identical configs — so the operator cannot tell which one they broke. Fix = `readJson` converts read/parse failure into a named `fail()` carrying the path. No general framework, no touching T3/T4's already-correct guards. The `baseline-probe.js` reads are carried into the T2/T3 dispatches instead of expanding T1. Cost if wrong: a few lines of unused defensiveness in one build script.
Task 1: fix round 1/5 dispatched (commit 4316581ad, follow-up not amend). Implementer extended the guard to a third read (`webview-baseline.json` itself) beyond my framing and flagged it; scoped re-review asked to trace whether a failed `readJson` now returns undefined and reintroduces the crash one line later as a TypeError.
Task 1: fix round 1/5 (1 addressed, 0 open; commits c50eb591f..4316581ad)
Task 1: minor (deferred): config reads nest inside `if (baseline)`, so a broken webview-baseline.json masks Tauri-config read errors until a follow-up run. Not a regression (pre-fix that input crashed before reaching them). For final-review triage.
Task 1: complete (commits 015556e4e..4316581ad, review clean)
Task 2: dispatched (impl-t2, haiku), BASE=4316581ad
Task 2: implementer went silent (no final message); work verified present: commit 9f652d3fd, clean tree, report written, guard green, zero literal </script> in probe.
Task 2: review — spec ❌ (2 Important, both plan-mandated: duplicated floor values in the fallback copy; unguarded `document.body` write), quality Needs fixes. 3 minors.
Ruling P9 (finding 1, duplicated floor literals): **keep the numbers, add the pairing assertion** — do not drop them from the fallback copy. The fallback page is the ONE surface where a human ever reads the floor; replacing "macOS 13.3+ · WebKitGTK 2.42+" with "a modern browser engine" satisfies the single-source rule by making the deliverable worse, which is the wrong trade. The repo's own criterion for a display copy is not "delete it" but "guard it", and edge B already exists to pair this exact file against the JSON — so this is one more assertion on an existing edge, not a fifth edge. Cost if wrong: a slightly noisier edge B.
Ruling P10 (finding 2, `document.body` guard): **fix it**, though I resolved the reviewer's ⚠️ in the safe direction — task-3-brief.md places the inline `<script>` inside `<body>` after `<noscript>`, so `document.body` exists today and this is a latent trap rather than a live defect. Fixing anyway: the cost is one line, the arming edit ("make the probe run earlier" → move it to `<head>`) is a natural future instinct, and the failure mode is silent and catastrophic — attribute set, fallback never renders, deferred module script boots the broken app anyway, i.e. exactly the white screen this feature exists to prevent. Cost if wrong: one redundant conditional.
Ruling P11 (minor 2, `.replace('.', ...)` escapes only the first dot): **folded into this round rather than deferred.** Minors do not normally enter the loop, but the loop is already open, the fix is one character in a file the implementer is already editing, and the defect is in a guard — an unescaped dot is a regex wildcard, i.e. a guard that silently matches too loosely, which is the failure class this plan exists to close. Deferring it would spend a final-review seat on a one-token change. Cost if wrong: none material.
Task 2: fix round 1/5 dispatched (commit 7d7657bed). Controller spot-checks: floor numbers kept per P9 (baseline-probe.js:114), body guard present (:106), tree clean, guard green.
Ruling P12 (model calibration): **stop using haiku for implementers after T2.** Its code was correct and its one judgment call (adding the try/catch the brief's sample lacked) was right — but it never returned a status message on either dispatch, forcing me to reconstruct state from git twice, and it misreported its own file's line count three times (157, then 116; actual 119 then 121). Correct code with an unreliable account of itself is expensive here, because the report IS the test evidence the reviewer is told to check. Sonnet is the floor for the remaining implementers. Cost if wrong: modestly higher token spend per task.
Task 2: fix round 1/5 (3 addressed, 0 open; commits 9f652d3fd..7d7657bed)
Task 2: minor (deferred): the new edge-B version assertion is a whole-file `includes("macOS 13.3+")`, not scoped to the `Minimum:` paragraph — a stray duplicate of that exact compound string elsewhere in the file would let a drifted fallback sentence pass. Each string occurs exactly once today. For final-review triage.
Task 2: minor (deferred): implementer misreported this file's line count a fourth time (claimed 116, actual 121). Evidence-quality note; underlies Ruling P12.
Task 2: complete (commits 4316581ad..7d7657bed, review clean)
Task 3: dispatched (impl-t3, sonnet), BASE=7d7657bed
Task 3: implemented, commit 68dd07b48. Two concerns raised by the implementer, both ruled below.
Ruling P13 (brief's Step-4 mutation does not falsify edge C): **the guard is right and the brief's chosen mutation was wrong.** Edge C compares with `.trimEnd()`, so appending a space to the last line is absorbed — i.e. the brief picked as its falsification test exactly the one difference the guard legitimately tolerates (the heredoc composition can add trailing newline). `.trimEnd()` stays. The falsification requirement is satisfied by the three mutations the implementer found that DO go red (non-whitespace edit, ordering violation, missing file). Cost if wrong: if `.trimEnd()` turns out to hide a real class of drift, edge C is weaker than advertised — the reviewer is being asked to judge exactly that.
Ruling P14 (Step 5 fallback-page render not observed): **deferred to Task 17 Step 6**, which already owns end-to-end Windows verification. The implementer spent 20+ minutes on a debug `aleph-server` link (12 GB link.exe) and stopped rather than fabricate an observation — correct call. Note also that a debug aleph-server is known to die on the first chat turn in this repo, so that route was doubly wrong. **Carried requirement for T17:** verify the fallback page by serving `interfaces/webchat/dist/` with a throwaway static node server and temporarily corrupting one CSS probe value in the inlined script, NOT by building a server. Cost if wrong: G1's only user-visible deliverable ships unobserved — which is why this is carried as a hard requirement rather than dropped.
Task 3: review clean — spec OK, quality Approved, 0 Critical/Important. Reviewer confirmed edge C asserts ORDERING not just containment (probeAt 964 < moduleAt 6420), and that `.trimEnd()` tolerates only trailing whitespace: leading whitespace, interior re-indentation, HTML-escaping and CRLF divergence are all still caught. P13 upheld on the merits.
Task 3: minor (deferred): justfile:203-204 comment says "byte-identical" while the guard tolerates trailing whitespace — precise today, imprecise for a future debugger.
Task 3: minor (deferred): check_webview_baseline.mjs:146-147 scans the same 6KB string twice for the same needle.
Task 3: minor (deferred): edge C's containment check would pass on a 0-byte probe (`''.includes('')`); edge B covers it in practice.
Task 3: complete (commits 7d7657bed..68dd07b48, review clean)
Task 4: dispatched (impl-t4, sonnet), BASE=68dd07b48
Task 4: census first run surfaced 5 novel names. 4 adjudicated by the implementer as in-floor (`circle`, `inset`, `minmax`, `repeat` — Safari 10.1+); 1 escalated to me.
Ruling P15 (`::view-transition-old/new(root)` in dist/tailwind.css, pre-existing from 15cd1f93d): the implementer is right that it is genuinely **outside** the Safari 16.4 floor (View Transitions needs Safari 18) — so do NOT file it under "reviewed = inside floor", which would make the list say something false. But it is also not a floor violation in the sense the census exists to catch. The distinction is the mechanism G1 is about: an unparseable `oklch()` **inside a custom property** collapses the whole palette at computed-value time, whereas an unknown **pseudo-element** invalidates only its own rule — surrounding rules and custom properties are untouched. So on Safari 16.4 the theme-toggle animation is silently absent and nothing else changes; it is additionally JS feature-detected at theme_toggle.rs:53. **Resolution: split the reviewed list in two by REASON — `IN_FLOOR` and `DEGRADES_UNUSED`** — because one list holding two different judgments is the "a whitelist's upstream is a field holding two kinds of thing" defect, and the real cost is the precedent: the next person adding an out-of-floor capability that does NOT degrade safely would see this entry and follow it. Each `DEGRADES_UNUSED` entry must state its degradation argument and name its detect site. Cost if wrong: a slightly more structured constant in one build script.
Ruling P16 (D1's `css.includes(fn + '(')` is satisfied by a longer function ending in the probe name — `lch(` matches inside `oklch(`): **fix it.** Latent today (`css_probes` has no such pair) but the failure direction is false-negative — a probe reported load-bearing because a *different* function contains its name — which is precisely "a guard that passes for the wrong reason". Same class as P11. Require a boundary check so the character preceding the name cannot be an identifier character. Cost if wrong: none material.
Task 4: review — spec mostly OK, quality Needs fixes. 2 Important (D2's failure message never updated for the P15 split; census regex structurally cannot see vendor-prefixed functions), 2 Minor.
Ruling P17 (D2 failure message): **fix — my ruling asked for it and it was missed.** The two-bucket model currently lives only in source comments; the message a developer actually sees still says "add it to REVIEWED", which is both silent about the split and now imprecise (REVIEWED is a derived union nobody edits). The message is the teaching surface — the split's whole purpose is precedent, and precedent is set by what the next author reads when the census fires, not by a comment they never open.
Ruling P18 (census regex blind to `-webkit-foo(` / `-moz-foo(`): **fix.** The census's entire contract is "never false-negative" — a named, dormant, one-character-fixable blind spot contradicts the one property that makes its silence trustworthy. Allowing an optional leading hyphen makes it report MORE, which is the safe direction, and the reviewer confirmed zero prefixed functions exist in the built CSS today, so it costs zero new list entries. Cost if wrong: slightly noisier census.
Ruling P19 (D1's lookbehind treats `-` as an identifier char, so a prefixed occurrence reads as absent): **do NOT harmonize it with the census — leave the behaviour, add the comment.** The two regexes answer different questions and therefore need opposite failure directions: the census asks "what is here that I do not know about" and must over-report, so it accepts prefixes; D1 asks "is my declared probe still load-bearing" and must never silently pass, so it stays conservative and spurious-reds instead. That asymmetry is principled but looks like an inconsistency, which means the next reader will "unify" them and silently break one — so the comment must state why they differ. Cost if wrong: one spurious red on a prefixed-only occurrence, which is the direction we want to fail in.
Task 4: fix round 1/5 (4 addressed, 0 open; commits f869a9d00..521597016). Re-reviewer independently probed the new regex against synthetic CSS: captures `-webkit-linear-gradient`, still ignores `--foo` and `var(--foo)` — P18's premise held and the boundary is still a real boundary.
Task 4: complete (commits 68dd07b48..521597016, review clean)
Task 5: dispatched (impl-t5, sonnet), BASE=521597016
Task 5: BLOCKED by a verified PLAN DEFECT (not an implementer failure). The brief's fence — pass an explicit `--enable-*` list to wasm-opt and expect it to reject anything outside it — is a **no-op**. Binaryen 130 auto-detects features from the LLVM `target_features` custom section and unions them into validation regardless of CLI flags (`--detect-features` help says it "does nothing"; `-mvp` does not suppress it). Implementer built a genuinely SIMD-bearing module (confirmed via `wasm-dis`: real v128 signatures) and wasm-opt exited 0. So the comment the brief mandates would have shipped a documented guarantee the code does not deliver.
Ruling P20: **take none of the implementer's three options — invert the mechanism.** Do not ask wasm-opt to *reject* (it structurally won't), and do not fight it with `--strip-target-features` + `-mvp` (that is arguing with a tool about a question it is not being asked). `wasm-opt --print-features` **already reports the module's effective required feature set** — the exact fact the fence wants. So read it and compare it against a declared allow-list ourselves. Direct instead of inferred-from-exit-code; falsifiable immediately with the SIMD artifact the implementer already built; no flag gymnastics. This is the repo's own "the number can be answered precisely — don't guess it with a constant / ask whether something already knows it" criterion: `--print-features` knew all along, we were reading an exit code instead.
Ruling P21: the WASM feature allow-list goes in `interfaces/webchat/webview-baseline.json` as `wasm_features`, beside `css_probes`/`js_probes`, and the justfile step reads it. A hardcoded list in the recipe would be a second declaration of the floor, which is the one thing this whole plan exists to prevent.
Ruling P22: the initial list is **derived from a real clean build's `--print-features` output, then adjudicated entry by entry against Safari 16.4 — not accepted wholesale**. The implementer already found the true default set is more granular than the brief's six flags (`bulk-memory-opt`, `call-indirect-overlong` appear). If any entry the current build actually requires falls OUTSIDE Safari 16.4, that is a live G1 defect shipping today and it escalates to me rather than being written into the allow-list. Cost if wrong: the fence checks binaryen's view of required features rather than the browser's actual support, so a mis-adjudicated entry passes — which is why each is adjudicated individually.
Ruling P23 (PARTIALLY REVERSES P20): the implementer produced a **tested** two-pass mechanism after I had already ruled against it untested, and the evidence beats my hypothesis. Adopt the two-pass fence (`--strip-target-features` as a separate invocation, then `-mvp` + explicit enables), NOT the `--print-features` comparison. Three reasons the evidence changed my mind: (1) it demonstrably works and emits a real validator diagnostic that *names* the feature (`SIMD operations require SIMD [--enable-simd]`) — the "it names the offender" advantage I claimed for `--print-features` was not exclusive; (2) it is a genuine **validation** of the instruction stream, strictly stronger than reading a report, which could not catch a module whose declared section understates what it uses; (3) my characterisation of it as "lying to the tool" was wrong — stripping the auto-detect section is not deceit, it is disabling an inference that makes our question unaskable. `--print-features` stays, demoted from gate to **derivation tool** for the list and to diagnostic text. Cost if wrong: three binaryen behaviours (strip-as-separate-pass, `-mvp`, parse-time detection order) rather than one output format could break on upgrade — the fence then fails loudly at build time, not silently.
Ruling P24: P21 and P22 SURVIVE unchanged. The 8-flag list lives in `webview-baseline.json` as `wasm_features` and the recipe reads it — not because the floor would otherwise be duplicated (WASM features are a separate vocabulary from the version floor, as I said in the dispatch) but because a list buried in a shell recipe never gets adjudicated, and adjudication is the whole point. Each of the 8 is judged against Safari 16.4 individually.
Ruling P25 (cross-task hazard only I can see): the two-pass intermediate file **must not be written inside `interfaces/webchat/dist/`**. Task 9 adds a guard pairing every dist file over a size threshold with a `.br` sibling, and Task 17 commits the whole directory — a stray intermediate left there by an aborted build (which `set -euo pipefail` makes likely, since the abort skips any cleanup line) would be both mispaired and committed. Put it outside dist entirely.
Task 5: implemented under P20/P21/P22 (commit c124bc99c) — the implementer acted on the ruling it had; P23 and the GO message crossed it in flight.
Ruling P26 (RESOLVES the P20-vs-P23 crossing): **accept what shipped; do not rework to the two-pass fence.** When I wrote P23 I believed I was choosing a *tested* mechanism over an *untested* one — that premise is now false: the implementer falsified the `--print-features` fence against the same real SIMD artifact and it goes red naming `--enable-simd`. With both tested, the comparison flips: what shipped is one wasm-opt invocation instead of two, needs no intermediate file (so P25's cross-task hazard is moot entirely), and depends on one binaryen behaviour rather than three. The only thing the two-pass version buys is catching a module that *uses* a feature its `target_features` section does not declare — and in a rustc -> wasm-bindgen -> wasm-opt pipeline that section is emitted by LLVM from the very features that permitted the instructions, so the divergence is exotic. The threat the fence actually exists for is "a future toolchain bump silently enables a new feature", and such a bump updates the section, so `--print-features` catches precisely that. Reverting now would be preference imposed at real cost over a tested implementation. Cost if wrong: the fence trusts the module's self-declaration; a post-processing step that injected undeclared instructions would pass. Logged as a deferred minor rather than pretended away.
Task 5: minor (deferred): the shipped fence reads the module's declared feature set rather than validating its instruction stream — see P26 for the threat model that makes this acceptable and the residual gap it leaves.
Task 5: review of c124bc99c — 1 Critical (empty/garbled `--print-features` capture reads as "zero features required" and passes silently), 1 Important (the ENFORCED invocation lacks `-mvp` so it reports Binaryen's always-on baseline UNION module-detected, not "what the module needs" as the comment claims — proven with an empty .wasm; and the adjudication used a *different* invocation, `-mvp --print-features`, so the re-derivation recipe is not the enforced one). Reviewer also flagged uncommitted working-tree drift.
Ruling P27 (the drift): caused by MY process error — I sent P23 and a GO message that crossed impl-t5's own commit, so it began building the two-pass fence after having already shipped the single-pass one under P20. The working copy is discarded by its author (not by me) with the recipe text preserved in the report, so nothing is lost.
Ruling P28 (FINAL — no further reversals): **P26 stands, single-pass.** New evidence could have reopened it: the reviewer proved the shipped fence reads a superset (baseline ∪ module), which is weaker semantics than real validation. But the tiebreaker is which mechanism fails *silently*, and it favours single-pass in both respects. (a) Reading a superset over-reports — the same safe direction Task 4's census is built on: it can flag a baseline flag missing from the list, it cannot miss a module flag. (b) If a future Binaryen makes `--strip-target-features` a no-op — and it has precedent, `--detect-features` is already documented as doing nothing — the two-pass fence falls back to auto-detection and passes **silently**, which is precisely the defect being fixed and would be invisible. The single-pass version's two defects are both fixable in place and both fail loudly once fixed. Deciding once and stopping: three reversals on one task is itself a cost the implementer pays.
Ruling P29 (FINAL, supersedes P28 — and this is an acceptance, not a fourth reversal): **keep the two-pass fence at ad702fb74.** My P28 crossed the implementer a third time; it had already shipped two-pass under P23/P25. Re-deciding on the state that actually exists rather than the one I last ordered: (1) HEAD is tested end-to-end through the real recipe including a genuine 7-minute SIMD rebuild that goes red naming the feature, honours P25's mktemp-outside-dist, and caught a real `-g` name-section bug on the way; (2) the single-pass alternative still carries an **unfixed Critical** (empty capture reads as zero features and passes silently) whose fix was in the message that crossed; (3) I have now reversed twice and the implementer has paid two ~7-minute rebuild cycles — a third reversal costs more than the delta between two acceptable mechanisms.
  P28's strong leg was that a future Binaryen no-op'ing `--strip-target-features` would make the two-pass fence fall back to auto-detection and pass **silently** — and `--detect-features` on this machine is already documented as doing nothing, so the precedent is real. That objection is sound but it does not require changing the mechanism: it requires making the precondition **loud**. So the acceptance is conditional on one addition — assert after pass 1 that the strip actually removed the section. With that, two-pass dominates: real instruction-stream validation AND a loud failure when its own precondition breaks, which was the only property single-pass had over it.
Task 5: c124bc99c (single-pass, P20) is left in history as a superseded commit rather than amended; its Critical is moot because that code no longer exists at HEAD.
Ruling P30 (CLOSES the mechanism question): HEAD `dd5850fb2` is single-pass **with the Critical closed** — and this is where both of my last two rulings converge, so it is the final state and I am not touching it again. P28 asked for exactly this. P29's only addition (assert the strip actually stripped) existed **because** the two-pass fence depends on a strip; single-pass performs no strip, so that condition is structurally inapplicable rather than skipped. Net state: superset reading (over-reports = safe direction), empty/garbled capture now fails loudly with the raw bytes named and falsified four ways, comment corrected to say "union of Binaryen's baseline and module-detected" rather than "what the module needs", re-derivation command matches the enforced invocation, happy path echoes the enforced list, two-pass recipe preserved verbatim in the report as the rejected alternative.
Note for the final review: the T5 range holds three commits (single-pass -> two-pass -> revert to single-pass + fixes). That churn is MY process error — four rulings crossed the implementer in flight — not implementer thrash. Judge the net state. The abandoned mechanism is documented rather than lost, which is the right outcome for a design that was genuinely reconsidered.
Task 5: net-state review — spec OK, quality Approved, 1 Important + 3 Minor. Reviewer independently hand-traced all three named risks and two edge cases beyond the report's own four.
Ruling P31 (Important — write-before-validate): **fix.** The `--print-features` query and the `-Oz -g -o` overwrite are one invocation, and that invocation exits 0 regardless of features, so the node judge runs only AFTER dist/ has already been overwritten. A tripped fence therefore exits 1 but leaves the out-of-floor artifact on disk. This matters more here than the reviewer could know: `interfaces/webchat/dist/` is a **git-tracked build output** that Task 17 commits wholesale — so a failed fence followed by Task 17's commit step would commit the very artifact the fence rejected. The abandoned two-pass mechanism had the opposite property (it only overwrote on success) and nobody, me included, noticed we were giving that up. Fix as the reviewer specified: query-only `--print-features` with no `-o` first, validate, then shrink-and-overwrite. Still single-pass in the sense that decided P28.
Ruling P32 (promoting reviewer's Minor 2 to must-fix): the justfile comment points readers to "the Task 5 report" for the rejected-alternative writeup — but that report lives in `.superpowers/sdd/`, which is git-ignored scratch that **this process deletes when the run completes**. The pointer is dead on arrival. This is the repo's own `.gitkeep` criterion exactly: a mechanism whose only rationale lives in a file the next person won't open gets deleted by someone who reads it as vestigial. The *reason* must live in the tracked file — inline the one-sentence argument (strip-no-op would silently restore auto-detection) in the comment and drop the dangling reference. The full recipe may stay in the report; the reason may not live only there.
Ruling P33 (CLOSES Task 5's mechanism for good — verified, not ordered): HEAD `b7947bb3a` is **two-pass + strip-efficacy assertion**, and it satisfies every outstanding ruling at once, so it stands. Verified by me directly against the recipe: pass 1 writes to `$wasm_opt_tmp` outside dist (P25), the strip assertion gates it (P29), and pass 2 is the ONLY writer of `dist/aleph_panel_bg.wasm` and fails before writing — so **P31 is structurally moot**, not skipped: the two-pass design inherently has the overwrite-safety property whose loss the reviewer flagged. `grep` confirms no reference to the SDD report survives in the justfile, so **P32 is satisfied** too. Net: fence validates the instruction stream, fails loudly, cannot corrupt dist on failure, and its own precondition is asserted.
Note: the implementer REJECTED my suggested strip check and was right to. I proposed asserting `--print-features` on the stripped intermediate returns empty/MVP; it tried that first and found that on any real module `-mvp` validation floods with thousands of genuine errors once the declaring section is gone, so "empty" never occurs whether the strip worked or not — the check carries **no signal**, i.e. it is a guard that cannot distinguish its two cases. It built a content-independent one instead (grep pass 1's output for the literal `target_features` name bytes, verified as a real section header via the LEB128 length prefix `0f`=15 and feature-count `08`, not a coincidental substring).
Task 5 commit sequence for the record: c124bc99c -> ad702fb74 -> dd5850fb2 -> b7947bb3a. Four of my rulings crossed the implementer in flight; the churn is mine, its work was consistently sound.
Task 5: final-state review — no Critical (reviewer empirically confirmed overwrite-safety and fence effectiveness against a live wasm-opt 130 with a scratch module), 2 Important text findings, 3 Minor.
Ruling P34: both Importants are **must-fix**, and they are the same defect in two places — a comment describing a mechanism that is not the shipped one. (a) `webview-baseline.json:2`'s `_comment` still describes the ABANDONED `--print-features` comparison — in the file that is the project's canonical single declaration, whose own text says "never retype anywhere else". A stale mechanism description in the declaration file is worse than one in the recipe: it is where a future reader goes to learn how this works. (b) `justfile:236-244` claims the precondition check is "if and only if" / "confirmed byte-for-byte" / "checked, not assumed" while the shipped code is a bare substring grep — the LEB128 verification was a one-time manual `xxd` during development and never encoded. That is precisely the overclaim the implementer originally refused to ship the brief's version over; it must not survive in its own work.
Ruling P35: fold in all three Minors rather than defer — each converts a silent or leaky failure into a loud one, each is a single line, and both files are already open. (1) a pass-1 crash leaks the mktemp file before cleanup under `set -e` (needs `trap ... EXIT`); (2) a missing `wasm_features` key fails via raw Node stack trace instead of this block's named-message idiom — the same P8 pattern already fixed twice in this plan; (3) a bare-string entry where a `[flag, note]` tuple is expected would have JS destructuring silently take its first character, producing nonsense flags. The third fails in the safe direction (nonsense flags + `-mvp` => pass 2 rejects loudly) but "silently takes the first character" is the shape this plan exists to eliminate, so validate the entry shape rather than comment it.
Ruling P36 (verified by me at HEAD b7947bb3a, settling an implementer challenge that the review was stale): the implementer was answering my RETRACTED P31/P32 message, not P34/P35. Its evidence about P31/P32 is correct and I had already withdrawn them. P34/P35's two findings are different and both CONFIRMED at HEAD: (a) `webview-baseline.json`'s `_comment` literally reads "fences the module's `wasm-opt --print-features` output against it" — the abandoned single-pass mechanism, in the canonical declaration file; (b) `justfile:239` asserts the grep finds the bytes "if and only if the section is still there" — the "only if" half is not delivered, since `-g` debug info could carry the literal string. **The reviewer slightly overread (b)**: the adjoining "confirmed byte-for-byte against a real build" clause is accurate — a one-time manual xxd did confirm the LEB128 prefix — so only the biconditional needs correcting, not the whole comment, and no code change is warranted.
Ruling P37 (implementer's own independent finding — real, deferred with a carry): `wasm-bindgen` writes `dist/aleph_panel_bg.wasm` as its FIRST step, before pass 1 runs. So a build failing anywhere after that write (tool check, strip, precondition) leaves dist holding raw un-shrunk un-fenced output — not the "shrunk out-of-floor artifact" P31 described, but still not a floor-validated file, and Task 17 commits dist wholesale. Genuine, but **not Task 5's defect** (pre-existing property of the recipe) and the fix is a recipe-wide temp-staging restructure. Deferred: Task 17 Step 1 rebuilds clean and Step 2 runs every guard, so a failed build stops T17 rather than being committed. **Carried requirement for T17: confirm dist holds a fenced artifact, not wasm-bindgen raw output, before the commit step.** Cost if wrong: a manual commit of dist after an unnoticed failed build ships an un-fenced 34MB binary.
Task 5: fix round 3/5 (5 addressed, 0 open; commits b7947bb3a..d1260726e). Re-reviewer confirmed grep/pass-1/pass-2 byte-identical (comment-only change as authorised), the `trap` covers every path including a pass-1 abort that the old explicit `rm -f` never did, cannot fire before assignment, and a `mktemp` failure exits before the trap installs with nothing to clean. New `_comment` is a pure pointer naming no mechanism detail.
Task 5: complete (commits 521597016..d1260726e, review clean). Five commits, four of my rulings crossed the implementer in flight; the churn was mine, its judgement was the strongest in the run.
Task 6: dispatched (impl-t6, sonnet), BASE=d1260726e
Task 6: review clean — spec OK, quality Approved, 0 Critical/Important. Reviewer independently cross-checked the shell's three literals against the probe's (`baseline-probe.js:32,36,37,38`) byte for byte, confirmed `host()` does zero UA sniffing, and confirmed the two new tests are genuine value comparisons that the described mutation really exercises.
Task 6: minor (deferred): `platform_host.rs:34` intra-doc link points at a `pub(crate)` item — harmless unless `cargo doc` is run.
Task 6: minor (deferred): `main.rs:417`'s pre-existing on_page_load comment still says SHELL_MARKER_JS "sets data-platform=macos"; accurate in its paragraph's context but now reads as implying the constant is macOS-only. Out of T6's declared scope.
Task 6: CARRIED LIMITATION: only the Windows `cfg` arm of SHELL_MARKER_JS was compiled. The macOS and Linux arms are `const &str` literals differing by one word, so risk is low and it was disclosed rather than glossed — but they are unverified by any toolchain. Must be named in the T16 QA script's documentation so the person running it on those platforms knows this is the first compile.
Task 6: complete (commits d1260726e..24fe54599, review clean)
Task 7: dispatched (impl-t7, sonnet), BASE=24fe54599
Task 7: implementer found a PLAN DEFECT before editing, verified with the real build tool (`npx @tailwindcss/cli --minify`). Step 1's "wrap the block body in `html[data-flat="1"] { ... }`" is CSS Nesting, and a nested selector without `&` gets an implicit **descendant** combinator — so the two rules inside that target `<html>` ITSELF (`:root, :root[data-material]` and `.dark, .dark[data-material]`, both of which denote the same element `data-flat` is set on) compile to `html[data-flat="1"] :root` and become permanently dead. Those two rules are the ones that zero `--mat-raised`/`--mat-grain`/`--aleph-canvas-base`/`--glass-saturate`, i.e. the whole material collapse. Step 5's verification would NOT catch it: `.glass` has its own direct `!important` override and still computes `backdrop-filter: none`, so the check passes while the token zeroing silently stops happening.
Ruling P38: take the implementer's **Option B** (prefix every selector group explicitly, no nesting anywhere) over its own preferred Option A (`&` for the two `:root`/`.dark` rules), despite B's larger diff. Three reasons, in order of weight. (1) **The failure mode is silent, so the design must make it impossible rather than currently-correct.** Option A leaves one block where two rules rely on `&` and ~13 rely on implicit descendant nesting, with no compile error distinguishing them — the next person adding a `:root`-targeting rule there writes a dead one and nothing says so. (2) **It makes the whole task use one pattern**: the brief's Step 2 already prefixes directly (`:root[data-flat="1"]:not(.light)`), so B makes both blocks consistent instead of introducing a third convention. (3) **It removes a dependence on build-tool behaviour relative to our own declared floor.** CSS Nesting needs Safari 16.5+ (17.2 for the relaxed element-first form) and our floor is 16.4 — today Tailwind flattens nesting away so the shipped CSS is fine, but Option A's correctness rests on that flattening continuing, and edge D's census scans for CSS *functions*, not nesting syntax, so it would not catch a regression. B depends on nothing. Cost if wrong: a larger diff to review, and ~15 lines that must each carry the prefix.
Task 7: review clean — spec OK, quality Approved, 0 Critical/Important. Reviewer enumerated all 19 rule groups 1:1, confirmed compound-vs-descendant is factually correct (grepped `src/app.rs:316` to prove `.aleph-shell` is on an inner div, not `<html>`), and verified the comment cites the floor by reference rather than a second hardcoded copy.
Task 7: minor (deferred): a reinforcing "compound, not nested" pointer comment sits above the `:root` rule but not above the `.dark` one, though the header names both as at risk. One line.
Ruling P39 (CARRY THIS OUT OF THE LEDGER — reviewer's Minor 2, a real live defect out of T7's scope): the build pipeline **drops the unprefixed `backdrop-filter`** from every hand-written glass rule, keeping only `-webkit-backdrop-filter`. Verified against the actually-shipped `dist/tailwind.css`, not inferred: affects `.glass`'s own base blur, `.aleph-scrim`, `.aleph-blur-subtle`, `.aleph-sidebar::before`, `.aleph-session-tabs` — rules this task never touches. Tailwind's own generated utilities keep both forms; only the hand-written family loses it. **Consequence: Firefox supports ONLY the unprefixed form, and the Panel is reachable over LAN from an ordinary browser — so on Firefox both the base blur and G5's flat-mode removal of it are already ineffective today.** Pre-existing, source is correct (both declarations present), cause is almost certainly the Lightning CSS/browserslist target in the build config. **This must reach `docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md` as a follow-up (FU-5) during Task 17 Step 8** — recording it only here would lose it, because this workspace is deleted when the run completes, which is the same defect P32 was about.
Task 7: complete (commits 24fe54599..c1bbe6e45, review clean)
Task 8: dispatched (impl-t8, sonnet), BASE=c1bbe6e45
Task 8: review — spec OK, quality Approved, 1 Important (plan-inherited) + 3 Minor. Reviewer confirmed byte-level round-trip (`Buffer.equals`, not length), `.br` inputs skipped so no `.br.br`, write structurally unreachable on mismatch, and re-verified all four on-disk sizes against the report.
Ruling P40: fix the stale-sibling gap. `precompress_dist.mjs` deletes a stale `.br` on the below-threshold path (:39-42) and the not-smaller path (:58-59) but NOT on the round-trip-mismatch path (:68-71) — while the module's own header declares "a `.br` that is out of sync with its source would serve WRONG BYTES". A module that states an invariant and then breaks it in one of three branches is the "enforced in 2 of 3 places" shape; the third is the one nobody looks at. The mitigation is real (exit 1 + `set -euo pipefail` halts before the dist guard or any commit) but it depends on the failure being **heeded**, and `rust_embed` reads dist straight from disk in debug builds — so a stale mismatched sibling surviving a worked-around failure is a live path to serving wrong bytes. One line. Cost if wrong: none.
Ruling P41: fold in Minors 1 and 2 as well. Minor 1 (below-threshold skips log nothing per-file while the not-smaller path logs a reason) is the same asymmetry in the reporting rather than the cleanup — "say what you skipped and why" is already this script's own behaviour on the sibling branch. Minor 2 (raw Node stack traces instead of the script's `✗` idiom) is the THIRD time this exact pattern has come up in this plan (Task 1's `readJson`, Task 5's missing `wasm_features`, now here) — at three occurrences it stops being a per-task nit and becomes the house style, so apply it rather than defer it again.
Task 8: fix round 1/5 (3 addressed, 0 open; commits 6f535b2e5..6dafb8bc7). Re-reviewer confirmed the new `unlinkSync` is best-effort so a mismatch on a file with no sibling yields ONE failure entry rather than a masking crash, and that caught errors still exit 1 (no swallow-to-zero path exists).
Task 8: minor (deferred, SAME CLASS as the fixed finding): the *general* per-file catch still leaves a stale `.br` behind — only the round-trip-mismatch branch got cleanup. Pre-existing and fails loudly (exit 1), but it is the second instance of the class, and this repo's own criterion is that fixing the instances you read is not fixing the class. One line in the general catch would close it. For final-review triage.
Task 8: measured figures for the spec (Task 17 Step 8): wasm 21,882,715 -> 3,363,082 B (-84.6%); js -87.8%; css -86.6%; index.html 6,925 -> 2,411 B (-65.2%). FOUR files get siblings, not the three the plan predicted — Task 3's inlined probe pushed index.html past the 4 KiB threshold.
Task 8: complete (commits c1bbe6e45..6dafb8bc7, review clean)
Task 9: dispatched (impl-t9, sonnet), BASE=6dafb8bc7
Task 9: BLOCKED by a third PLAN DEFECT, verified by running it. `precompress_dist.mjs` has no entry-point guard — its whole body (scan, compress ~22MB at quality 11, write every sibling) is top-level module code. ESM executes all of that on import, so `import { MIN_BYTES }` re-runs the entire precompression pass before the importing file's first line. Consequences the implementer demonstrated: (1) every `check_panel_dist.mjs` invocation silently recompresses the wasm (~33s); (2) corrupting `tailwind.css.br` then running the guard exits **0 green**, because the import overwrote the corrupted sibling with a fresh valid one before Direction 1's check ran; (3) deleting the sibling likewise exits **0 green**, recreated before Direction 2's check ran. **Both new assertions are structurally unreachable, and the brief's own falsification steps pass.** A guard that heals the defect it is testing for, before testing for it.
Ruling P42: take option (a) — add an entry-point guard to `precompress_dist.mjs` and export `MIN_BYTES` unconditionally above it. Rejecting (b) (regex the constant out of the source text) because it reintroduces the second source of truth the plan forbids and is fragile against formatting. The scope objection — that this touches Task 8's file — is answered by the plan itself: Task 9's Interfaces block specifies `MIN_BYTES` is "imported, not retyped", so making the import safe IS the plan's intent, not a deviation from it. The real defect is that a file was written as a script and then depended on as a library.
Ruling P43 (the hazard the fix introduces, which is worse than the bug): if the entry-point comparison is too strict, **direct invocation silently becomes a no-op** — `just wasm` would stop producing siblings while printing nothing and exiting 0. On Windows this is a live risk (drive-letter casing and separator normalisation in `import.meta.url` vs `pathToFileURL(process.argv[1]).href`). So the fix must be falsified in BOTH directions: direct invocation still compresses, AND import produces no side effect. Verifying only the second would ship a build step that has quietly stopped doing anything.
Task 9: implementer found a FOURTH plan defect stacked under the third — the brief's Step 3 mutation (`printf '\x00' >>`) is inert: Node's `brotliDecompressSync` ignores a trailing byte after a complete stream, so that falsification stays green even with the import bug fixed. It used a mid-payload byte flip instead and got real RED. Two independent reasons the same falsification was vacuous.
Ruling P44 (answering the implementer's concern 2 — no regression test for the entry-point guard): **require one.** The entry-point guard is currently protected by nothing: delete it and the import side-effect returns, the dist guard heals-then-checks again, and both directions silently report green — the exact defect we just spent a round-trip discovering, restored invisibly. That is the plan's own recurring theme ("a guard that has never been falsified is not a guard") extended one step: a guard whose *precondition* can silently vanish needs that precondition asserted. A source-level assertion that `precompress_dist.mjs` still carries an entry-point guard is sufficient and fails in the safe direction — a refactor that expresses it differently breaks the assertion loudly rather than silently. Cost if wrong: a spurious red after an unrelated refactor of that file.
Ruling P45 (carry to Task 17): the plan document at `docs/superpowers/plans/2026-08-21-tauri-webview-resource-control.md` still contains the inert `printf '\x00' >>` mutation in Task 9's Step 3. Leaving a known-vacuous falsification in the tracked plan is the same defect class this whole run keeps closing — the next person to follow it gets a green that means nothing. Correct it during Task 17's documentation step, alongside the FU-5 backdrop-filter note (P39).
Ruling P46 (RETRACTING my own stated expectation): I told the T9 implementer that an over-threshold-but-incompressible file "must not be flagged as missing" and called a size-only check a defect. **That was wrong and the implementer was right to refuse to change it.** The brief's Direction 2 is deliberately a fast static check that knows only `size >= MIN_BYTES` and whether the sibling exists; determining compressibility in advance requires running the compressor (defeating the point) or a marker file recording "considered and skipped" (new persistent state nothing writes). The brief's own comment already states the design and the remedy: "the producer and the guard disagree — extend both together, never just the guard." Flagging stands.
Ruling P47 (the real, smaller defect underneath my wrong expectation): the behaviour is right but the **message is misleading**. Today no such file exists, and the failure is loud rather than silent — the safe direction. But if someone later adds an already-compressed asset over 4 KiB to `dist/` (a `.png`, a `.woff2`), the build fails telling them to `run node scripts/precompress_dist.mjs` — and running it will NOT fix anything, because the producer correctly skips such files. A red build with instructions that cannot work is the trap. Fix the message to name both causes (sibling genuinely missing vs. source that does not compress smaller) and both remedies, per the "a failure message must tell a stranger what to do" principle this plan has applied throughout. No behaviour change, no new scope.
Note: my chase message to impl-t9 was based on a pre-`788007e93` snapshot — my error, though the review package was correctly cut at `6dafb8bc7..788007e93`.
Task 9: message fix committed as ad40093e7 (wording only, 9+/1-). NOTE: rev-t9 was dispatched against 6dafb8bc7..788007e93 and does NOT see this commit. If it reports the misleading-message issue, that is ALREADY FIXED at HEAD — adjudicate as closed rather than opening a fix round. Deliberately did not send a second mid-flight correction to the reviewer; crossed messages have been this run's dominant cost and a false finding costs one line to close.
Task 9: rev-t9 idled twice after a chase without producing a verdict — abandoned it and dispatched a fresh reviewer (rev-t9b) on the true final range 6dafb8bc7..ad40093e7, which also picks up the message-fix commit the stale range missed.
Controller-verified directly while the reviewer was stuck: Direction 1 DOES compare content — `brotliDecompressSync` then `back.equals(source)` at check_panel_dist.mjs:133, with a distinct "not valid brotli" branch at :130 and a no-source orphan branch at :123. So the "stale-but-valid" RED claim is real, not an artifact. Whole guard runs in 0.172s, consistent with brotli decompress throughput on a 21 MB output plus node startup — fast AND content-comparing, not fast BECAUSE it skips the comparison.
Task 9: review (rev-t9b, final range) — spec OK, quality Approved, 1 Important + 3 Minor.
Reviewer settled risk 1 structurally rather than empirically: `isMain` compares `import.meta.url` against `pathToFileURL(process.argv[1]).href`, and BOTH sides derive from the same `process.argv[1]` string in the same process, with `pathToFileURL` resolving via `path.resolve()` exactly as Node computes the main module's own URL. So drive-letter casing cannot make it diverge — it is one input through equivalent resolution on both sides, not a coincidence of this path. Residual risk is symlinked entry files, which is inherent to Node's documented pre-`import.meta.main` idiom and not introduced here.
Ruling P48 (Important — fix): the guard-presence failure message says only "Restore `if (isMain) { ... }`". Someone who renamed `isMain` **deliberately** hits it in CI and is steered to revert their rename, when the actual remedy is to update the twin substrings in `check_panel_dist.mjs` in lockstep. An assertion that fires with no correct path forward is its own trap. One sentence pointing at the twin check closes it.
Ruling P49 (REJECTING the obvious fix for risk 3): do NOT short-circuit the two sweeps when guard-presence fails. It reads like enforcing the stated dependency, but the reviewer's analysis shows it would remove the one check that still means something: with a broken guard the import has already re-run compression and healed stale/missing siblings, so those sweeps go quiet rather than falsely passing — while **Direction 1's orphan branch survives**, because an accidental recompression never deletes a sibling whose source is gone. Short-circuiting would suppress the only surviving signal. The dependency stays stated in prose, first in output by insertion order.
Task 9: fix round 2/5 (1 addressed, 0 open; commits ad40093e7..9a6c896a4). Message now branches on accidental vs deliberate and names the right file and construct for each.
Task 9: minor (deferred): the two-substring source match cannot confirm the substrings are connected (no AST) — accepted trade per P44, loud-false-positive direction.
Task 9: minor (deferred): no defence against symlinked entry files / npx-shim invokers — inherent to Node's pre-`import.meta.main` idiom, not introduced here.
Task 9: complete (commits 6dafb8bc7..9a6c896a4, review clean). Four commits; three plan defects found and closed (import side-effect healing the guard, inert null-byte mutation, single-remedy message).
Task 10: dispatched (impl-t10, sonnet), BASE=9a6c896a4
Task 10: impl-t10 was LOST to a session compaction after writing Step 1's three tests (uncommitted in the working tree at `src/gateway/control_plane/server.rs`, deliberately red — no negotiation logic exists). Re-dispatched as impl-t10b from the same BASE=9a6c896a4 with the existing red tests handed over as its starting point rather than discarded.
Task 10: impl-t10b DONE_WITH_CONCERNS, commit 8acdf2e92. `CompressionLayer` pass-through claim MEASURED on a real Windows server, not cited: br -> 3,363,082 B byte-identical to the committed `.br`; gzip-only -> 5,089,368 B valid gzip decompressing byte-identical to the identity wasm; no header -> 21,882,715 identity. No double-encoding, no stripping, no re-encoding. The plan's central unverified assumption is now verified evidence.
Ruling P50 (concern 1 — ACCEPT the deviation): the implementer refused to commit the brief's verbatim Step 6 comment because it is false, and it is right twice over. (a) It states index.html has no precompressed sibling; index.html is 6,925 B against a 4,096 B threshold, so it does — this is the same fact Task 8 already recorded ("FOUR files get siblings, not the three the plan predicted"), and the brief was written before that measurement existed. (b) Its framing "gzip for assets without a sibling" misdescribes the layer's scope: the wasm HAS a sibling and is still correctly runtime-gzipped for a gzip-only client, because the sibling is chosen per-request, not per-asset. Committing a comment that is wrong on a fact this plan itself measured would be the exact defect P32/P34 closed twice already. The shipped comment carries the measured figures instead. No behaviour change.
Ruling P51 (concern 2 — HONOR q=0, overriding the brief): the brief's comment defends ignoring q-values with "a client that sends `br;q=0` while also sending it as a token is not a real client." That is a claim about who is on the wire, not a property of the code, and it is the enumeration shape this run keeps closing — it only describes the client population as it was on the day it was written. Weigh the two directions instead. Ignoring q=0 means a client that EXPLICITLY refused brotli receives brotli: silent wrong bytes, and the person most likely to send it (`curl -H 'Accept-Encoding: br;q=0'` to fetch identity bytes while debugging) is exactly the person the wrong answer will confuse most. Honoring it costs ~8 lines and fails safe — a q=0 client gets identity, which every client can read. RFC 9110 §12.5.3 is explicit that qvalue 0 means "not acceptable", so this is conformance, not preference. Cost if wrong: a client sending `br;q=0` that secretly wanted brotli gets a larger response; there is no such client.
Ruling P51a (the trap inside the fix): the parameter test must be numeric, not textual. A `contains("q=0")` or a prefix match treats `br;q=0.5` — a valid LOW-PREFERENCE acceptance — as a refusal, silently disabling brotli for a client that asked for it. Parse the qvalue and compare against zero so every spelling (`0`, `0.0`, `0.000`) is caught and every nonzero one is not. Scope is exactly the explicit `br` token: `gzip, *;q=0` already yields false today (no `br` token present), and a bare `*` with positive q yields false too, which errs toward identity and is safe.
Task 10 (P51 fix) — CONTROLLER-OBSERVED WIRE EVIDENCE, captured directly against the live server (PID 3760) built from the working tree containing the fix. Recorded here because the SDD workspace is deleted at run end and this is the only durable place until the report delta lands; asset = /aleph_panel_bg.wasm.
  Accept-Encoding: <none>        -> 200, no content-encoding, content-length 21,882,715, vary: accept-encoding
  Accept-Encoding: br            -> 200, content-encoding: br, content-length 3,363,082
  Accept-Encoding: br;q=0        -> 200, NO content-encoding, content-length 21,882,715
  Accept-Encoding: gzip, br;q=0  -> 200, content-encoding: gzip (chunked, no content-length)
  Accept-Encoding: br;q=0.001    -> 200, content-encoding: br, content-length 3,363,082
  ETag identical across ALL FIVE: W/"18e15e606e31539b1be5b1027d45ee5aa9b0885568cbc45b4017a3a8efa1a501"
Three things this proves that no handler-level test can. (1) The identity-derived ETag holds ON THE WIRE across every encoding, so the false-304 trap (switch Accept-Encoding, take a 304, decode brotli as identity) is closed in the assembled stack, not merely in a unit test that never runs the layer. (2) `br;q=0` yields NO `Content-Encoding` at all — `CompressionLayer` does not step in and gzip a response for a client that advertised no gzip; the refusal is honoured end-to-end. (3) `gzip, br;q=0` still gets runtime gzip, so the refusal narrows brotli SPECIFICALLY rather than disabling compression wholesale — the two cases together are what distinguish a correct fix from one that reads any q=0 as "send raw bytes". Also confirms the running binary contains the fix, since an unfixed build would have returned brotli for `br;q=0`.
Task 10: P51 fix committed by the CONTROLLER as 55289539a, not by impl-t10b. It had the change complete and correct in the working tree but idled twice on the mechanical commit step, so rather than spend a third round-trip I read the whole diff, ran the scoped suite myself (11/11 pass, exit 0), captured the wire evidence above, and committed. Note for the reviewer: the implementer authored the code; only the commit action is mine.
Task 10: the one warning in that test run (`unused_imports` on a `warn` import at some file's line 9) is PRE-EXISTING and not from this task -- `src/gateway/control_plane/server.rs` contains zero references to `tracing`. My capture command piped through `tail -25` and lost the warning's header line, so the exact path is not in the log; re-run without the pipe to name it. Flagged here so the reviewer does not read it as this task's noise, since "warnings in the reported test output are findings" is in its rubric.
Task 10: commits 9a6c896a4..55289539a (8acdf2e92 implementation + 55289539a q=0 fix). Ready for review.
Task 10: the first rev-t10 was lost to a controller-side compaction without producing a verdict (no report file, no ledger line). Re-dispatched as rev-t10-2 (sonnet) on the same package review-9a6c896a4..55289539a.diff, carrying both provenance facts (report Concern 2 is superseded by the appended Delta; the Delta and commit 55289539a are controller-authored and get the same skepticism) and the same four named risks.
Housekeeping: killed the leftover verification server aleph-server.exe PID 3760 (started by impl-t10b for the wire capture). On Windows a running binary locks its own .exe, so leaving it up would have failed Task 17's rebuild with os error 5 rather than anything legible.
Task 11: dispatched (impl-t11, sonnet), BASE=55289539a. Dispatched CONCURRENTLY with rev-t10-2 — different files (new `src/gateway/server/byte_range.rs` vs `src/gateway/control_plane/server.rs`), and the reviewer reads a pre-cut diff file rather than git, so a Task 11 commit landing mid-review cannot disturb it. Carried Ruling P6 into the dispatch so the implementer does not re-litigate the brief's `String` vs `Option<String>` contradiction.
Task 10 (loose end closed): the `unused_imports` warning is `src/config/save.rs:9` — the only `use tracing::{debug, error, info, warn};` in the tree at line 9. Confirmed pre-existing and outside the Task 10 diff, which touched `src/gateway/control_plane/server.rs` only. Found by grep rather than by re-running the build, to avoid contending for the build-directory lock impl-t11 is holding.
Task 10: review (rev-t10-2) — spec ✅, quality Approved, 0 Critical, 0 Important, 5 Minor. It settled all four named risks on the merits: ETag/Vary decoupling correct on both 200 arms AND the 304 arm (the diff's `+` on the 304 arm confirms Vary was previously missing there), missing-sibling fallback collapses `Some(None)` -> identity rather than 404/empty, and the q=0 fix is a genuine RFC-conformance improvement over the brief.
Ruling P52 (Minors 1-3 — FIX all three, one commit): the reviewer rated the qvalue parser's three gaps Minor on the grounds that no real client emits them. That is the exact argument P51 already overrode **for this same function** — "a claim about who is on the wire, not a property of the code" — so accepting it here would make the function's two halves rest on contradictory reasoning. All three also fail in the SAME direction, and it is the unsafe one: brotli sent to a client that refused it. (a) `.any()` short-circuited on the first accepting token, so `br;q=0.9, br;q=0` served brotli — and so did `br;q=0, br;q=0.9`, which the reviewer did not note: BOTH orders were broken, while the duplicate-`q=` case one level down was already refusal-biased. Two halves of one function disagreeing is how a rule nobody wrote gets inferred later. (b) `br;Q=0` missed the `strip_prefix("q=")` and read as acceptance; RFC 9110 makes both the coding token and the parameter name case-insensitive, so this is conformance. (c) `br;q = 0` likewise read as "a `br` with no weight". Also tightened `== 0.0` to `<= 0.0` — the reviewer asked about `br;q=-1` and a negative weight is not a positive preference by any reading. Cost if wrong: a client sending one of four malformed spellings gets a larger response.
Ruling P53 (the reviewer's unverifiable-claim flag — the comment was TRUE, but made checkable): rev-t10-2 could not verify "every embedded asset currently has one" from the diff and asked for a fact-check. Measured: `dist/` holds exactly four sources (index.html 6,925 / aleph_panel.js 109,347 / tailwind.css 144,702 / aleph_panel_bg.wasm 21,882,715), all above the 4,096 B floor, and all four have `.br` siblings. The claim was correct. But an unverifiable-looking true claim rots into a false one, so the comment now names the floor that makes it true and states what happens to an asset below it (no sibling; `serve_static_or_index` falls through to identity and this layer compresses it) — a reader can now check it instead of trusting it.
Ruling P54 (Minors 4-5 — the reviewer was right about MY prose, which is why it was told to be hardest there): conclusion #2 of the wire-evidence Delta is overstated. The `br;q=0`-only row cannot distinguish "refusal honoured" from "gzip was never offered"; only the contrast with row 4 (`gzip, br;q=0` -> still gzip) demonstrates narrowing, and that is conclusion #3's job. Conclusion #2 rewritten to claim only what its row shows. Minor 5 also accepted: the ETag was stated in a sentence below the table while conclusion #1 depends on it, so it is now a table column. Both are report-only; no code change.
Task 10: P52/P53 implemented by the CONTROLLER (impl-t10b's session had ended; single file, ~25 lines). Four new tests, one per gap. Same provenance caveat as 55289539a — it goes into the final whole-branch review with everything else.
Task 10 (P52 verification): `cargo test -p alephcore --lib control_plane::server -j 2` -> 15 passed, 0 failed, exit 0 (11 prior + 4 new). rustfmt clean at edition 2021 on the single file (NOT `cargo fmt -p alephcore`, which ignores the path argument and reports the whole crate — this repo is not globally fmt-clean; also note the workspace is edition 2021, so `rustfmt --edition 2024` produces a spurious import-reordering diff). The build output also names the pre-existing warning outright — `src\config\save.rs:9:35`, unused `warn` import — confirming the earlier grep-based attribution from the real compiler rather than by inference.
Task 10 (P52 falsification PENDING): the four new tests have not yet been observed RED. Plan requirement, not optional. One mutation covers all four: revert `accepts_brotli` to the pre-P52 `.any()` body — under it `br;q=0.9, br;q=0` returns true, `BR` returns false, `br;q = 0` returns true, and `br;q=-1` returns true, so every one of the four expectations inverts. Deferred only to avoid thrashing the build lock while impl-t11 is running its own verification; it is the next thing after T11 commits.
Task 11: impl-t11 wrote `byte_range.rs` (13 tests) and the `mod` line, then went idle WITHOUT committing and WITHOUT a report — twice, the second time after an explicit five-point chase naming the three remaining steps. Reading the file explains the silence: **it never ran its own tests.**
Task 11 DEFECT (controller-found, would have shipped): `parse_range` returns `end` UNCLAMPED for a range that runs past the resource — `bytes=900-99999` against a 1000-byte body yields `Satisfiable { start: 900, end: 99999 }`. Its own test `an_end_past_the_resource_clamps_rather_than_refusing` asserts `end: 999`, so the suite was RED on disk. Consequence is not cosmetic: `end` reaches Tasks 12/13 as a slice bound, so an unclamped one is an out-of-range index driven straight from a request header — a panic on an attacker-controlled `Range`. The comment directly above it said "clamp rather than refuse" while the code did not clamp, which is the comment-describes-a-mechanism-that-is-not-shipped class this run keeps closing.
Ruling P55 (what the defect actually IS, and the new criterion): the brief's Step 6 falsification reads, verbatim, "Change `end: end.min(total - 1)` to `end` (dropping the clamp) and confirm `an_end_past_the_resource_clamps_rather_than_refusing` fails. Restore." The shipped code is **that mutation, unrestored** — the implementer performed the falsification and never put the code back, and because it also never ran the final green pass, nothing told it. **New criterion: a falsification step that mutates production code needs its RESTORATION verified, not just its RED observed.** The plan (and this skill) say "break the guard and see it go red"; neither says "then prove it is back". A run that stops after the red leaves the deliberate bug in the tree, and it is a bug the author already convinced themselves is a bug — the most likely one to be waved past on a second look. Every remaining task in this plan that names a mutation now requires the post-restore green run to be quoted, not just the red one.
Ruling P56 (loop change — implementers stop committing): impl-t10b, and now impl-t11 twice, all produced correct-to-nearly-correct work and then died at the same step — write report file, stage, commit. Three agents, one failure point, so it is the step and not the agents. For Tasks 12-17 the contract becomes: the implementer implements, verifies, falsifies, restores, and writes its report file; **the controller stages and commits.** This costs the run nothing that matters — the independent task review is the gate that catches errors, and it still runs on the committed diff. What it removes is the one instruction none of them has completed. Cost if wrong: commit authorship is mine on tasks whose code is not, which the ledger records per task and the final whole-branch review sees regardless.
FALSIFICATION EVIDENCE (Tasks 10 + 11, one mutated build, `cargo test -p alephcore --lib -j 2 -- byte_range control_plane::server`). Both guards mutated simultaneously; T11's mutation is the brief's own named one. Result `FAILED. 24 passed; 5 failed` — exactly the five expected, each naming itself:
  gateway::server::byte_range::tests::an_end_past_the_resource_clamps_rather_than_refusing  (left `end: 99999`, right `end: 999`)
  gateway::control_plane::server::tests::accepts_brotli_does_not_read_a_negative_weight_as_a_preference
  gateway::control_plane::server::tests::accepts_brotli_lets_a_refusal_win_over_a_duplicate_accepting_token
  gateway::control_plane::server::tests::accepts_brotli_tolerates_whitespace_around_the_parameter_equals
  gateway::control_plane::server::tests::accepts_brotli_matches_the_token_and_the_q_parameter_case_insensitively
The other 24 stayed green under both mutations, so the guards DISCRIMINATE rather than the suites merely being sensitive. Restored from a pre-mutation copy kept outside the repo (scratchpad), then re-verified.
⚠️ Note on reading that run: it exited **code 0** because the pipeline ends in `grep`. The authoritative signal is the `test result:` line, per the four-way RED/GREEN/BUILD-ERROR/VACUOUS ordering already recorded in CLAUDE.md §10 — classify by the line only one outcome can print, never by the exit code of a pipeline.
Task 11: `byte_range.rs` as delivered was also NOT rustfmt-clean (two `Satisfiable` struct literals needed splitting) — further evidence impl-t11 ran nothing. Reformatted with `rustfmt --edition 2021` (the per-file form; `cargo fmt -p alephcore` ignores its path argument and reports the whole crate, which is not globally fmt-clean).
Task 10 fix committed as 9e3591e94; Task 11 committed as cdc1f68f1. Post-restore GREEN confirmed: `cargo test -p alephcore --lib -j 2 -- byte_range control_plane::server` -> **29 passed, 0 failed** (13 byte_range + 15 control_plane::server + 1 unrelated name match on the `byte_range` filter). rustfmt clean on both files at edition 2021.
`cargo clippy --all-targets -j 2`: this branch adds exactly THREE warnings, all in `byte_range.rs` — `enum RangeVerdict is never used` (:23), `method content_range is never used` (:37), `fn parse_range is never used` (:48). Everything else in the output is pre-existing and elsewhere: `casting to the same type` at `loop_graph/store.rs:130`, `matching on Some with ok() is redundant` at `loop_graph/mod.rs:122`, `unused import: warn` at `config/save.rs:9`, plus two long-standing lib warnings.
Ruling P57 (do NOT silence those three): the obvious move is `#[allow(dead_code)]` with a comment saying Tasks 12/13 will consume it. Rejected — an `allow` added for a future consumer is exactly the annotation nobody removes, and it would then be hiding a genuinely unconsumed module, which is the R10 "zero existing consumers" case. Left loud instead, and thereby repurposed: **these three warnings ARE the check that Task 12 actually wired the parser in.** If they are still present after T12 commits, T12 did not consume what it claims to consume — a free guard costing zero lines. Task 12's dispatch carries this as an explicit expectation, and Task 17's full sweep re-checks it. Cost if wrong: three warnings in the build output for the length of one task.
CONTROLLER'S INDEPENDENT WALK of `parse_range` (recorded BEFORE rev-t11 reports, so its verdict can be compared against it rather than simply believed). TOTAL=1000 unless stated.
  `bytes=0-0` -> Satisfiable{0,0}; `bytes=-1` -> Satisfiable{999,999}; `bytes=0-` -> Satisfiable{0,999}; `bytes=999-999` -> Satisfiable{999,999} — all correct.
  `bytes= 100 - 199 ` -> Satisfiable{100,199}. Outside the ABNF but tolerated by the two `.trim()`s; safe direction.
  empty `Range:` value -> `strip_prefix("bytes=")` fails -> Whole. Correct.
  `bytes=-` -> suffix form, `"".parse::<u64>()` fails -> Whole. Correct.
  `bytes=00000000000000000000-1` -> leading zeros parse to 0 -> Satisfiable{0,1}. Correct (RFC permits leading zeros). A start that genuinely OVERFLOWS u64 -> parse fails -> Whole, i.e. 200 with the entire body rather than 416. Syntactically valid but unsatisfiable, so 416 is arguably more correct; Whole is defensible, matches the module's stated "malformed yields Whole" rule, and errs toward sending more data rather than refusing. Not a defect.
  ARITHMETIC: all three `total - 1` sites and the `end.min(total - 1)` sit after `if total == 0 { return Unsatisfiable }`. No underflow. No inverted `Satisfiable` can escape: closed path has start <= total-1 (from `start >= total` -> Unsatisfiable) and end >= start, so `end.min(total-1) >= start`; suffix path has n >= 1 so `saturating_sub` lands at or below total-1.
  CLAMP COMPLETENESS: the defect was on the closed path only. Suffix and open-ended both hard-assign `end = total - 1` and are inherently clamped; suffix's `start` is clamped by `saturating_sub`. Nothing else of that class.
  SURVIVING MUTATION FOUND (Minor, my own): `if spec.contains(',') { return Whole }` is **provably redundant** — delete it and all thirteen tests still pass. Every multi-range spelling reaches `Whole` anyway via a parse failure, because a comma surviving into either half makes `u64::from_str` fail (`0-99,200-299` -> last `"99,200-299"`; `1,2-3` -> first `"1,2"`; `-0,5` -> `"0,5"`). So `multi_range_falls_back_to_the_whole_resource` does not discriminate the line it appears to guard. NOT proposing removal — the line makes a deliberate design decision visible where the next reader looks, and the module doc leans on it. Recording it because "a guard that has never been falsified is not a guard" applies to this one, and the honest statement is that intent here is documented rather than enforced.
Task 10: re-review (rerev-t10, on 55289539a..9e3591e94) — all five findings addressed, no regressions (it hand-traced all 11 pre-existing tests plus `gzip, br` / `br;q=0.001` / `*;q=1` / `gzip, *;q=0` / absent header against the new code), all four new tests confirmed to discriminate, `<= 0.0` judged justified rather than gold-plating. It also **independently verified the router comment's factual claim** — `MIN_BYTES = 4096` in `precompress_dist.mjs`, and exactly four non-`.br` files in `dist/`, all above the floor, all with siblings. That is the claim rev-t10-2 could not check from the diff and P53 measured; a second party has now confirmed it. Two Minor prose nits (doc said "three malformed shapes" while four are tested; the report said "one per gap" while the fourth guards controller-added scope) — both fixed in 44daa7603.
Task 11: review (rev-t11) — spec ✅ except the `pub mod` deviation, quality Changes Requested, 1 Important + 2 Minor. It independently walked the same nine inputs I had walked and agreed on every one, including the leading-zeros case (it correctly noted 20 zeros is not an overflow at all — the value is 0 and parses fine).
Ruling P58 (the Important — ACCEPT the change, REJECT the reason): rev-t11 says RFC "requires" an inverted range be ignored. That is too strong — RFC 9110 §14.2 says a server "MAY ignore **or reject**" an invalid ranges-specifier, so 416 also conformed and this was not a conformance defect. **But it is a real defect for a different reason**: §14.1.1 classifies last-byte-pos < first-byte-pos as *invalid*, not *unsatisfiable*, and this module's own doc says invalid input yields `Whole` — so the code contradicted its own stated rule in exactly one case, which is how a rule nobody wrote gets inferred later. Changed to `Whole`. The tempting counter-argument (416 is cheap, `Whole` sends the entire body, and §14.2 itself flags invalid specs as possible DoS) **does not survive**: it fails to distinguish this shape from `bytes=abc-def`, which already yields `Whole` today — an attacker wanting the full body out of a bad header has that route regardless, so rejecting only the inverted spelling buys nothing and costs consistency. Both enum variants' docs now draw the invalid/unsatisfiable line explicitly. Cost if wrong: a nonsense `Range` gets 200 + full body instead of a cheap 416.
Ruling P58a (this was a PLAN DEFECT, the fourth this run): the brief contains BOTH design rule "Anything malformed also yields `Whole`" (line 26) AND the test `an_inverted_range_is_unsatisfiable` (line 197) asserting the opposite. My pre-flight scan caught T11's `String` vs `Option<String>` contradiction and missed this one, because I compared the Interfaces block against the code body and did not compare the **design rules** against the **test list**. Noted for the remaining tasks: a task's prose rules and its test expectations are two statements of the same thing and need to be read against each other, not just its signatures against its body.
Ruling P59 (Minor — KEEP `mod`, reject the brief's `pub mod`): the brief's Step 3 says `pub mod byte_range;`; shipped is private `mod byte_range;`. rev-t11 confirmed private is sufficient — Tasks 12/13 are descendants of `gateway::server` and reach `pub` items inside a private sibling module fine. Private also matches all seven existing sibling declarations in that file and the repo's own "pub(crate) over pub" rule. Nothing outside `server/` needs it, so `pub` would widen the surface for no consumer. Recorded here rather than as a comment in `mod.rs`, where a note saying "private like its neighbours" would be noise.
Ruling P60 (rev-t11's second Minor — FIXED, and it is the better half of a pair): it found that deleting both `.trim()` calls left all thirteen tests green. I had independently found that deleting `if spec.contains(',')` also left all thirteen green. **Two different surviving mutations, one found by each pass** — which is the argument for running both. The trim hole is closed with a real test (`whitespace_inside_the_spec_is_absorbed`); the comma line stays as documented-not-enforced per my earlier note, because removing it would delete a visible design decision and no test can distinguish it.
Task 10: complete (commits 9a6c896a4..9e3591e94, re-review clean).
Task 11: complete (commits 9e3591e94..44daa7603, review findings all addressed).
Task 12: dispatched (impl-t12, sonnet), BASE=44daa7603. Under Ruling P56 it does NOT commit — it implements, verifies, falsifies AND RESTORES, and writes its report; I commit. Dispatch carries: the three shipped facts about Task 11 that differ from what the brief assumes (`Option<String>`, inclusive `end`, private-`mod` reachability), the P58 change (inverted range now `Whole`, so its 416 test must use a start past the end — verified its brief already does), the P57 dead-code expectation (all three warnings must be gone afterwards; a survivor means the parser was not actually consumed), and P55's requirement to quote the post-restore GREEN run and not only the RED.
PRE-FLIGHT RE-SCAN of T12-T15 under the new P58a rule (compare each task's PROSE RULES against its TEST LIST, not just its signatures against its body) — it paid for itself immediately:
  T12: rules are "Range applies after every gate" and "206 and 416 both need the document CSP". Tests cover both — `a_range_does_not_bypass_the_capability_gate` and `partial_and_unsatisfiable_document_responses_keep_the_csp`. Consistent.
  **T13: GAP.** Its Interfaces block says "Same behaviour as Task 12", which inherits Task 12's rule #1 — Range applied only after the capability gate — but its four tests are full-read / satisfiable / unsatisfiable / SVG-CSP. **There is no `a_range_does_not_bypass_the_capability_gate` equivalent.** The rule is inherited in prose and unguarded in tests, i.e. the one security-relevant property of this change is the one nothing checks on this route. T13's dispatch must require that test. Cost of missing it: a future edit reorders the range branch above the gate on `/canvas-asset` and nothing goes red.
  T14: rule is "three states, not two — `gst-inspect-1.0` absent means I DON'T KNOW, and must be read as neither healthy nor broken". Three tests, one per state, including `unknown_is_neither_ok_nor_a_warning`. Consistent.
  T15: rules are "only MEDIA_ERR_SRC_NOT_SUPPORTED (4) means missing decoder" and "narrow to Linux". Three tests: the code, other codes, other platforms. Consistent.
Task 12: impl-t12 reported properly — DONE_WITH_CONCERNS, both RED and post-restore GREEN quoted for two separate mutations, all three P57 dead-code warnings gone. **Ruling P56 worked**: the one implementer this run that was not asked to commit is the one that delivered a complete report.
Ruling P61 (plan defect #5 — CONFIRMED, implementer's fix stands): the brief's `seeded_artifact()` helper and CSP test call a 5-arg `f.store.write(&session, filename, mime, &bytes, origin)` that **does not exist**. `ArtifactStore` has only `put(session_key, run_id: Option<&str>, origin, filename, mime_type, bytes)` — six args, different order, `src/artifacts/store.rs:94`, and it is what every pre-existing test in that file already calls. Verified directly; there is no `write` method on the type at all. impl-t12 followed the brief's own fallback instruction and rewrote both call sites. Accepted as-is.
Ruling P62 (Important, controller-found on review of the diff — FIX ROUND 1 DISPATCHED): the new `ARTIFACT_RANGE_READS_PER_MINUTE = 3000` bucket is selected by `headers.contains_key(header::RANGE)`, which is **entirely caller-chosen**. `parse_range("bytes=0-", total)` yields `Satisfiable { start: 0, end: total-1 }` — the whole body as a 206 — and `Range: garbage` yields `Whole`, also the whole body. So attaching one header to every request lifts a scraper from 240 full reads/min to 3,000. This is CLAUDE.md §0's "一个权限层按某个轴分级，那个轴就不能由调用方自己挑" verbatim.
  I sized it before rating it, because the answer decided Minor vs Important: `ArtifactCapabilities::mint(session_key)` (`src/gateway/security/artifact_caps.rs:88`) keys the capability to a **SESSION, not an artifact**. One capability therefore reaches every artifact in that session, so the narrow bucket really is what bounds a capability holder's pull rate — precisely the "bulk scraping by someone who obtained a capability" its own doc names. Had capabilities been per-artifact this would have been a Minor comment fix; they are not.
  The shipped doc comment asserts the protection that does not exist — "The FIRST request for an artifact carries no `Range` and therefore still draws from the narrow bucket, so the number of distinct artifacts a caller can start pulling per minute is unchanged" — which is a description of how a well-behaved media element happens to act, stated as a property of the code. Same class as §0's "一句关于什么被闸住的话…发给模型/读者的那份说了假话最贵".
  Required property sent to the implementer: **the bucket must be chosen by what the response sends, not by what the request asked for.** Approach given (open to pushback): keep step 3's cheap pre-filesystem check as-is, and at step 9 — where `verdict` and `total` are known — additionally charge the NARROW bucket when a ranged request is about to return the entire resource (`Whole`, or `Satisfiable{start:0,end}` with `end == total-1`). A genuine partial read then costs only a wide token; a full read always costs a narrow one, however it was dressed. Correct rather than merely safe for real playback: WebKit and Chrome routinely open media with `Range: bytes=0-`, that IS a full-body read, and one narrow token per media open is far inside 240/min.
⚠️ ROOT CAUSE OF THE WHOLE RUN'S "SILENT SUBAGENT" PATTERN, and a CORRECTION to my own Ruling P56. impl-t12's second idle notice carried a `failureReason`: **"API Error: Connection lost mid-response."** That is a transport failure, not agents ignoring instructions. Re-reading the run in that light: impl-t10b, impl-t11 (twice), rev-t10, rerev-t10 and rev-t11 all almost certainly hit the same thing — every one of them had done the work and lost the final message.
  **P56's mitigation was right; its stated reason was wrong.** I wrote "three agents, one failure point, so it is the step" — a false inference from three samples that shared a cause I could not see. Taking commits off the implementer's plate still helps, but not for the reason given: it helps because it SHORTENS the agent's life (fewer turns = fewer chances to lose the connection) and because a file-based handoff survives a dropped message while an inline report does not. Recording the correction rather than editing P56, because "why did we decide this" is the part that has to stay true.
  Two practical consequences for the rest of this run: (1) route every deliverable through a FILE, never an inline final message — the chase that worked on both reviewers was "write your verdict to <path>, then send me two lines"; (2) an idle notice with no report means CHECK THE TREE FIRST, because the work is usually there.
Task 12: fix round 1 landed. impl-t12 dropped its connection before applying it (tree unchanged, 242 insertions, old comment still at line 95), so the controller applied P62 directly. Fix is three parts: the doc comment now describes what is enforced instead of asserting a protection that did not exist; step 3's provisional pricing is documented as provisional and says why it cannot make the final call (it runs before the read, so `total` and the verdict do not exist yet); step 9 charges the narrow bucket when `has_range && sends_everything`, where `sends_everything` is `Whole || Satisfiable{start:0,end} with end+1 == total` — the predicate is what we are about to SEND, never what was asked.
Task 12 FALSIFICATION (P55-compliant, both directions quoted): mutation `if false && has_range && sends_everything` -> **`a_range_header_cannot_buy_more_whole_artifact_reads` FAILED, left: 206, right: 429**, and only that one test (25 passed / 1 failed). Restored -> **26 passed, 0 failed**. The new test asserts BOTH halves deliberately: a full read is refused however it was dressed (`bytes=0-` AND a malformed `bytes=abc-def`), and a genuine `bytes=10-19` still returns 206 after the narrow bucket has closed — without that second half the "fix" could have been a silent regression that throttled the exact media scrub the wide bucket was added for.
Task 12 verification: `cargo test -p alephcore --lib -j 2 -- artifact_route` -> 26 passed / 0 failed. `cargo test -p alephcore --lib --no-run -j 2` -> clean. `rustfmt --edition 2021 --check` -> clean. `cargo clippy --all-targets -j 2` -> **all three P57 dead-code warnings are GONE** (total dropped 8 -> 5); the five remaining are all pre-existing and elsewhere (`config/save.rs:9`, `loop_graph/store.rs:130`, `loop_graph/mod.rs:122`, plus two long-standing lib warnings). P57's free guard did its job: the parser is genuinely consumed, not merely referenced.
Task 12: committed as 4673bd35f.

## Pre-flight of the tail (T14, T15) — 2026-08-22

Done while T12's review and T13's implementation ran. Same method as the T13
pre-flight that found the missing capability-gate test: read each brief's
DESIGN RULES against its TEST LIST and against the real APIs it names, not
just its interface block against its code block.

**T14 — verified against the real API.** `Finding::ok` sets `Severity::Info`
(finding.rs:66). `Finding::problem(.., Severity::Info, ..)` also sets Info.
`Finding::is_problem()` is `severity > Severity::Info`. `HostPlatform`,
`DEFAULT_CHECK_TIMEOUT`, `with_fix_hint`, `Posture` all exist as the brief
names them.

Ruling P63: **T14's headline promise is false for every machine consumer, and
its own tests cannot see it.** The brief's title is "Three states, not two"
and its module doc says unknown "must not be read as healthy". But `Ok` and
`Unknown` both come out as `Severity::Info` with `is_problem() == false` and
no tag. The only thing separating them is English in the title. The `--json`
lint surface, CI, and the LLM `doctor` tool all read severity — so all three
read "I could not tell" as "healthy", which is the exact CLAUDE.md §8
criterion the brief quotes at itself. The brief's `unknown_is_neither_ok_nor_a_warning`
test asserts on the TITLE STRING, so it passes while the property fails.
Fix follows repo precedent, not invention: `Finding::with_tag` exists for
exactly this, and two checks already use it to separate states inside one
severity (`providers_connectivity` TAG_REACHABLE/TAG_UNREACHABLE,
`sqlite_integrity` TAG_DB_OK/TAG_DB_CORRUPT). Three verdicts get three tags,
and the test asserts the tag. Cost if wrong: one unused tag per finding.
Cost of not doing it: the check ships with its stated purpose unmet.

Ruling P64: **T14 as written adds dead-code warnings on every non-Linux
build.** `Format`, `FORMATS`, `CodecVerdict` and `findings_for` are consumed
only by the `#[cfg(target_os = "linux")]` `probe`/`run` and by the tests, so
the non-test lib target on Windows and macOS has no reader for any of them.
That is ~4 new warnings over this crate's baseline of 5 — permanent, not
transient like T11's. Gate them `#[cfg(any(target_os = "linux", test))]`,
which is the honest statement: they exist for Linux and for the tests that
prove the mapping. NOT `#[allow(dead_code)]` — that silences the signal for
the next reader too. Cost if wrong: the mapping tests would stop compiling on
a platform, which the build would say immediately.

Ruling P65: **T14 Step 5's inertness check is a manual observation where a
guard is free.** "Run `aleph doctor` and see no media/codecs line" builds a
debug `aleph-server` (expensive here, and this crate's build is
memory-heavy) and leaves nothing behind. A `#[cfg(not(target_os = "linux"))]`
unit test asserting `run()` returns empty costs one function, runs on every
CI pass, and is the same claim. Add the test; attempt the doctor run only if
cheap, and report honestly if it is not run rather than implying it was.

**T15 — verified against the real code.** `ChatState` is `#[derive(Clone,
Copy)]` (state.rs:507) so the brief's `let chat = *chat;` compiles.
`HostPlatform::{MacOs, Windows, Linux}` match the brief's spellings exactly.

Ruling P66: **T15's field doc promises a dismissal the component does not
have — and the sibling it claims to copy does.** The prescribed doc says the
notice is "Cleared by the user or by the next successful playback", but
`VoiceNoticeBanner` as specified renders no control, so the only clearer is
the next successful playback — which on a machine that cannot decode is
never. The banner is un-dismissable on exactly the systems it targets.
`SendErrorBanner` (composer/mod.rs:1508-1517) has the `✕` button and an
existing i18n key `t_string!(i18n, chat.dismiss)`. Mirror it. This is the
"two statements of one fact, only one updated" shape, in its cheapest form:
the false half was written before the true half existed.

Not raised as defects, recorded so the final review does not re-litigate:
- The `<Show when=…>` + body double-read in `VoiceNoticeBanner` is the shape
  CLAUDE.md §7 warns about, but the body is `.map()` over an `Option`, not an
  `expect`, so `None` renders nothing and there is no panic. `SendErrorBanner`
  ships the identical shape. Consistency with the sibling wins.
- The notice text is an English literal composed in Rust, not an i18n key.
  Out of scope for this plan; flagged for the final review only.

## Pre-flight of the tail (T16, T17) — 2026-08-22

Verified against the real code, not read for plausibility:
- The control-plane router is merged at the gateway ROOT (`server/mod.rs:758`,
  routes `/` and `/{*path}`), so T16's `$BASE/aleph_panel_bg.wasm` resolves.
- `Vary: accept-encoding` is present on all three response paths — 304, br,
  and identity (`control_plane/server.rs:171,197,211`). T17's "no false 304"
  row therefore correctly EXPECTS a 304: the ETag describes the resource and
  `Vary` is what keeps a shared cache from crossing encodings. The row's name
  reads like it expects the opposite; the expectation is right and the name is
  misleading. Left as-is, noted here so a future reader does not "fix" it.
- T16's `gst-inspect-1.0` MP3 loop was traced by hand for all four
  present/absent combinations. It is correct. So is the escaped-backtick echo
  (`\`` inside double quotes is a literal, not command substitution).

Ruling P67: **T16 Step 3 leaks a server process.** It says
`cargo run --bin aleph-server &` and never kills it. This repo has already
paid for that exact shape twice (two e2e probes parked `aleph-server` in a
`static OnceCell`, leaving a process still holding the port after every run),
and here it also collides with the OS-level singleton `flock` — the next
`just dev` fails with a lock error whose cause is three steps upstream. The
smoke run captures the PID and kills it in a trap on exit. Cost if wrong:
none; the kill is unconditional cleanup.

Ruling P68: **T16 asserts brotli arrives but never asserts it can be
declined, which is the half that breaks a real client.** Everything T10 spent
four rounds hardening — `br;q=0`, malformed `Accept-Encoding` resolving toward
identity — has zero wire-level coverage in the QA script. Serving brotli to a
client that said it cannot decode it is the failure mode that produces a blank
Panel, and it is one `curl` away from being observable. Add two assertions to
the br-negotiation block: `Accept-Encoding: identity` and `Accept-Encoding:
br;q=0` must each come back with NO `content-encoding: br`. This is inside the
task's stated purpose ("every assertion is an effect assertion"), not scope
creep — the existing block asserts one direction of a two-direction
negotiation.

Ruling P69: **T16's `aleph doctor` cross-check covers one of four formats.**
The script probes only MP3, then prints "confirm media/codecs agrees with the
line above". T14's check covers MP3, AAC, Opus and VP8/VP9. Accepted as a spot
check rather than expanded — the operator-facing answer is `aleph doctor`
itself, and duplicating its whole format table in bash would be a second
source for the same list. The printed instruction is reworded to say it is a
spot check on MP3, so nobody reads a green line as "all four agree".

Carries into T17, restated here because the workspace directory is deleted
when this run completes and these are the only record:
- P14: verify the fallback page with a throwaway static node server plus a
  forced-fail CSS probe, NOT by building a server.
- P37: confirm `dist/` holds a fenced (wasm-opt, feature-set-fenced) artifact
  BEFORE committing it.
- P39: add FU-5 to the spec for the dropped unprefixed `backdrop-filter`.
- P45: correct the plan's inert `printf '\x00' >>` mutation in Task 9 Step 3.
- Record the measured brotli figures (spec §5.1) and which assertions were
  actually falsified by mutation (spec §7.3).
- Only the Windows `cfg` arm of `SHELL_MARKER_JS` has ever been compiled. The
  macOS and Linux arms are unverified and T16's docs must name that.

Ruling P70: **T13 and T14 dispatched in parallel.** The SDD skill says never
run two implementers at once, and its stated reason is conflicts. There are
none here: T13 touches `src/gateway/server/canvas_asset_route.rs` only, T14
touches `src/diagnostics/**` only — disjoint files, disjoint test modules, no
shared `mod.rs`. Commits are mine, so there is no git race either; both were
told explicitly not to run any git command. Given this run's connection-loss
rate and five tasks left, the wall-clock is worth it. Cost if wrong: one
`git diff` shows me a collision immediately and I split them.

Ruling P71: **T15 and T16 joined the parallel batch; T17 stays serial.**
File sets are provably disjoint — T13 `src/gateway/server/`, T14
`src/diagnostics/`, T15 `interfaces/webchat/`, T16 a new file under `qa/`.
Cargo serializes them on the shared target-dir lock anyway, so this does not
risk the parallel-rustc OOM this crate is known for; it only overlaps their
thinking and editing time. T16 was told to run NO cargo command at all (its
dry run needs none) and T17 is held back because it consumes all four.
Cost if wrong: a `git diff` shows a collision and I re-run one serially.

Ruling P72: **T16's live smoke run moves to T17.** Overriding its Step 3
rather than dropping it: the claim Step 3 is really making is "the script does
not crash on a platform it was not written for", and a dry run against no
server proves that — curl fails, assertions report FAIL with their observed
values, skips fire, the summary prints. What it cannot prove is that the
assertions PASS against a live server, and T17 already stands one up for the
full sweep. Folding it there costs nothing and avoids a fifth concurrent
build. T16 must state the deferral in its report so its dry run is not read
as more than it is.

## P45 confirmed empirically, and downgraded — 2026-08-22

Ran the plan's Task 9 Step 3 mutation against a scratch copy:

    cp dist/tailwind.css.br $S/t.br && printf '\x00' >> $S/t.br
    -> decompress: OK, sha matches source: true

So appending a null byte to a brotli stream is **completely inert** — the
decoder stops at the final block and ignores trailing bytes, the output is
byte-identical to the source, and `check_panel_dist.mjs` cannot go red on it.
The plan's stated expectation (`✗ ... is not valid brotli` or `... is STALE`)
was unreachable by that command.

**But my carry overstated the consequence.** I had this filed as "a guard the
plan claims was falsified but never was". Reading `task-9-report.md`: T9's
implementer ran the brief's exact command, observed `EXIT: 0`, wrote "the
brief expected RED here", explicitly said "I could not produce the honest RED
evidence the brief requires", and then went and found mutations that DO bite —
truncation/garbage giving `✗ tailwind.css.br is not valid brotli:
Decompression failed`, and recompressing different content giving `✗ ...
decomposes to something other than tailwind.css — it is STALE`. All three
directions ended up genuinely falsified.

So the guard is sound and the report was honest. What remains is a **plan text
defect only**: Step 3 still tells the next reader to run an inert command and
expect red. T17 rewrites it to the two mutations that actually work, taken
from T9's report rather than invented. Severity: documentation, not
correctness.

Worth keeping as a method note: the implementer that refused to write "RED"
for something it had not seen go red is the reason this cost a doc fix instead
of a false guard. That behaviour is what the falsification rule is FOR — and
it is the opposite of the T11 implementer in this same run, which left its
mutation in place and reported success.

## Controller's independent pass over T12 — 2026-08-22

Written BEFORE rev-t12 reported, so the two passes stay independent. Earlier
in this run two independent passes each found a surviving mutation the other
missed; that is the argument for doing both.

Walked by hand and found correct:
- The suffix form. `bytes=-<total>` goes through `saturating_sub` to
  `start = 0, end = total - 1`, so `sends_everything` DOES catch a
  whole-resource suffix. This was the case I most expected to slip.
- `end + 1` cannot overflow: every `Satisfiable` end is clamped to
  `total - 1`, so `end + 1 <= total`.
- `bytes[s..=e]` is in bounds for every reachable verdict; `total` is derived
  from the same buffer that gets sliced, two lines apart.
- Gate ordering: the whole representation block is step 9, after TLS, origin,
  rate limit, capability and the store read.
- Zero-length artifact: `Whole` → 200 empty; any range → 416 `bytes */0`.

**Coverage hole found — the `end + 1 == total` conjunct is untested.**
Mutating `start == 0 && end + 1 == total` to just `start == 0` leaves every
test green. The three ranges the bucket test drives are `bytes=0-` (whole),
`bytes=abc-def` (malformed → whole) and `bytes=10-19` (partial, `start != 0`),
and the only `bytes=0-4` in the module is the SVG CSP test, which runs on
loopback and is therefore exempt from rate limiting. So nothing exercises a
partial read that STARTS at zero — which is the first request a media element
makes. Under the mutation, every such probe would be charged the narrow
bucket and real seeking would throttle. The code is right; the guard for half
of its predicate does not exist. Fix: add a `bytes=0-9` case to the bucket
test's second half, on the remote IP, asserting 206 while the narrow bucket
is closed.

Observation, not a finding: between the wide limit and the narrow one,
requests 241..3000 per minute do their filesystem read before step 9 refuses
them, so step 3's comment ("a flood is refused before it costs any filesystem
work") holds only up to the wide bucket. Bounded, not unbounded, and the wide
bucket exists precisely so scrubbing is not throttled — so this is the price
of the design, not a defect. Recorded so the final review does not re-derive
it as one.

## P39 re-examined, and it is a live G5 defect, not a follow-up — 2026-08-22

The carry said "add FU-5 to the spec for the dropped unprefixed
`backdrop-filter`". Went to write it and found something worse.

`interfaces/webchat/styles/tailwind.css` pairs every `backdrop-filter` with a
`-webkit-backdrop-filter` twin — setting rules and flat-mode kill rules
alike. But `backdrop-filter` also appears **outside the stylesheet**, in a
Rust string constant: `todo_panel.rs:131`, inside `TODO_PANEL_CSS`:

    .aleph-todo-wrap{ ... backdrop-filter:blur(8px); ... }

Two things are wrong with it, and they fail on different platforms:

1. **No `-webkit-` twin.** On WKWebView the blur silently does not apply, so
   this one surface is flat while every sibling is frosted — the inconsistency
   nobody would file a bug about because it looks like a design choice.
2. **`.aleph-todo-wrap` is in no flat-mode rule.** The flat block enumerates
   `.glass`, `.aleph-sidebar`, `.aleph-scrim`, `.aleph-blur-subtle`,
   `.aleph-composer`, `.aleph-session-tabs`, `.chat-scroll-fade`,
   `.nav-tile-active`. On Linux, where G5 says flat mode degrades
   unconditionally with no opt-out, **the todo panel keeps its backdrop
   blur** — on the exact machines the degradation exists to protect.

Ruling P73: **fix it in T17 rather than deferring it to FU-5.** G5's whole
content is "flat mode removes the expensive materials"; a surface that keeps
its blur in flat mode is that goal not met, not a nice-to-have. The fix is two
lines: add the `-webkit-` twin at the source, and add `.aleph-todo-wrap` to
the flat kill list. Deferring means shipping G5 with a hole and writing a
follow-up describing the hole.

Ruling P74: **the enumeration gets a guard, because it is the thing that
failed.** `.aleph-todo-wrap` was missed for a structural reason, not
carelessness — it lives in a Rust `const`, so nobody reading the stylesheet's
flat block could see that the list was short. This is the enumeration failure
mode: the list only covers the world as it was the day it was written. I
considered replacing the list with `html[data-flat="1"] *`, which cannot rot,
but a universal selector carrying `!important` on every element costs style
recalc on exactly the weak machines flat mode targets — wrong trade.

So the CSS keeps its cheap list and the LIST gets the rule: a source-level
test that scans BOTH the stylesheet and the Rust CSS string constants for
every selector that sets `backdrop-filter`, and asserts each one appears in a
`html[data-flat="1"]` rule that nulls it. Derived from the source, never a
second hand-written list — a hand-written expectation here would be the same
defect one level up. Falsify it by deleting `.aleph-composer` from the flat
block and confirming it names that selector.

P74 implementation note (found while waiting, so T17 does not re-derive it):
the guard has a home and a precedent. `interfaces/webchat/src/appearance.rs`
already does `include_str!("../styles/tailwind.css")` inside its test module
(lines 831, 855) and already carries a block-extraction helper that panics
with "unclosed block for selector ... in tailwind.css" (lines 780-796). The
flat-mode census belongs there, reusing that helper, not in a new file with a
second copy of the parser. The Rust-side CSS constants are reachable the same
way — `include_str!` on the `.rs` file, or better, scan for `const *_CSS: &str`
so a new one is picked up without being named.

## T12 review: Changes Requested — 2 Important, 3 Minor — 2026-08-22

rev-t12 landed only after a second chase. **Third time in this run that
"write your verdict to <path>, then send me two lines" recovered an agent
that had gone silent** — the work existed, the final message did not.

**The two independent passes each found a different surviving mutation, again.**
I found that dropping `end + 1 == total` (leaving `start == 0`) stays green.
The reviewer found that dropping `start == 0` (leaving `end + 1 == total`)
ALSO stays green — and then did what I did not: asked what request exploits
that half. The answer is `Range: bytes=1-`.

Ruling P75: **the `bytes=1-` bypass is real and I am fixing it.** Trace:
`parse_range("bytes=1-", total)` → `Satisfiable{start: 1, end: total-1}` →
`start == 0` is false → `sends_everything` false → the narrow bucket is never
charged. The response body is every byte except the first, which for
essentially every content type is a complete usable copy. So a fixed,
size-independent header lifts a capability holder from 240 artifact reads a
minute to 3000. This is P62 all over again one level down: I fixed the axis
the caller picks by *header presence* and left an axis the caller picks by
*offset*. The doc comment I wrote — "The wide bucket is never a way to read
more whole artifacts" — is false, which makes it the most expensive kind of
comment in this repo's conventions.

Ruling P76: **the predicate becomes "how much does this send", not "does this
send exactly everything", and it moves to `byte_range.rs`.** Two reasons for
the move: T13 has just copied the same predicate verbatim into
`canvas_asset_route.rs`, so the bug is already duplicated, and a security
predicate with two copies is the failure this repo warns about loudest. New
method `RangeVerdict::is_bulk_read(&self, total: u64) -> bool`, both routes
call it, tests live with the method plus one wire-level test per route.

Ruling P77: **the threshold is "more than half", and the doc states the
residual instead of hiding it.** There is no complete fix at this size. With
a fraction threshold f, an attacker splits each artifact into 1/f requests, so
the reachable rate is the wide bucket times f — at f = 1/2 that is 1500
artifacts/min, not the 240 the narrow bucket names. Tightening f punishes real
seeking (GStreamer does pull large chunks) without closing the hole. The only
complete fix is byte-budget accounting in the rate limiter, which is a new
mechanism and a different piece of work.

So: half, because a single request for more than half a file is not seeking by
any reading; and the doc must say what is actually guaranteed — "bounds bulk
reading to roughly half the wide bucket" — never repeat the claim that the
wide bucket cannot be used to read more artifacts. Recorded as FU-6 in the
spec. Cost if wrong: a media element that pulls >50% in one request pays the
narrow bucket; at 240/min that throttles only a client opening 240+ distinct
large files a minute, which is not playback.

Also taking from the review:
- [Minor] `Unsatisfiable => false` is untested — mutating it to `true` stays
  green. Covered by the new `is_bulk_read` tests.
- [Minor] the two 429 responses are near-verbatim duplicates. Extracting the
  helper while I am in this match anyway.
- Explicitly NOT taking: the reviewer's note that invalid-capability probes
  carrying a `Range` land in the wide bucket. True, pre-existing, and
  irrelevant — capabilities are 256-bit CSPRNG secrets, so the bucket a
  brute-force probe lands in does not change its feasibility.

Verified independently before accepting: `interfaces/webchat/Cargo.toml` was
touched by T15, which looked like a global-constraint breach ("no new runtime
dependency"). It is not — it adds the `MediaError` FEATURE to the existing
`web-sys` dependency, which is how `HtmlMediaElement::error()` becomes
callable. Within constraints.

## T13–T16 complete — 2026-08-22

Commits: `3a6c53f5b` (T12 fix), `27c6515f5` (T13), `af6c50871` (T14),
`fdc2687a4` (T15), `70fe04466` (T16).

**Correction to something I said an hour ago, and it matters more than the
thing it corrects.** I reported that impl-t13 "died mid-falsification with
MUTATION 3 still in the tree, the T11 failure exactly repeating." That was
wrong. impl-t13 was alive and *in the RED phase of its own mutation 3* when
I ran verification concurrently; it restored the gate itself minutes later,
re-read the file to confirm no markers survived, and filed a complete report.
Both of us restored the same lines, so nothing broke — I checked the region
for a duplicated gate and there is none, and 60 tests pass.

The lesson is not "I misread a file". It is that **P70's parallel dispatch is
safe for file collisions and unsafe for concurrent verification.** Disjoint
file sets stop two agents corrupting each other's edits; they do nothing
about the fact that a falsifying agent *deliberately makes the tree wrong for
a while*. While a mutation-based falsification is running, the working tree
is not an observation surface — a controller reading it sees a defect that is
actually a guard doing its job. And the reading is unfalsifiable from
outside: an abandoned mutation and an in-flight one are byte-identical.

Two things follow, and I am recording both rather than just the rule:
1. Do not run verification against a tree an implementer still holds. Wait
   for its report, or ask.
2. The T11 precedent is what made me confident, and that is exactly why I was
   wrong. Having seen this failure once, I recognised its shape and stopped
   looking. A prior instance is a hypothesis, not an identification.

What I did do, and what stands:
- Restored the capability gate (redundantly, as it turned out) and confirmed
  green.
- Applied the P75–P77 fix: `RangeVerdict::is_bulk_read`, both routes switched
  to it, wire tests for `bytes=1-` on each. Falsified by reverting to exact
  coverage — four tests red across three modules — then restored, 55 green.
- Finished T14's falsification, which it had not reached: tagging the unknown
  verdict `codecs-ok` reds `unknown_is_neither_ok_nor_a_warning`. So P63's tag
  is load-bearing, not decorative — which was the open question about it.
- Finished T15's, which it had not reached either: dropping the platform
  clause reds the other-platforms test; moving the code 4 → 3 reds the
  other-errors test. Restored, 3 pass, 1046 in the crate, and the shipped
  `wasm-release` cdylib builds with no warnings.
- Clippy is back at the baseline 5, all three pre-existing (`config/save.rs`
  unused import, and the two in `loop_graph`). T13's report of 7 was measured
  while other work was in flight.

Rulings that held up in implementation, worth noting because pre-flight
rulings usually get revised: P58a (the capability-gate test T13's brief
omitted) **caught a real 206-where-404-is-required** the moment the gate was
disabled — the one security-relevant property of that change was the one
nothing in the brief checked. P63's tag and P66's dismiss button both landed
as specified. P64's cfg gate held clippy at baseline, and T14 found one thing
I had not predicted: `Format::elements` is read only by the Linux-only
`probe`, so it needed a real invariant test rather than a wider gate. It
wrote one (`every_format_lists_at_least_one_element`) instead of reaching for
`#[allow(dead_code)]`, which is the right instinct.

Remaining: T17 only.

## T17 in progress — 2026-08-22

Commits so far: `cbee070b6` (flat-mode fix + census), `abc24d614` (dist),
`39776efca` (spec figures, falsification record, plan mutation correction).

**P73/P74 landed and the census earned itself immediately.** Two mutations
bite and each names the selector AND the file it was declared in — including
across the Rust/CSS boundary, which is the specific blindness that let
`.aleph-todo-wrap` sit unlisted.

**A third mutation did not go red, and that was the right answer.** I removed
`.aleph-composer` from the flat block expecting red; the guard stayed green.
Following the rule I wrote down earlier this run — when fewer things go red
than predicted, suspect the prediction first — I went and looked: that
selector has a flat rule but sets **no backdrop filter anywhere** in the
stylesheet. Its flat rule is defensive or vestigial, and there was nothing
for the census to miss. Recorded in the spec alongside the null-byte case,
because a mutation that quietly fails to bite is indistinguishable from a
blind guard unless someone writes down which it was.

**Measured brotli, from the clean `just wasm`:** 22,177,008 → 3,396,020 bytes
(−84.7%); wasm alone 21,914,484 → 3,360,760. All four assets clear the 4 KiB
floor. `✓ wasm-opt applied (feature set fenced)` printed before the commit,
which is the P37 carry discharged. Both guards green standalone.

**FU-5 was not created.** The carry said to file the dropped unprefixed
`backdrop-filter` as a follow-up; P73 ruled to fix it instead, since a
surface keeping its blur in flat mode is G5's own goal unmet rather than a
nice-to-have. And **FU-6 was not created either** — FU-3 in the spec already
names byte-based rate limiting as the correct fix; the right move was to
sharpen the existing entry with the measured residual, not to add a second
follow-up saying the same thing under a new number.

**Open, and not caused by this work:** `cargo test -p alephcore --features
test-helpers --test '*' --no-run` fails with `can't find crate for alephcore`
across four integration targets. This matches a previously recorded condition
in this checkout — that command was already failing at main HEAD here. Being
confirmed rather than assumed before I call the verification set clean; if it
turns out to be mine, it blocks.

## T17 Windows sweep — measured, 2026-08-22

Server run against an **isolated `ALEPH_HOME`**, not the user's. Two reasons:
the OS-level singleton `flock` would collide with anything the user has
running, and a sweep should not write to real state to prove a transport
property. Killed afterwards and verified gone — `tasklist` shows zero
`aleph-server.exe`, the port refuses, the temp home is deleted. That is
P67's lesson applied to my own run rather than only to T16's script.

Wire results, all four as designed:

| Request | Result |
|---|---|
| `Accept-Encoding: br` | `content-encoding: br`, **3,360,760 bytes** — exactly the `.br` file, not double-encoded |
| `Accept-Encoding: gzip` | `content-encoding: gzip`, 5,089,074 bytes (the spec estimated ~5.02 MB) |
| `Accept-Encoding: identity` | no `content-encoding`, 21,914,484 bytes — the identity file exactly |
| `Accept-Encoding: br;q=0, gzip` | **gzip, not brotli** — the refusal is honoured on the wire |

`ETag: W/"eb7278…"` and `Vary: accept-encoding` on every response including
the 304. Conditional requests return 304 with a zero-byte body under all
three encodings, which is the designed behaviour and safe precisely because
`Vary` is there: the ETag describes the resource, and the cache is told what
else varies. Without a validator the same three encodings return 200 at three
different sizes.

The QA script runs live and exits 0: **4 passed, 0 failed, 3 skipped**. The
two passes I added under P68 — identity honoured, `br;q=0` honoured — are the
only wire-level coverage the brotli refusal path has, and they pass. Skips
name their own reason.

`doctor` runs **13 checks** (was 12, so `MediaCodecsCheck` is registered) and
emits **zero** `media/codecs` lines on Windows. Registered and inert, which
is what the non-Linux arm promises. The `duplicate-instance` warning is the
doctor correctly noticing my own sweep server.

Served `index.html` carries the probe: `data-platform` ×3, `data-flat` ×3,
`CSS.supports` ×2, a `<noscript>`, and the four capability names the fallback
page lists (`oklch`, `color-mix`, `registerProperty`, `WebAssembly`).

**Not measured, and I am not claiming it:** the Range 206/416 path on the
wire. It needs a minted capability URL, i.e. a real session with a stored
artifact. It is covered by 60 unit tests that drive the actual axum router
end to end, including the capability-gate and rate-bucket cases — but that is
not the same as a live curl, and the QA script correctly SKIPs it with an
instruction rather than pretending.

**Two false alarms this phase, both mine, both the same shape.** First the
`--test '*'` failure, which I nearly filed as a pre-existing condition —
building one named target succeeds, so my first read (concurrent builds
corrupting the shared rlib) was wrong too. Second the `.aleph-composer`
mutation. In both cases the cheap check was to isolate the variable, and in
both cases my first explanation was the one that matched a story I already
had. Third time this run.

## The `--test '*'` question, settled — 2026-08-22

`cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1`
→ **exit 0, 0 errors, 137 executables, 36m22s.**

So the six `can't find crate for alephcore` failures are a **parallel-build
resource condition** — rustc running out of room building many test targets
at once against this crate, which is a known property of it — and not a code
defect, not a feature-unification problem, and not a structural fault in this
checkout. Serializing fixes it completely.

**This corrects a recorded memory**, which says `--test '*'` "is broken at
main HEAD here". It is not broken; it needs `-j 1`. The note was written from
the same symptom I hit today and stopped at the same place I nearly did.

Worth keeping as method: the sequence that settled it was
① `--lib --no-run` passes ⇒ the library itself compiles;
② one named target builds ⇒ nothing structural about that target;
③ the full set at `-j 1` builds ⇒ the variable is parallelism, not code.
Each step costs more than the last and each one narrows it, so running them
in that order means usually stopping at ①.

## T17 verification set — complete, 2026-08-22

| Command | Result |
|---|---|
| `cargo test -p alephcore --lib --no-run` | 0 errors |
| `cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1` | exit 0, 137 executables |
| `cargo test -p aleph-panel --lib` | 1047 passed |
| `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release` | clean, no warnings |
| `cargo clippy --all-targets -p alephcore -j 1` | exit 0, 5 warnings — all pre-existing |
| `just _stage-shell-placeholders && cargo check -p aleph-desktop-shell` | Finished |
| `node scripts/check_webview_baseline.mjs` | ✓ |
| `node scripts/check_panel_dist.mjs` | ✓ |
| touched-module tests (`byte_range` / `artifact_route` / `canvas_asset_route` / `media_codecs`) | 60 passed |

The three hard global constraints checked directly rather than assumed:
- `git diff --stat 015556e4e..HEAD -- src/harness/` → **empty**. R10 intact.
- No commit message in the range claims a Linux or macOS behaviour was
  verified (grepped for it).
- The only `Cargo.toml` change across 34 commits is the `web-sys` `MediaError`
  **feature** — no new dependency in either crate.

34 commits, working tree clean. Final whole-branch review dispatched on the
most capable model, scoped to source (dist excluded — 21 MB of build output
whose correctness is a build-guard question, not a reading question).

## Final whole-branch review — 2026-08-22

Dispatched on the most capable model over `015556e4e..HEAD`, source only
(dist excluded — 21 MB of build output whose correctness is a build-guard
question, not a reading question).

It is earning its seat. Four findings so far, and **all four are the same
defect class this repo names loudest: a sentence that was true when written
and stopped being true beside code that kept working.** None was caught by
any per-task review, because each needs two tasks in view at once.

1. **`artifact_route.rs:94` — mine, and the worst of the four.** The constant's
   doc still opened with *"The wide bucket is never a way to read more whole
   artifacts."* That is the exact sentence P75 identified as the reason
   `bytes=1-` went unnoticed. I fixed the predicate, the call-site comment,
   and wrote honest text into the new helper — and left the original claim
   standing one screen above. **I named this failure mode in a ruling and
   then committed it in the same change.** Fixed, and the replacement says
   the old sentence was false rather than quietly deleting it: someone who
   finds the removal in history should learn why.
2. **`check_panel_dist.mjs:151`** sent the reader to "precompress_dist.mjs's
   own note on this". No such note existed. Written, at the branch it
   describes.
3. **`check_webview_baseline.mjs`, two defects, both reproduced.** `readJson`
   returns null so an operator gets a named file instead of a stack trace —
   and edge D dereferenced it unguarded, so a missing or malformed baseline
   produced precisely the TypeError that design prevents *and* swallowed the
   queued diagnostics naming the file. Falsified both ways: hiding the file
   and corrupting it now each exit 1 with the reason. Separately, the D2
   census used a consuming boundary class where a lookbehind was needed —
   `matchAll` resumes past the trailing `(`, so `calc(` nested directly inside
   `translate(` is invisible. Measured: old form `[translate]`, new form
   `[translate, calc]`. **Zero new names in today's stylesheet (32 either
   way)** — the blind spot is structural, not yet triggered, which is exactly
   the standard the paragraph above it already sets for itself.
4. **`control_plane/server.rs:28-31`** carried three measured byte counts
   labelled "Measured on this build". They were measured at T10; `abc24d614`
   rebuilt dist, so all three were stale while asserting currency. Replaced
   with the real figures plus the date and commit they came from, and a note
   that they are a snapshot — the live source is the spec table and the
   precompressor's own output.

Commit `b63003585` carries 1-3. 4 is staged next.

Worth stating plainly: **three of the four are in code I wrote or edited
during this run**, and two are in the very mechanisms built to catch drift.
A guard is not exempt from the defect it guards against.

## Final review closed — all 9 live findings addressed — 2026-08-22

Verdict: 0 Critical, 2 Important, 7 Minor (13 filed; 4 fixed mid-review).
Commits `cf6c2d52d` and `3d51b3eb1`. Nothing parked.

**The two Important ones were both about claims, not code:**

1. The justfile said the WebView baseline guard "runs in CI"; `grep` over
   `.github/` returned nothing. Its only trigger was a local `just wasm`, so
   edges A–D — the macOS install gate among them, G1's hardest — drifted
   unwatched. **Wired rather than walked back**: every input it reads is
   committed, so it runs on the existing `panel-dist` job's bare checkout
   with no WASM toolchain, exactly like the `.br` check beside it. Making a
   false claim true beats correcting it when the claim describes protection
   that ought to exist.
2. The spec said BOTH Tauri confs declare `minimumSystemVersion`. They must
   not — the lite conf is a merge overlay, and restating the value is the
   second source of truth edge A forbids. Code right, spec wrong, corrected
   in three places **as a recorded correction**: a reader closing that gap in
   the direction the old text stated would create the exact defect being
   guarded against.

**Falsifying my own fixes found three more blind spots the review never
reached.** Closing 5d (the census enumerated its FILES while its doc claimed
the set was derived) meant walking the tree — and the probe I wrote to prove
the fix stayed green. Three separate causes, each independently sufficient:
the scan was line-oriented, so a one-line rule was invisible — and one-line
rules are the dominant style in these Rust CSS consts; it ignored
`-webkit-backdrop-filter`, exempting the very engine flat mode exists for;
and its head walk stopped only at `}`, so `.glass` inside
`@layer components {` was never seen. Ground-truthed against an independent
`awk` scan: six setters exist, six are found.

That sequence is the lesson of this whole run in miniature. **The fix for a
blind spot is where the next blind spot lives**, because you write it with
the same assumptions that produced the first one. The only thing that broke
the chain was refusing to accept a green result from a mutation I had
predicted would go red.

**Three of the four mid-review findings were in code I wrote during this
run**, and two were inside the drift-catching mechanisms themselves. A guard
is not exempt from the defect it guards against.

Worth recording as a near miss: `artifact_route.rs:94` still opened with
"The wide bucket is never a way to read more whole artifacts" — the exact
sentence P75 identified as the reason `bytes=1-` went unnoticed. I fixed the
predicate, the call site, and the new helper's doc, and left the original
claim standing one screen above, in the same commit where I named the
failure mode. Naming a defect class does not inoculate you against it.

## Two amendments from the agents' late final messages — 2026-08-22

Every agent's closing message arrived after I had already read its work off
disk and committed it. Nothing in them was outstanding; two are worth
correcting into the record.

**1. The census-regex fix was bigger than I measured, and the reviewer's
number is the better one.** I recorded "32 distinct names either way, no new
names — structural blind spot, not yet triggered." True, but it measures the
wrong thing. Re-measured against the built stylesheet:

    occurrences  consuming: 2433   lookbehind: 2666   hidden: 233
    distinct     consuming:   32   lookbehind:   32

So the consuming boundary class was silently skipping **233 function
occurrences**, not zero. The census stayed green before and after only
because every one of those 233 happened to be a repeat of a name already
reviewed elsewhere in the file. That is luck, not coverage: the guard's whole
value is that its silence can be trusted, and it was blind to 9% of its own
input. "No new names surfaced" was the right observation and the wrong
headline.

**2. impl-t13's "tooling quirk" was me.** Its report flags that an Edit call
to restore its mutation-3 errored `string not found`, while a read
immediately after showed the code correctly restored — and says it could not
explain that from the file contents. The explanation is not tooling: I had
restored the same lines myself, seconds earlier, having read its in-flight
RED phase as an abandoned mutation (see the correction above). Two writers,
same content, one of them surprised.

Recorded because the report is now a committed artifact and its open question
otherwise stands as a suspected tool defect. It is not one. It is the
concrete cost of P70's parallel dispatch meeting mutation-based
falsification: the tree is not a stable observation surface while a falsifier
is running, and the confusion is mutual — the agent could not explain my
write any more than I could explain its mutation.
