# Whole-branch final review — `015556e4e..60886e0b8 (35 commits)`
# Tauri Cross-Platform WebView Resource Control (G1–G5, 17 tasks, 33 commits)

Reviewer: final whole-branch reader. Read-only; no code modified.
Status: COMPLETE. Verdict: Changes Requested — 0 Critical, 2 Important, 7 Minor live (4 more filed and fixed mid-review, verified against 60886e0b8).

## Hard constraints — verified first

| Constraint | Verdict | Evidence |
|---|---|---|
| `src/harness/` zero delta | PASS | `git diff --stat 015556e4e..HEAD -- src/harness/` → empty |

(remaining sections appended as verified)

| No new runtime dependency | PASS | `precompress_dist.mjs` uses `node:zlib` only; `byte_range.rs` is hand-written, no `tower-http` `fs` import anywhere in the diff |
| `dist/` committed + paired-guarded | PASS | `node scripts/check_panel_dist.mjs` → green, both `.br` directions run; 4 sources / 4 siblings, sizes match spec §5.1 exactly |

---

## 1. Cross-task seams

### 1a. `byte_range` ↔ both routes — FITS

`parse_range` / `RangeVerdict::is_bulk_read` / `content_range` are consumed identically
by `artifact_route.rs:379-443` and `canvas_asset_route.rs:284-352`. Verified line by
line: same status mapping, same `has_range && is_bulk_read` narrow-bucket second charge,
same `Accept-Ranges` on every built response, same CSP re-application on 206/416, same
`bytes[s..=e]` slice guarded by the parser's `start <= end < total` invariant. Neither
route grew a private copy. The only asymmetries are correct ones: artifact keys CSP on
`is_active_document(mime)` (html/xhtml/svg), canvas keys it on `image/svg+xml` alone
because `served_content_type` already downgrades `text/html` to `text/plain`.

Range is applied strictly last in both — after insecure-remote refusal, origin policy,
rate limit, capability→session/canvas resolution, store read, and (artifact) the
record-side capability re-validation. A range cannot reach a byte an unranged request
could not: the slice is taken from a buffer the gates already authorised.

**One doc/code divergence found in this seam — see FINDINGS (artifact_route.rs:94).**

### 1b. `MIN_BYTES` ↔ precompressor/guard — FITS, with a dangling pointer

`check_panel_dist.mjs:23` imports `MIN_BYTES` from `precompress_dist.mjs:24`; single
source, never retyped. The import-inertness problem is real and is handled properly:
`precompress_dist.mjs:39` gates the whole body on an entry-point check, and
`check_panel_dist.mjs:102-117` asserts that gate **at source level** — correctly, since
a runtime check cannot distinguish "the import was inert" from "there was nothing to
compress". The `pathToFileURL` form (not a raw `argv[1]` compare) is required on Windows
and is what is used.

Divergence: the producer skips a file whose brotli output is not smaller
(`precompress_dist.mjs:90-95`) and deletes any sibling; direction 2 of the guard
(`check_panel_dist.mjs:142-155`) demands a sibling for every source over `MIN_BYTES`.
Today dist/ holds four assets and all four compress, so the two agree. The day a `.png`
or `.woff2` over 4 KiB lands in dist/, the guard fires. That failure direction is safe
(loud, named, handed to a human) and the guard's own message says so — but it points the
reader at "precompress_dist.mjs's own note on this", and **that note does not exist**.
See FINDINGS.

### 1c. `webview-baseline.json` ↔ its four consumers — FITS, two defects in the guard itself

Ran `node scripts/check_webview_baseline.mjs` → green. Edges A/B/C/D all present and
each is genuinely wired:

- **A** reads `bundle.macOS.minimumSystemVersion` from the base conf and only checks the
  lite overlay for a *contradiction* — correct, since `tauri.lite.conf.json` is a deep
  merge overlay and duplicating the value there would be the second source of truth.
- **B** is set-equal in both directions (lines 101-106), so neither an added nor a
  removed probe drifts. It additionally pins the two version numbers that the fallback
  page retypes as user-visible copy (`macOS 13.3+` / `WebKitGTK 2.42+`, lines 107-117) —
  I went looking for that as an unguarded second copy and it is guarded.
- **C** asserts byte-identity of the probe inside `dist/index.html` *and* that the probe
  precedes `<script type="module">`. The ordering half is load-bearing and is checked.
- **D1** (reverse) is honest and anchored with a negative lookbehind so `lch(` cannot be
  satisfied by `oklch(`.

Two defects, both in `check_webview_baseline.mjs` — see FINDINGS:
1. Edge D dereferences `baseline` with no null guard (`:198`), so a malformed or missing
   `webview-baseline.json` produces precisely the raw stack trace `readJson`'s own
   comment (`:22-27`) says it exists to prevent, **and** swallows the named edge-A/B
   diagnostics that were queued in `problems`. Reproduced.
2. The D2 census regex consumes its boundary character, so a function nested immediately
   after another function's `(` is structurally invisible. Reproduced. This contradicts
   the census's own stated invariant, and the file already contains the correct spelling
   100 lines above.

### 1d. Working tree diverges from the reviewed range — read this before acting on findings

`git status --porcelain` during this review:

```
 M scripts/precompress_dist.mjs
 M src/gateway/server/artifact_route.rs
```

Both files changed **on disk while this review was running**, and neither change is in
`HEAD` (`39776efca`). The two edits are exactly the two doc defects recorded below as
F-1 and F-2: `precompress_dist.mjs` gained the "NOTE — this branch and
check_panel_dist.mjs's direction 2 disagree by construction" note that the guard's message
pointed at, and `artifact_route.rs` lost the false "the wide bucket is never a way to read
more whole artifacts" headline and gained a "What this bucket does NOT promise" section.

Both findings were real in the reviewed range and both are addressed by these uncommitted
edits. **They are not committed**, so the branch as it stands still carries them. Commit
them or the fixes are lost.

**Update, later in the same review:** `scripts/check_webview_baseline.mjs` also changed on
disk mid-review, fixing both defects I had just reproduced (an early exit after the
baseline read, and the census regex switched to `(?<![\w-])`). All three findings below
are therefore **present in the reviewed range and fixed only in the uncommitted tree**.
The remainder of this review is written against the *working tree*, since that is what
will be committed.

### 1e. `HostPlatform` ↔ probe / shell / Panel — FITS

Three writers, one resolved value, and the ordering hazard the spec §3.2 identified is
handled correctly:

- `baseline-probe.js:30-41` resolves (keeping a shell-set value if present, else a
  three-bucket UA fallback resolving ambiguity to `linux`) and **writes** the attribute.
- `desktop/shell/src/main.rs:103-114` declares all three `cfg` arms, including the
  Windows arm that "uses it for neither today" — declared so that absent ≠ Windows.
  Idempotent with the probe by construction.
- `platform_host.rs` is a pure reader with **no** UA fallback, and its unknown/absent
  case resolves to `Linux` — the same safe direction the probe chose. Verified there is
  no second UA parser anywhere in `interfaces/webchat/src/`.
- Step 2 (`data-flat`) uses the local `platform` variable, **not** a re-read of the
  attribute (`baseline-probe.js:54`) — exactly the trap the spec named.

`host()`'s only consumer is `voice_playback.rs`. Its non-wasm arm returns `Linux`, which
means host-toolchain tests of any future consumer silently take the Linux branch; today
that is harmless because the only decision function (`is_missing_decoder`) takes the
platform as an explicit parameter and its tests pass all three values.

---

## 2. Dead / duplicated / contradictory across tasks

One stale duplicate. `src/gateway/control_plane/server.rs:28-31` says
"**Measured on this build:** wasm 21,882,715 B identity → 3,363,082 B via `.br` →
5,089,368 B via this layer's runtime gzip". Those figures were measured at task 10
(`progress.md:181,186`) against the then-current dist. Commit `abc24d614` rebuilt dist,
and the committed bytes are now 21,914,484 / 3,360,760 — which is what the spec's own
table (§5.1, commit `39776efca`) records. Two copies of one measurement; the code copy is
stale and is the one asserting "on this build". See FINDINGS.

Nothing else: no dead helper, no orphaned constant, no contradictory rule found. The
`ARTIFACT_RANGE_READS_PER_MINUTE` / `CANVAS_RANGE_READS_PER_MINUTE` pair is a deliberate
non-share with the reasoning written at the constant, and the shared thing that *matters*
(`is_bulk_read`) is genuinely shared.

---

## 3. Security-relevant surfaces

**Can a range reach a byte an unranged request could not? — No.**
In both routes the `Range` header is read only after: plaintext-remote refusal → origin
policy → rate limit → capability→scope resolution → store read → (artifact only) the
record-side capability re-validation and filename match. The slice is taken from a buffer
those gates already authorised (`artifact_route.rs:434-443`, `canvas_asset_route.rs:343-352`),
and `parse_range` guarantees `start <= end < total`, so the `bytes[s..=e]` index is not
attacker-driven past the end (`bytes=900-99999` clamps; there is a test).
`a_range_does_not_bypass_the_capability_gate` exists in both routes and forges the cap.

**Can the wider bucket be reached by something that is not really a seek? — Yes, and it
is bounded, adjudicated, and now honestly documented.**
`Range: bytes=0-9` against 3000 distinct resources per minute draws only wide tokens.
`is_bulk_read`'s "more than half" threshold closes the one-header bypass (`bytes=0-`,
`bytes=1-`, and a malformed `Range` all charge the narrow bucket — all three are asserted
in `a_range_header_cannot_buy_more_whole_artifact_reads`). The residual — split each
resource into two sub-half requests and you read it entirely on wide tokens — is what
FU-3 records, and is stated at `RangeVerdict::is_bulk_read`'s doc in the exact "do not
restate this as…" form. It was *also* contradicted by a headline 300 lines away in
`artifact_route.rs`; that headline is F-2 and is fixed in the working tree.

**Does any refusal distinguish itself from "not found"? — No new leak.**
416 is only reachable after the full capability chain, so it reveals nothing a 200 does
not. `not_found()` is the single shared refusal for wrong-cap / missing / expired /
filename-mismatch / traversal in both routes, and the 416 path never runs before it.
Note that `Accept-Ranges`, `Content-Range` and the CSP are emitted only on responses
built by the shared builder (200/206/416) — the 403/404/426/429 refusals carry none of
them, so the shapes stay uniform per class.

**Rate-limit keying** is `(identity, scope)` and the two scopes are distinct map entries
(`rate_limiter.rs:60-76`), loopback is exempt before any accounting, so the desktop App
is unaffected as claimed. `RpcRealtime` is genuinely otherwise unused in these two
private limiters — verified by grep, no other `RateLimitScope::RpcRealtime` use inside
either route.

**Brotli negotiation** is refusal-safe in every ambiguous direction: unparsable header,
`q=0` in any spelling/case, `q<=0`, a refusal anywhere in the list, and `*` never
consulted — all resolve toward identity, which every client can read. Seven unit tests
cover the qvalue matrix and the ETag is identity-derived with `Vary` (asserted).

---

## 4. Comments and docs that state something the code does not do

Four, in descending cost.

**4a. `justfile:382-383` claims CI runs the baseline guard. It does not.**
The `check-baseline` recipe's doc says it is "Run by `just wasm`, and in CI on any change
under `interfaces/webchat/` or `desktop/shell/`." `grep -rn "check_webview_baseline"
.github/workflows/` returns **nothing**. `aleph-core-ci.yml` runs only
`check_panel_dist.mjs`, and its `paths:` filter names `interfaces/webchat/dist/**` and
`scripts/check_panel_dist.mjs` — not `webview-baseline.json`, not `baseline-probe.js`,
not `scripts/check_webview_baseline.mjs`. So the guard's only trigger is a local
`just wasm`, which requires binaryen and a WASM toolchain that, by this repo's own
account (`check_panel_dist.mjs:37-44`), no CI job owns. This is the branch's own D4
argument — "a gate that silently skips when a tool is missing is a gate that exists only
on the author's machine" — applied to the guard written to satisfy it.
The dist/`.br` pairing half *is* wired (the `panel-dist` job runs `check_panel_dist.mjs`
and `interfaces/webchat/dist/**` is in the paths filter), so this gap is specific to the
baseline guard.

**4b. The spec says both Tauri confs get the two keys. Only the base conf does — and that
is the correct implementation.**
`tauri.lite.conf.json` is a deep-merge overlay, so duplicating `minimumSystemVersion` or
`webviewInstallMode` there would create the second source of truth that
`check_webview_baseline.mjs` edge A is written to forbid ("the overlay must omit it or
match"). The implementation got this right. The spec did not get updated, and says the
opposite in four places: 3.3 ("the two `tauri.conf.json` files' `minimumSystemVersion`"),
4.1 line 244 ("Both ... gain"), 6.1 line 461 ("both conf files declare"), 9 line 619
("same two"). The spec is the binding authority; a later reader closing that gap in the
direction the spec states would break the single-source property.

**4c. `control_plane/server.rs:28-31` — "Measured on this build" is measured on a previous
build.** (Detail in section 2.)

**4d. `voice_playback.rs:70,128` tell the user to "run `aleph doctor` for the exact
package" — `doctor` answers for the CORE machine, not the machine rendering the Panel.**
`MediaCodecsCheck::run` is `Vec::new()` off Linux, and on Linux it probes the core's own
GStreamer registry. The predicate that produces this message keys on `host()` — the
*client's* platform, resolved from `data-platform`/UA, which is exactly right for deciding
whether to show it. But a Linux browser Panel against a Windows or macOS core gets a
remedy that returns no codec finding at all; against a Linux core it gets a finding about
the wrong machine's decoders. In the co-located desktop App — the only topology where a
Tauri WebKitGTK WebView is involved — the advice is correct. This is the same
client-vs-server asymmetry the spec's own D3 rejects for platform adjudication,
reintroduced in the remedy text.

Verified and found accurate (not defects): the `is_bulk_read` doc's "what this bounds and
what it does not"; `precompress_dist.mjs`'s entry-point-guard rationale; the
`check_panel_dist.mjs` both-directions rationale; the justfile's two-pass wasm-opt
explanation including its explicit "this is NOT a biconditional" caveat; the QA script
header's statement that only the Windows arm of `SHELL_MARKER_JS` has ever been compiled;
`media_codecs.rs`'s "20s ceiling" (`DEFAULT_CHECK_TIMEOUT` is 20s) and its three-state /
tag-not-severity reasoning (`MediaCodecsCheck` is registered at
`src/diagnostics/mod.rs:82`, so the check is not a severed wire); `VoiceNoticeBanner` is
mounted (`composer/mod.rs:1206`) and uses the `Option`-view shape rather than a
`Show`-guard plus `expect`, avoiding the known Leptos race.

---

## 5. Guards that cannot fail

Two surviving mutations, plus a bounded blind spot.

**5a. `Accept-Ranges` on 206 and 416 is untested in both routes.**
`artifact_route.rs:416` / `canvas_asset_route.rs:324` set the header on the shared
builder, and the comment at both sites says "Advertised on EVERY response, including the
refusals". `grep -n ACCEPT_RANGES` over both files returns exactly two occurrences each:
the production line and one assertion — and that assertion is in
`a_full_read_advertises_range_support`, which issues a plain `request()` and gets a 200.
**Mutation that leaves every test green:** move the `.header(header::ACCEPT_RANGES, ...)`
call out of the shared builder and into the `RangeVerdict::Whole` arm only
(`artifact_route.rs:416`, `canvas_asset_route.rs:324`). No test reads the header on a 206
or a 416, so nothing goes red while the stated invariant is broken.

**5b. `Vary: accept-encoding` is asserted on one of its three emission sites.**
`control_plane/server.rs` emits it on the 304 arm (`:171`), the brotli 200 arm (`:197`)
and the identity 200 arm (`:211`). Only the brotli arm is asserted
(`brotli_is_served_when_the_client_accepts_it`). **Mutations that leave every test green:**
delete `(header::VARY, "accept-encoding")` from `:171`, or from `:211`. Both are in the
safe direction (identity is always readable, and the dangerous variant — brotli stored
without `Vary` — is the one that *is* covered), which is why this is Minor.

**5c. `CompressionLayer` pass-through has no automated coverage at all.**
The whole D2 decision rests on "tower-http passes through a response that already carries
`Content-Encoding`, so the 22 MB is not double-encoded". Every test in that module calls
`serve_static_or_index(...)` **directly**; `test_create_router` only checks that the router
compiles. `identity_is_served_when_brotli_is_not_accepted`'s own message says "let
CompressionLayer decide" and then never lets it. This is not a false claim — the wire
behaviour *was* measured on a live Windows server and the evidence is in
`task-10-report.md:26-71` and `progress.md:181-188` — but it is a regression a tower-http
bump could reintroduce with the suite green. One `oneshot` against
`create_control_plane_router()` would close it.

**5d. Bounded blind spot (not a surviving mutation today).**
`appearance.rs::no_backdrop_filter_survives_flat_mode` derives its setter set from exactly
two sources: `tailwind.css` and `todo_panel.rs`, hardcoded at `:903-904`. Its doc says the
set is "DERIVED from both places a blur can be declared, never hand-listed" — the
*selectors* are derived, but the *file list* is the enumeration. **Mutation:** add
`backdrop-filter:blur(8px)` to any `.rs` other than `todo_panel.rs` and the census stays
green. Today `grep -rn backdrop-filter interfaces/webchat/src/` returns exactly one
non-test hit (`todo_panel.rs:131`), so the list is complete as of now. Related: the setter
scan matches `backdrop-filter:` but not `-webkit-backdrop-filter:`, so a rule that sets
only the prefixed property is invisible; every current rule declares both.

**Checked and found genuinely falsifiable:** every `byte_range` mutation I could construct
(`>` to `>=` on the half threshold, `>=` to `>` on `start >= total`, dropping the
`end.min` clamp, `q <= 0.0` to `q < 0.0`, early-return on the first accepting `br` token)
reds a named test. The two dist-guard directions, the `is_bulk_read` narrow-bucket second
charge, the CSP-on-206/416, the capability-gate precedence, and the codec tag all have
real assertions. `nulled.len() >= 5` and `!setters.is_empty()` are present as
self-protection against the flat census going vacuous.

**One surviving mutation in the safe direction, noted not filed:** dropping `has_range &&`
from the second bucket check (`artifact_route.rs:398`, `canvas_asset_route.rs:303`) leaves
every test green and silently halves the plain-read budget from 240 to 120. Stricter, not
looser, so it is not a hole — but nothing observes it.

---

## 6. Honesty of the record

Spot-checked five of section 7.3's claims — three "went red", both "did not bite".

| Claim | Verdict |
|---|---|
| `is_bulk_read` / revert to exact coverage / "red x4, across three modules" | **Accurate, exactly.** `bytes=1-` reds `a_range_missing_only_the_first_byte_is_still_a_bulk_read`; `bytes=499-` reds `whole_and_full_coverage_are_bulk_reads`; plus one route test each in `artifact_route` and `canvas_asset_route`. Four tests, three modules. |
| capability gate precedes Range / disable the gate's early returns / "red, 206 where 404 required" | **Accurate in shape.** `a_range_does_not_bypass_the_capability_gate` forges the cap segment and sends `bytes=0-9`, asserting 404; with the cap gate's early returns removed the request reaches the builder and answers 206. Present in both routes. |
| codec verdict tags / tag the unknown verdict `codecs-ok` / "red" | **Accurate.** `unknown_is_neither_ok_nor_a_warning` asserts `has_tag(TAG_CODECS_UNKNOWN)`; retagging the `Unknown` arm reds it. Severity alone would not — both are `Info`, exactly as the doc says. |
| did-not-bite: appending `\x00` to `tailwind.css.br` is inert | **Verified empirically.** Appended a NUL to a copy of the real `tailwind.css.br`: `brotliDecompressSync` succeeds and the output is byte-identical to `tailwind.css`. No guard could have fired. |
| did-not-bite: `.aleph-composer` has a flat rule but sets no backdrop filter | **Verified.** `.aleph-composer` appears at `tailwind.css:1164` (the flat null) and `:1619/:1633/:1638/:1647`; none of the latter declares a backdrop filter. Nothing for the census to miss. |

Commit messages: scanned all 33. **No commit claims a Linux or macOS behaviour was
verified.** `70fe04466` states the constraint explicitly and its script header goes
further than required, naming that only the Windows arm of `SHELL_MARKER_JS` has ever been
*compiled*. `8acdf2e92`'s "Verified on the wire against a live server" is Windows-local and
the evidence is in `task-10-report.md`. Spec 7.3's framing ("the list is exhaustive for
guards this work added — anything not named here was not falsified") correctly excludes the
untested items in section 5 above.

The one place the record is not honest is the *spec*, not the commits: 4b above.

**Independent verification run for this review:**
`git diff --stat 015556e4e..HEAD -- src/harness/` -> empty ·
`node scripts/check_panel_dist.mjs` -> green ·
`node scripts/check_webview_baseline.mjs` -> green ·
`cargo test -p alephcore --lib --no-run` -> exit 0 (one pre-existing `unused import: warn`
in `src/config/save.rs`, last touched by `4ad48261b`, outside this range) ·
`cargo test -p alephcore --lib -- byte_range artifact_route canvas_asset_route
media_codecs control_plane` -> **77 passed, 0 failed**.
Not run: `cargo clippy --all-targets`, `cargo test -p aleph-panel --lib`,
`cargo build -p aleph-panel --target wasm32-unknown-unknown` (the only command that
compiles the Panel's shipped form). The Panel crate gained a `web-sys` feature and new code
in four files, so that last one is worth running before merge if it has not been —
`fdc2687a4`'s message says the wasm-release cdylib builds clean.

---

## Addressed during this review — verified fixed, no action needed

Four findings I filed were fixed and committed while the review was still running, in
`b63003585` and `60886e0b8`. I re-verified each against the new `HEAD` rather than
trusting the diff. Line numbers below are the pre-fix ones.

| Was | Fix | How I verified it |
|---|---|---|
| [Important] `artifact_route.rs:94` — the false headline "The wide bucket is never a way to read more whole artifacts" | `b63003585` | The assertive headline is gone. What remains at `:121` is a *historical* note inside a new "What this bucket does NOT promise" section (`:114`) recording that the sentence was false and that it is why `bytes=1-` went unnoticed. That is the right disposal — the claim is retired without erasing why it mattered. |
| [Important] `check_webview_baseline.mjs:198` — edge D dereferenced `baseline` with no null guard | `b63003585` | Re-ran my repro (a trailing-comma `webview-baseline.json` in an isolated tree) against the new file: it now prints `cannot parse ... as JSON: Expected double-quoted property name...` and exits 1. No stack trace, and the queued edge-A diagnostic survives. The early exit at `:53-56` carries the reasoning. |
| [Important] `check_webview_baseline.mjs:276` — the D2 census regex consumed its boundary character | `b63003585` | Now `(?<![\w-])(-?[a-zA-Z][\w-]*)\(` at `:297` — the same spelling `isLoadBearing` already used. Guard re-run against the real `tailwind.css`: exit 0, no newly-surfaced names (the fix takes 233 hidden *occurrences* to 0 while the distinct-name set stays at 32, which is why it was green before and is green now). |
| [Minor] `control_plane/server.rs:29` — "Measured on this build" was measured on a previous build | `60886e0b8` | Now reads "Measured 2026-08-22 on the dist committed in `abc24d614`: wasm 21,914,484 B -> 3,360,760 B -> 5,089,074 B", matching the committed dist byte-for-byte, and the gzip figure was re-measured (5,089,368 -> 5,089,074) rather than carried over. It also adds an explicit staleness warning naming the failure it just came from. |

The fifth thing I raised in prose but never filed — `check_panel_dist.mjs:151` pointing at
"precompress_dist.mjs's own note on this" when no such note existed — is also closed:
`precompress_dist.mjs:90-105` now carries it, and says what to do when the two disagree
rather than only that they can.

Both guards re-run clean at the new `HEAD` (`check_panel_dist` exit 0,
`check_webview_baseline` exit 0), and the branch's 77 tests still pass.

---

## FINDINGS

Live only. Line numbers re-derived against `60886e0b8` — several shifted when the two fix
commits grew the doc comments above them.

- [Important] `justfile:382-383` — the `check-baseline` recipe's doc claims the WebView baseline guard runs "in CI on any change under `interfaces/webchat/` or `desktop/shell/`", but `grep -rn check_webview_baseline .github/workflows/` returns nothing; its only trigger is a local `just wasm`, which needs a WASM toolchain no CI job owns — so edges A-D, including the macOS install gate that is G1's hardest gate, drift with nothing to catch them. (The `.br` pairing half *is* wired: the `panel-dist` job runs `check_panel_dist.mjs` and `interfaces/webchat/dist/**` is in the paths filter.)
- [Important] `docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md:244,461,619` (and section 3.3) — the spec says both Tauri confs gain `minimumSystemVersion` and `webviewInstallMode`, while the shipped code correctly puts them only in the base conf because the lite conf is a merge overlay; a later reader closing the spec/code gap in the direction the spec states would create the second source of truth `check_webview_baseline.mjs` edge A exists to forbid.
- [Minor] `src/gateway/server/artifact_route.rs:427` and `src/gateway/server/canvas_asset_route.rs:324` — both comments claim `Accept-Ranges` is on "EVERY response, including the refusals", but each file's only assertion (`artifact_route.rs:576`, `canvas_asset_route.rs:617`) is inside `a_full_read_advertises_range_support`, which reads it off a 200; SURVIVING MUTATION: move the `.header(header::ACCEPT_RANGES, ...)` call out of the shared builder into the `RangeVerdict::Whole` arm and every test stays green while the stated invariant is broken.
- [Minor] `src/gateway/control_plane/server.rs:178,218` — `Vary: accept-encoding` is emitted on three arms (304 at `:178`, brotli-200 at `:204`, identity-200 at `:218`) and asserted on one (`:294`, the brotli arm); SURVIVING MUTATIONS: delete it from `:178` or from `:218` and every test stays green. Safe direction only — the dangerous variant, brotli stored without `Vary`, is the covered one — which is why this is Minor.
- [Minor] `src/gateway/control_plane/server.rs:240` — `test_create_router` only checks that the router compiles, and every other test in the module calls `serve_static_or_index(...)` directly, so the `CompressionLayer` pass-through the whole D2 decision rests on has zero automated coverage; it was genuinely measured on the wire (`task-10-report.md:26-71`), but a tower-http bump would reintroduce double-encoding of the 22 MB wasm with the suite green. One `oneshot` against `create_control_plane_router()` closes it.
- [Minor] `interfaces/webchat/src/appearance.rs:903-904` — `no_backdrop_filter_survives_flat_mode` hardcodes its two source files while its doc says the required set is "DERIVED from both places a blur can be declared, never hand-listed"; the selectors are derived but the *file list* is the enumeration, so a `backdrop-filter` added to any `.rs` other than `todo_panel.rs` is invisible. Complete as of today (one non-test hit repo-wide). Related: the setter scan matches `backdrop-filter:` but not `-webkit-backdrop-filter:`, so a rule setting only the prefixed property would also be invisible.
- [Minor] `interfaces/webchat/src/platform/wide/views/chat/voice_playback.rs:66` — the `canPlayType` pre-check leg re-implements the "and only on Linux" clause inline instead of calling `is_missing_decoder`, so the two legs carry two copies of the platform predicate and only leg 2 has tests; leg 1 has none, which spec section 7.3 records honestly rather than papering over.
- [Minor] `interfaces/webchat/src/platform/wide/views/chat/voice_playback.rs:70,128` — both user-visible strings say "run `aleph doctor` for the exact package", but `MediaCodecsCheck` answers for the CORE machine and returns `Vec::new()` off Linux, so a Linux browser Panel against a non-Linux core is pointed at a diagnostic that reports nothing, and against a Linux core it reports the wrong machine's decoders. Correct in the co-located desktop App — the only topology with a Tauri WebKitGTK WebView — but it is the same client-vs-server asymmetry spec D3 rejects for platform adjudication.
- [Minor] `src/gateway/server/byte_range.rs:24-28,115-117` — `RangeVerdict::Whole`'s doc says every invalid spec lands there, but the `total == 0` early return precedes the parse, so a malformed `Range` against a zero-length resource answers `Unsatisfiable`/416 instead of `Whole`/200; conformant either way under RFC 9110 section 14.2, but the doc's enumeration is not.

**Counts: 0 Critical, 2 Important, 7 Minor** (13 filed; 4 fixed and verified during the review).
