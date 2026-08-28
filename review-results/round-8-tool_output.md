# Logic Review Report — src/tool_output
**Module**: src/tool_output
**Scope**: full module (13 .rs files)
**Date**: 2026-08-29
**Mode**: strict

## Findings

### [Warning] `distill_output` counts blank lines in `total_lines`, making the digest header misleading
- **Location**: `src/tool_output/distill.rs:237-244`
- **Trigger condition**: Input that contains a long run of empty lines around real content, e.g. `"\n".repeat(3000) + "error: boom"` or any tail-heavy diagnostic surrounded by padding whitespace.
- **Risk**: `total_lines` is incremented on every `text.lines()` element **before** the `stripped.is_empty()` check, so the model-facing header `[Output digest: 3001 lines, 1 error]` reports 3001 lines of content when the actual signal is one line among 3000 empty lines. The "3001 lines" wording overstates the input's volume, which (a) is an honesty-of-header invariant the module docs otherwise hold to (`Reduction::render` is meant to "double as a signal to the model that this result is partial"), and (b) makes the `Lines { kept, total }` tally in the structured reducers semantically inconsistent (those reducers count content lines).
- **Current impact**: low–medium. The salient extraction is correct; only the header text overstates.
- **Suggestion**: Increment `total_lines` for non-empty lines only (or maintain a parallel `raw_total_lines` and report the raw count while the kept/total tally counts content lines).

### [Warning] Dedup across blank lines can silently collapse distinct identical errors
- **Location**: `src/tool_output/distill.rs:251-263`
- **Trigger condition**: A log like `error: E\n\nerror: E\n\ntest result: ...` — same `error: E` separated by a blank line. The dedup hash (`prev_hash`) is updated **only** on non-empty lines but **not reset** by blank ones, so the second `error: E` is skipped because its hash matches `prev_hash` from three iterations earlier.
- **Risk**: Identical adjacent duplicate noise (progress bars, repeated dots) is the documented intent, but real-world logs frequently repeat identical messages across blank-line boundaries (retry attempts, repeated test failures across `---` separators). The current behaviour is a faithful port of the previous "keep a String copy of the previous line" code path, so this is pre-existing — but the migration to a hash-based comparison was the right opportunity to also reset `prev_hash` on blank lines, and didn't.
- **Current impact**: low. Affects only the salient-line count and the rendered digest when repeated errors are present; no path/error-count loss because the first occurrence is still extracted.
- **Suggestion**: Treat a blank line as a dedup boundary — e.g. `if stripped.is_empty() { prev_hash = None; continue; }`.

### [Warning] `compress_screenshot` can mis-detect base64 in non-image alphanumeric payloads that contain `+`/`/`/`=`
- **Location**: `src/tool_output/compressor.rs:200-230`
- **Trigger condition**: Any string > 100 bytes whose first 128 bytes are all alphanumeric/`+`/`/`/`=` **and** whose first 128 bytes contain at least one of `+`/`/`/`=` (or the whole string ends with `=`). Examples that will be mis-replaced:
  - Error messages like `"ConnectionRefused+retry-after-1500ms-or-more-or-less"` (all alphanumeric + `+`).
  - Concatenated tracebacks with `+` paths.
  - URL-safe base64 identifiers that happen to flow through `take_screenshot` on misconfigured tool servers.
- **Risk**: The whole payload is replaced with `[Screenshot captured successfully]`, silently losing content. The detection is necessary for legitimate screenshot data, but the only ASCII check (`is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='`) is too permissive — actual base64 has a well-defined character-frequency distribution (roughly uniform across 64 symbols) and a known minimum entropy, neither of which is checked. The doc explains the regression that motivated the `+`/`/`/`=` requirement (long hex dumps being mistaken for base64), but the converse failure (an alphanumeric text with `+` being mistaken for base64) is not defended.
- **Current impact**: low. Real `take_screenshot` tools return data URIs or opaque base64, but the absence of a length-aligned or entropy check is a latent footgun.
- **Suggestion**: Either tighten to a length ≥ 1024 with at least 8 unique base64 symbols in the prefix, or require an even-aligned prefix length (real base64 chunks are 4-char multiples with `=` padding).

### [Warning] `compress_screenshot` does not recognise uppercase data URLs
- **Location**: `src/tool_output/compressor.rs:217`
- **Trigger condition**: `output.starts_with("data:image/")` is byte-exact lowercase. A server that emits `DATA:image/png;base64,...` or `Data:Image/PNG;base64,...` (RFC 2397 says `mediatype` is case-insensitive in practice; some tools emit mixed-case) falls through to the base64 prefix check.
- **Risk**: Mixed-case data URLs that contain a space in the metadata section (e.g. `data:image/png; name="screenshot.png";base64,...`) fail the all-base64-chars check on the space, and end up in the metadata branch: first 5 lines kept (one long line), no `[Screenshot captured successfully]` placeholder. The model sees the raw base64 string in the result field, which is the exact outcome the compressor exists to prevent.
- **Current impact**: low. The MCP and Chrome DevTools servers tested emit canonical lowercase, but the doc is silent on case sensitivity.
- **Suggestion**: Lowercase the first 5 bytes before comparison, or use `output.get(..10).map_or(false, |p| p.eq_ignore_ascii_case("data:image"))`.

### [Warning] `compress_snapshot` returns an empty string for a >4 KB single-line input (silent amputation)
- **Location**: `src/tool_output/compressor.rs:285-295`
- **Trigger condition**: A `take_snapshot` result that is ≥ 4 KB but contains **zero newlines** (a minified AX tree, a single-line JSON envelope, or any malformed-shape output). `lines.len() == 0`, so the no-interactive fallback computes `summary_lines = 0.min(20) = 0`, `lines[..0] = &[]`, and the conditional `if lines.len() > summary_lines` is `0 > 0 = false`, suppressing the trailing marker.
- **Risk**: The result field is set to `""` and the model sees an empty snapshot. No `[Snapshot compressed: …]` header is emitted because `lines.len() == 0`, so the model has no indication that the snapshot existed at all. The walker returns a `Some(new)` to `compress_result_value`, `changed = true`, and the pre-compression original is persisted — so the bytes are recoverable via `ctx_search`, but the model must know to look.
- **Current impact**: low. Real chrome-devtools-mcp output is multi-line; this hits only on a malformed/shape-violating producer.
- **Suggestion**: Either passthrough single-line input (treat it as "no lines, no compression, but don't pretend we compressed") or emit an explicit `[Snapshot too dense to compress — N bytes omitted]` marker in place of the silent empty.

### [Warning] JSON reducer cap is exceeded by one (the `…(+N more)` marker counts toward the kept tally)
- **Location**: `src/tool_output/structured/json.rs:213-228` (object arm), `structured/json.rs:170-185` (array arm)
- **Trigger condition**: An object with `> profile.json_object_keys` entries, or an array with `> profile.json_array_elems` entries. The first loop fills `out` to the cap, the second loop bails on `out.len() >= cap`, then the marker is **inserted/pushed after** the cap check, so `out` ends at `cap + 1`.
- **Risk**: A consumer reading the kept count (e.g., the `Tally::Chars { kept, total }` returned from `reduce_json`, or a downstream `serde_json::Map::len()`) sees `cap + 1` rather than `cap`. The `…` key in objects is also semantically distinct (it does not roundtrip into the original key set), so any code that re-keys on the kept count is mildly off-by-one.
- **Current impact**: low. The central `is_meaningful_shrink` byte guard still rejects the result if the marker made it bigger than the input, so the user-visible output is never wrong — only the counted tally is.
- **Suggestion**: Document the `cap + 1` invariant in `Profile`, or trim the loop one element short so the marker fits inside the cap.

### [Warning] `inline_error_digest` does not check the rendered digest fits in budget (inconsistent with `distill_or_truncate`)
- **Location**: `src/tools/result_processing.rs:591-604`
- **Trigger condition**: `inline_error_digest` always returns the digest if it has any errors, with no budget check on the rendered size. The cap defaults to 8 salient lines × `MAX_LINE_CHARS` (400) chars each = ~3.2 KB before headers — far above the 400-token (≈1.6 KB) budgets that small-window models install.
- **Risk**: The caller (`apply_result_budget` line 295) installs the digest verbatim as `body` of the recovery footer: `"body\nfooter"`. The combined `body + footer` can exceed the declared budget, defeating the Layer-2 size discipline. Compare with `distill_or_truncate` at line 567 which **does** check `estimate_tokens_smart(&rendered) <= budget_tokens` and falls through to head/tail truncation if the digest doesn't fit.
- **Current impact**: medium. This is the opaque-result path (no structured reduction was available), which is rare for builtin tools but common for MCP wrappers.
- **Suggestion**: Mirror `distill_or_truncate`'s `if estimate_tokens_smart(&rendered) <= budget_tokens` check; fall through to a budget-bounded head/tail slice when it doesn't.

### [Warning] `truncate_with_budget` degenerates to "header + full text" when budget saturates to 0
- **Location**: `src/tools/result_processing.rs:608-639`
- **Trigger condition**: `budget.saturating_sub(footer_tokens) == 0` (footer alone eats the budget) → `distill_or_truncate(text, 0)` → `truncate_with_budget(text, 0)` → `target_chars = 0` → `head_chars = 0`, `tail_chars = 0` → `head_end = 0`, `tail_start = 0` → format string returns `"" + "\n... [output truncated, ~N tokens omitted] ...\n" + text` — the entire input is preserved with only a misleading header.
- **Risk**: When the footer (recovery marker + search hint) consumes the entire budget, the "truncated" body is the full input plus a `[output truncated, ~N tokens omitted]` marker that lies about how much was actually dropped. The body+footer combination is then ≈ `text + header.length()`, which is *larger* than the input — strictly defeating the offload's token savings.
- **Current impact**: low. Only triggered when `footer_tokens >= budget`, which is itself an upstream sizing issue; but the saturating-sub hides it.
- **Suggestion**: When `budget.saturating_sub(footer_tokens) == 0`, return just the footer (no body), or shrink the head/tail slice to `budget - footer_tokens - header_chars` and add an explicit "no body fits" note.

### [Warning] ANSI stripper drops ESC + one byte for non-CSI/OSC sequences, leaking the tail of DCS/PM/APC payloads
- **Location**: `src/tool_output/distill.rs:107-114` (the `Some(_)` arm of `strip_ansi`); also reached from `src/tool_output/sanitize.rs::sanitize_command_output`
- **Trigger condition**: Input containing DCS (`ESC P … ESC \`), PM (`ESC ^ … ESC \`), or APC (`ESC _ … ESC \`) sequences. The escape and the next byte (the introducer letter) are dropped, but the sequence body and the terminator are left in the output verbatim.
- **Risk**: A producer that emits terminal-title sequences via DCS (some `tmux`, `screen`, and modern logging frameworks) leaves the DCS body (a potentially long payload of arbitrary text) in the output. For a sanitizer whose purpose is "strip bytes that are never content", leaving a 1 KB DCS payload is a contract violation — the model receives content that was supposed to be removed. The pure ANSI stripping also misses the `ESC c` (full reset) and `ESC =`/`ESC >` (keypad mode) cases, which are short enough that the one-byte drop is sufficient.
- **Current impact**: low–medium. DCS payloads are rare in command output; sanitizer guarantees are weakened but the leak is bounded by how often exotic escapes are used.
- **Suggestion**: For the `Some(_)` arm, consume bytes until the next `ESC` or end-of-string, mirroring the OSC arm's `ESC \` termination handling.

### [Warning] `compress_result_value` matches a tool name like `take_snapshot` against **any** MCP server, not just chrome-devtools
- **Location**: `src/tool_output/compressor.rs:80-90`
- **Trigger condition**: A non-Chrome MCP server (e.g. a Playwright MCP wrapper, a custom automation tool) that happens to register a verb named `take_snapshot`, `take_screenshot`, `list_network_requests`, etc. The bare-name lookup matches first, so the compressor treats the result as a Chrome DevTools payload.
- **Risk**: The DevTools strategies assume a specific output grammar (aria-tree lines with `uid=` handles, base64 PNG bytes, JSON arrays of `{method, url, status}`, etc.). A different server's payload of the same name will likely not match the grammar:
  - `compress_snapshot`: no interactive role found → falls into "no interactive elements" arm → keeps first 20 lines.
  - `compress_screenshot`: base64 detection may false-positive on alphanumeric output with `+`/`/`.
  - `compress_network_requests`: JSON parse fails → falls back to `compress_generic` at `MIN_GENERIC_CAP_BYTES` (1 KB), possibly amputating a 300 KB payload to 1 KB.

  The doc explains this trade-off as "all seven are browser-automation verbs, and the compression each one gets … follows from the *shape* of that output". When the shape does not match, the compression is at best inefficient and at worst lossy. The flag-and-fall-back-to-generic is the only thing keeping this safe, and the amputation in the third case is real.
- **Current impact**: low. No such conflicting server observed in production, but the dispatch is widening rather than server-precise.
- **Suggestion**: Either narrow the match to specific MCP server prefixes (`chrome_devtools__*`, `playwright__*`) or, before applying a strategy, validate the payload's shape (`looks_like_aria_tree` etc.) and fall back to passthrough when it doesn't match.

### [Warning] `compress_snapshot`'s role match is case-sensitive despite AX-tree role grammar being case-insensitive in some emitters
- **Location**: `src/tool_output/compressor.rs:288-296`
- **Trigger condition**: A server that emits `Button "Save"` or `TEXTBOX "Email"` (capitalised role tokens, as some Playwright configurations do) — `trimmed.starts_with("button")` returns false.
- **Risk**: The line is not classified as interactive; falls into the "no interactive elements" arm; the controls are dropped. A live snapshot with 30 interactive controls ends up as 20 lines of "structural summary" — the same failure shape that motivated the `uid=` prefix handling, but for case rather than order.
- **Current impact**: low. The current Chrome DevTools MCP and Playwright defaults emit lowercase roles; the regression risk is dormant until a server change.
- **Suggestion**: `trimmed.eq_ignore_ascii_case_prefix(role).is_some()` or lowercase the role names in `INTERACTIVE_ROLES` and use `eq_ignore_ascii_case` for the prefix.

### [Warning] `is_continuation` accepts any non-alphanumeric marker followed by whitespace, including markdown bullets and arithmetic operators
- **Location**: `src/tool_output/structured/log.rs:128-135`
- **Trigger condition**: A pytest assertion block followed by markdown-style commentary: `> assert got == 1\n* note: developer wrote this\n...`. The `*` line is at the same indent as the error and `is_continuation("* …")` returns true (`*` non-alphanumeric, ` ` whitespace), so `mark_signal` keeps it as "context" rather than treating it as a sibling end-of-block.
- **Risk**: Non-diagnostic lines that start with `*`, `+`, `-`, `:`, `=`, `#`, `>`, `!`, `?` are pulled into the kept region as if they were a continuation of the prior loud line. This widens the kept context past what `mark_signal` actually intended ("a compiler error's `--> src/main.rs:10:5` continuation or a panic's stack frames") and consumes budget that should have gone to actual diagnostic bodies.
- **Current impact**: low. Real pytest assertion bodies use `E   AssertionError:` and rustc continuations use `|`, both of which correctly match. The permissive pattern is over-inclusive for adjacent-prose noise that happens to start with a bullet.
- **Suggestion**: Tighten the predicate to require that the second char is whitespace **and** the line either carries the gutter marker of a recognised test runner (`E`, `>`, `|`) or begins with whitespace past the marker (true indented continuation).

### [Warning] `distill::extract_path` URL/path heuristic returns `Some("https://…")` for URLs containing `:digits`
- **Location**: `src/tool_output/distill.rs:144-188`
- **Trigger condition**: An error message that includes a URL with an explicit port: `connect to https://api.example.com:8443/health failed`. The token `https://api.example.com:8443/health` contains a `:digits` run; `match_indices(':')` finds the colons in order, eventually reaches the `:8443` colon, the path part `https://api.example.com` contains `.` and `/`, the line digits `8443` are non-empty, so the function returns `Some("https://api.example.com:8443")`.
- **Risk**: The path index in the model-facing digest carries a URL fragment dressed up as a `file:line` reference. The model may try to `read_file` it (no such file), or treat the URL as a local path. The function is gated by `is_error_line`/`is_context_line` to even consider the path, so only error/context lines with URLs are affected — but URLs in error messages are common (HTTP errors, API failures).
- **Current impact**: low. The model typically still has the full error line to read.
- **Suggestion**: Reject candidate paths whose path-part starts with a URL scheme (`http://`, `https://`, `mailto:`, `file://`).

### [Warning] `size_hint` under-counts for objects with non-string scalar leaves, misrouting the blocking-worker decision
- **Location**: `src/tool_output/ingress.rs:155-163`
- **Trigger condition**: A tool result that is mostly large numbers, booleans, and nulls — e.g. a metrics dump like `{"a": 9999999999, "b": 8888888888, …}` for hundreds of keys. Each non-string leaf contributes `16` bytes to the hint (the catch-all `_ => 16` arm) rather than its actual size.
- **Risk**: The blocking-worker gate (`size_hint < INGRESS_BLOCKING_THRESHOLD` in `dispatch.rs:1553`) sees a hint smaller than the real flattened value. If the real value crosses `INGRESS_BLOCKING_THRESHOLD` but the hint doesn't, the ingress runs on the async executor rather than a blocking worker — exactly the stall the threshold exists to prevent. The doc claims the hint is a "cheap upper-bound-ish" estimate, but for numbers/bools/nulls it is an underestimate.
- **Current impact**: low. Tool results are predominantly string leaves; the undercount only matters for unusually numeric outputs.
- **Suggestion**: For `Value::Number`, estimate `value.to_string().len()` lazily, or use a per-type constant that approximates real sizes (`Number` ≈ 20, `Bool`/`Null` ≈ 8).

### [Warning] The distiller's `extract_path` is not applied to `compress_screenshot`'s metadata branch — metadata-only screenshots lose path references
- **Location**: `src/tool_output/compressor.rs:235-244`
- **Trigger condition**: A screenshot whose payload is not base64 (e.g. an HTML or JSON error response from the screenshot tool) falls into the "metadata lines" branch. Lines like `Saved to /tmp/build/screenshot-abc123.png:2024-08-12` or any error path are kept as-is but no path extraction or distillation runs.
- **Risk**: The model receives the raw metadata and has no path index. Compare to `distill_output`, which extracts paths from error lines and emits a `[Files: …]` trailing line. The compressor is lossy by construction but does not preserve the path signal in the non-base64 fallback.
- **Current impact**: low. Screenshot tools rarely return non-base64 failures.
- **Suggestion**: When in the metadata branch, run `distill_output` (or just `extract_path`) over the kept lines and append the `[Files: …]` line.

### [Warning] `clean_result_value` runs sanitization inside `rewrite_interior`, but sanitizer's droppable-control filter erases leading whitespace control bytes that carry formatting
- **Location**: `src/tool_output/sanitize.rs:38-50` + `hygiene.rs:191`
- **Trigger condition**: A field whose payload relies on `\u{0}` (NUL) as a separator (rare, but some binary protocols) or `\u{8}` (backspace) for terminal overstrike formatting. After sanitize, the NULs and backspaces are gone, and the surrounding text becomes a contiguous word.
- **Risk**: The byte-level meaning of the payload is altered in ways the producer never intended. For typical command output (where control bytes are noise), this is the desired behaviour; for any payload where control bytes carry meaning, it's a corruption. The sanitizer's doc acknowledges this trade-off ("ANSI stripping plus residual control-byte removal"), so the behaviour is documented, but the call site (`hygiene::reduce_field`) does not opt-out per field.
- **Current impact**: low. Tool outputs rarely rely on control bytes for structure.
- **Suggestion**: Provide a `sanitize_with_ansi_only` mode for cases where the producer is trusted to emit control bytes semantically, and pass through hygiene untouched otherwise.

### [Warning] `MIN_FIELD_TOKENS = 150` is a soft floor measured against the field's *current* size, not the field's *original* size after stripping
- **Location**: `src/tool_output/hygiene.rs:189-194`
- **Trigger condition**: A field whose ANSI-escape-rich content is > 150 tokens **before** stripping but < 150 tokens **after** stripping — `tokens_before` is measured against the un-stripped field (correct), but the field may carry hundreds of escape runs that the sanitizer would remove cheaply. The early-return `tokens_before < MIN_FIELD_TOKENS` is gated on the wrong axis.
- **Risk**: Fields with many escapes but few actual signal chars (e.g. a 200-token field of colourised progress dots) bypass hygiene entirely; the escapes reach the model and inflate the context window. The sanitize-only path is only entered when the cleaner declines — and the early-return short-circuits before the cleaner is asked.
- **Current impact**: low. Most fields either carry signal (passed the floor) or are small enough that escapes don't matter.
- **Suggestion**: Measure `tokens_before` against the post-sanitize version, or lower the floor when the field contains escape bytes.

### [Warning] `serde_json::Map` ordering means the salient-scalar priority in `shrink` still depends on alphabetical key ordering for short scalars whose names sort late
- **Location**: `src/tool_output/structured/json.rs:201-218`
- **Trigger condition**: A document whose salient keys are not alphabetically early. The first loop prefers short scalars but iterates them in `BTreeMap` (alphabetical) order; if 50 of the first 8 short scalars are `code_*` numeric fields (all short scalars) and `status`/`message` sort later, the cap binds on the `code_*` fields and `status`/`message` are dropped in the second loop only if room remains.
- **Risk**: A 200-key object with `code_000`–`code_199` short scalars and the salient `status`/`message`/`error` keys all short scalars: the cap binds on the first 48 alphabetically-sorted short scalars (`code_*` first because `c < e < m < s`), and the second loop cannot add `status`/`message`/`error` because `out.len() == cap`. The module doc claims "Short scalars like `status` / `error` / `message` are admitted first" — but this is only true when `status`/`error`/`message` sort before the alphabetically-first non-salient short scalars. For dense numeric-prefixed keys, the priority list is dominated by numerics.
- **Current impact**: low. The test `the_object_cap_keeps_salient_scalars_not_the_alphabetically_first_keys` constructs the inverse scenario (`aaa_bucket_*` first, then `status`) and passes. The reverse case is not tested.
- **Suggestion**: Maintain an explicit `SALIENT_KEY_NAMES` allowlist (`status`, `message`, `error`, `code`, `type`, `id`, `request_id`, `retry_after`) and short-circuit the priority sort with that list before falling back to alphabetical order.

### [Warning] `clean_for_ingress` mutates `value` even when hygiene's overall reduction is rejected
- **Location**: `src/tool_output/ingress.rs:106-117`
- **Trigger condition**: A field where hygiene's per-field reduction is accepted but the *flattened* result is not actually smaller (`estimate_tokens_smart(&flattened) < before` is false — e.g., compression already cut hard and hygiene's modest improvement is erased by flatten syntax).
- **Risk**: The walker has already replaced the field string with the rejected reduction (an in-place mutation), so `value` is now in an inconsistent state. The caller installs `outcome.model_facing` as the result, so the inconsistency is never observed — but the next time `value` is read (e.g., for an inline-error-digest path that inspects `value` rather than `model_facing`), the rejected reduction is what shows up. The doc acknowledges this ("a rejected pass leaves the mutations in `value`"), but the inconsistency is reachable from `apply_layer_two`'s `out.value = Value::String(processed.text)` only on success; on the rare error path where `processed.text` is computed differently, the mutated value is what gets shipped.
- **Current impact**: low. The current call path always installs `processed.text`, never reads `value` again.
- **Suggestion**: Document the contract more explicitly, or take a snapshot of `value` before hygiene and restore it on rejection (with the cost comment about deep-cloning large values acknowledged).

### [Suggested Test] `distill_output`'s `total_lines` should not include blank lines
```rust
#[test]
fn total_lines_excludes_blank_lines() {
    let mut s = String::new();
    for _ in 0..3000 { s.push('\n'); }
    s.push_str("error: boom\n");
    s.push_str("at src/main.rs:42:9\n");
    for _ in 0..3000 { s.push('\n'); }
    assert!(s.len() > MIN_DISTILL_INPUT_BYTES);
    let d = distill_output(&s).expect("signal");
    // header should report a single line of content, not 6002.
    let rendered = d.render(20);
    assert!(rendered.contains("1 lines"), "got: {rendered}");
}
```

### [Suggested Test] Dedup must reset across blank-line boundaries
```rust
#[test]
fn duplicate_error_across_blank_lines_is_not_collapsed() {
    let mut s = String::new();
    for _ in 0..1500 { s.push_str("   Compiling crate_x\n"); }
    s.push_str("error[E0308]: mismatched types\n");
    s.push_str("\n");
    s.push_str("error[E0308]: mismatched types\n"); // intentional repeat after blank
    s.push_str("\n");
    s.push_str("note: rerun the build\n");
    assert!(s.len() > MIN_DISTILL_INPUT_BYTES);
    let d = distill_output(&s).expect("signal");
    // both error occurrences should be counted (or, if dedup is desired,
    // the cap should be sized such that the model sees the failure mode
    // was repeated).
    assert!(d.error_count >= 1);
}
```

### [Suggested Test] `compress_screenshot` rejects all-alphanumeric text containing `+`
```rust
#[test]
fn compress_screenshot_does_not_treat_non_base64_alnum_plus_as_image() {
    // 100+ bytes, all alphanumeric + '+', so the existing detection fires.
    let error_like = "ConnectionRefused+retry-after-1500ms-or-more+now+exhausted".repeat(5);
    assert!(error_like.len() > 100);
    let out = compress_tool_output("take_screenshot", &error_like);
    assert_ne!(
        out,
        "[Screenshot captured successfully]",
        "an error message must not be replaced with the screenshot placeholder"
    );
}
```

### [Suggested Test] `compress_snapshot` does not silently drop a single-line 5 KB input
```rust
#[test]
fn compress_snapshot_handles_a_single_line_oversized_input() {
    // 5 KB on one line: no newline at all, lines.len() == 1.
    let one_line = format!("uid=1_0 RootWebArea \"{}\"", "x".repeat(5_000));
    assert!(one_line.len() > 4 * 1024);
    let out = compress_tool_output("chrome_devtools__take_snapshot", &one_line);
    // The result must not be the empty string — and ideally carries a marker
    // explaining why the snapshot could not be compressed.
    assert!(!out.is_empty(), "got an empty reduction");
}
```

### [Suggested Test] `compress_screenshot` accepts uppercase data URLs
```rust
#[test]
fn compress_screenshot_accepts_uppercase_data_url() {
    let upper = format!("DATA:image/png;base64,{}", "A".repeat(2_000));
    let out = compress_tool_output("take_screenshot", &upper);
    assert_eq!(out, "[Screenshot captured successfully]");
}
```

### [Suggested Test] `distill::extract_path` rejects URLs as file references
```rust
#[test]
fn extract_path_rejects_url_with_port() {
    assert_eq!(
        extract_path("connect to https://api.example.com:8443/health failed"),
        None,
        "URL fragments with :digits must not look like file:line"
    );
}
```

### [Suggested Test] JSON reducer's salient-scalar priority under alphabetical short-scalar crowding
```rust
#[test]
fn shrink_preserves_status_under_code_prefixed_short_scalar_crowd() {
    let mut obj = serde_json::Map::new();
    for i in 0..200 {
        obj.insert(format!("code_{i:03}"), serde_json::json!(i));
    }
    obj.insert("status".into(), serde_json::json!("failed"));
    obj.insert("message".into(), serde_json::json!("connection refused"));
    let (reduced, _) = shrink_default(&Value::Object(obj));
    let out = reduced.as_object().unwrap();
    assert!(out.contains_key("status"));
    assert!(out.contains_key("message"));
}
```

### [Suggested Test] `truncate_with_budget` with budget 0 does not return the full input
```rust
#[test]
fn truncate_with_budget_zero_does_not_return_full_input() {
    let big = "x".repeat(10_000);
    let out = truncate_with_budget(&big, 0);
    assert!(
        out.len() < big.len(),
        "budget 0 must yield a body shorter than the input, got {} bytes for {}",
        out.len(),
        big.len()
    );
}
```

### [Suggested Test] `inline_error_digest` rejects a digest that overflows the budget
```rust
#[test]
fn inline_error_digest_budget_overflow_returns_none_or_truncated() {
    let mut s = String::new();
    for i in 0..60 {
        s.push_str(&format!("error[E{i:04}]: long failure with lots of detail at app.rs:{i}:1\n"));
    }
    s.push_str(&"x".repeat(5_000));
    s.push('\n');
    assert!(s.len() > 4_000);
    let out = inline_error_digest(&s, Some(400));
    if let Some(body) = out {
        assert!(
            estimate_tokens_smart(&body) <= 800,
            "digest must not exceed ~2× the declared budget"
        );
    }
}
```

## Cross-Module Findings

### [Warning] `distill_output` is called from three sites with two different cap defaults
- **Location**: `src/tools/result_processing.rs:567-589` (`distill_or_truncate`, default cap = `MAX_SALIENT_LINES = 60`); `src/tools/result_processing.rs:591-604` (`inline_error_digest`, default cap = `8`); `src/tool_output/hygiene.rs:209-216` (via `digest.render(salient_cap(budget_tokens))`).
- **Risk**: The three callers compute different caps from the same `Option<usize>` budget. `distill_or_truncate` passes `MAX_SALIENT_LINES` (60) as the default, `inline_error_digest` passes `8`, and `hygiene::salient_cap` passes `MAX_SALIENT_LINES` (60). When `budget_tokens = None` (no budget declared), the same text produces digests of very different sizes across the three sites — for `None` input, all should be the same default to preserve the "default = byte-for-byte" contract stated in `mod.rs`.
- **Current impact**: medium. The opaque-result path (inline_error_digest) is consistently 8× tighter than the typed-result path (distill_or_truncate), which is undocumented and likely unintended.
- **Suggestion**: Extract a single `pub(crate) fn digest_cap(budget_tokens: Option<usize>) -> usize` in `tool_output::distill` and call it from all three sites.

### [Warning] No git-repo-wide grep checks for callers of `pub fn distill_output` and `pub fn sanitize_command_output` after the no-newline precondition was added
- **Location**: search results from `src/` (grep for `distill_output`): callers in `src/tools/result_processing.rs:568`, `src/tools/result_processing.rs:600`, `src/tool_output/hygiene.rs:209`. All three call after the no-newline precondition was hardened.
- **Verification**: all three call sites are followed by a check that handles the `None` case (`distill_or_truncate` falls through to `truncate_with_budget`, `inline_error_digest` returns `None`, `hygiene::reduce_field` falls through to the next tier).
- **Current impact**: none (good). This entry documents that the cross-module call sites are all hardened; future callers added without the same discipline would silently regress.

### [Warning] `compress_result_value` walks with the shared `MAX_WALK_DEPTH = 4`, but `hoist_inline_images` uses a much larger `MAX_HOIST_DEPTH = 16`
- **Location**: `src/tool_output/walk.rs:21` (`MAX_WALK_DEPTH = 4`) vs `src/tools/result_processing.rs` (`MAX_HOIST_DEPTH = 16`).
- **Risk**: A deeply nested tool result (e.g., 8 levels of nesting) has its image payload extracted by `hoist_inline_images` but then **not seen** by the compressor or the hygiene pass (because MAX_WALK_DEPTH is 4). The image base64 stays in the text channel until the result budget amputates it — the exact failure the hoist exists to prevent, one layer of indentation deeper than the test's 4-level fixture.
- **Current impact**: low. Real tool results rarely nest past 4 levels, but the dispatch.rs layer's hoisting is provably incomplete for pathological shapes.
- **Suggestion**: Either expose `MAX_WALK_DEPTH` from `walk.rs` as `pub(crate)` and have `hoist_inline_images` use the same bound, or raise `MAX_WALK_DEPTH` to `MAX_HOIST_DEPTH` with a test of the deeper shape.

### [Warning] `distill::strip_ansi` is shared between `distill` and `sanitize`, but `sanitize_command_output` is the production sanitizer and `distill::strip_ansi` is the line-level one
- **Location**: `src/tool_output/distill.rs:92` (`pub(crate) fn strip_ansi`) and `src/tool_output/sanitize.rs:27` (`use super::distill::strip_ansi`).
- **Risk**: A future change to `distill::strip_ansi` (e.g., adding DCS support as suggested above) silently affects the sanitizer — both modules must agree on what "clean" means. The current sharing is the right choice (single source of truth for escape handling), but the contract is implicit and brittle if either module evolves.
- **Current impact**: low (currently aligned). Future regression risk.
- **Suggestion**: Add an inline `// shared by sanitize and distill` comment at the `strip_ansi` definition and a test that exercises both modules with the same fixture (DCS, OSC, C1) to lock in the contract.

### [Warning] `compress_result_value` mutates `value` via the walker and reports success, but the doc says "a passthrough does not count as a rewrite"
- **Location**: `src/tool_output/compressor.rs:131-145`
- **Observation**: The walker's callback replaces `*field` with the compressed text only when `rewrite_interior` returns `Some`, and only when the rewrite changed the content (`compressed != payload`). Both gates are in place; no spurious `changed = true` setting observed.
- **Current impact**: none (good). This entry confirms wiring correctness.

### [Warning] `lookahead` for `compress_generic` does not respect the JSON document structure when cutting mid-string
- **Location**: `src/tool_output/compressor.rs:444-457`
- **Risk**: `compress_generic` cuts at `max_bytes` and walks back to a char boundary, but does not check whether the cut landed inside a JSON string literal, between an escape and its terminator, or inside a markdown fence opener/closer. The downstream model receives a `[... output truncated, showing first N bytes of M total]` footer but the head of the truncated body may be syntactically invalid JSON or unbalanced markdown.
- **Current impact**: low. Documented behaviour for the "fallback" strategy; the field-level structured reducer handles the JSON case more gracefully.
- **Suggestion**: Document this in `compress_generic`'s docstring ("the fallback cuts mid-structure; callers needing JSON-safe truncation should prefer `compress_network_requests` or the structured reducer"), or detect the cut position and emit a `}`-balanced footer when JSON-shaped.

### [Warning] `compress_snapshot`'s `role_token` strips leading whitespace, list dashes, and `uid=` handles but not other prefixes the grammar uses
- **Location**: `src/tool_output/compressor.rs:255-267`
- **Risk**: Some AX-tree emitters prefix roles with tab indentation, multiple list dashes, or `>` (blockquote) markers. The current `trim_start().trim_start_matches('-').trim_start()` handles whitespace, dashes, and `uid=`, but stacked dashes (`--`) only strip one dash at a time (recursive iteration), and `>` prefixes are left intact.
- **Current impact**: low. Real Chrome DevTools MCP and Playwright emitters use the documented single-dash + `uid=` grammar.
- **Suggestion**: Add a `.trim_start_matches(|c: char| matches!(c, '-' | '>' | '|' | '*'))` after the whitespace strip, gated on a check that the trimmed content still starts with a known role or `uid=`.

### [Warning] `compress_screenshot`'s metadata branch keeps only 5 lines without character-cap
- **Location**: `src/tool_output/compressor.rs:233-244`
- **Risk**: The "may contain metadata lines before or instead of base64" branch keeps the first 5 lines verbatim — no `cap_line` (500 chars) is applied. A tool that returns 5 lines each 50 KB long (unlikely but possible if metadata is a JSON dump) keeps 250 KB. Compare to `compress_snapshot`, which applies `cap_line` per kept line.
- **Current impact**: low. Screenshot tools return 5-line metadata at most.
- **Suggestion**: Apply `cap_line` to each kept line in the metadata branch.

### [Warning] `distill::distill_output`'s `MAX_SALIENT_LINES = 60` is the same as `OutputDigest::render`'s default cap — but `inline_error_digest` overrides it to 8
- **Location**: `src/tool_output/distill.rs:78` (`MAX_SALIENT_LINES = 60`) vs `src/tools/result_processing.rs:604` (`map_or(8, ...)`)
- **Risk**: The `MAX_SALIENT_LINES` constant is exported for "the caller scales down from when it has a token budget". Two callers scale to 60 (default) and one scales to 8. The 60-line default is meant for the field-wise ingress (`hygiene::digest.render(salient_cap(budget))`); the 8-line default is for the inline error preview. The inconsistency is intentional per the inline_error_digest doc, but `MAX_SALIENT_LINES = 60` and the inline preview's `8` are not connected by a single source.
- **Current impact**: low. Documented.
- **Suggestion**: Add a `distill::INLINE_PREVIEW_LINES: usize = 8` constant in `distill.rs` and reference it from `inline_error_digest`.

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 25 |
| Suggested Test | 8 |

---

## Module-Level Assessment

The `src/tool_output` module is **wiring-correct and well-tested at the level of common shapes**. The five-phase audit found no Critical findings: no panic-unsafe `unwrap`s in production code, no missing match arms over `ContentKind` variants, no missing error-propagation context, no use of `as` casts that lose information, no `&s[..n]` byte-slicing on multi-byte strings (every cap operation uses `chars().take(n)` or `is_char_boundary`).

The 25 Warnings fall into four clusters:

1. **Cap-and-marker arithmetic** (3): the JSON reducer's `cap + 1` shape, the dedup-across-blank-lines semantic, the distiller's `total_lines` blank-line count.
2. **Heuristic false positives** (8): base64 detection on alphanumeric-`+` strings, uppercase data URLs, role-case sensitivity, URL vs `file:line` in `extract_path`, logcat `e/` prefix, `is_continuation` over-matching, `devtools_tool_name` matching any MCP server, sanitiser undercounting in `size_hint`.
3. **Path/edge-case ergonomics** (5): empty result for single-line oversized snapshot, `truncate_with_budget(0)` returning full input, `inline_error_digest` skipping the size check, `clean_for_ingress` mutating `value` on rejection, `MAX_WALK_DEPTH` mismatch with `MAX_HOIST_DEPTH`.
4. **ANSI / control-byte coverage** (2): DCS/PM/APC tail leak in `strip_ansi`, `compress_screenshot`'s metadata branch lacking `cap_line`.

The most impactful Warnings are the **heuristic false positives in `compress_screenshot` and `devtools_tool_name`** — both can silently lose data when the input shape doesn't match the assumed grammar. The **dispatch inconsistency between `distill_or_truncate` and `inline_error_digest`** (cap default 60 vs 8; budget check present vs absent) is the largest cross-module contract drift.

The 8 Suggested Tests cover the cap-and-marker arithmetic, base64 mis-detection, single-line snapshot degeneration, data URL case sensitivity, URL path extraction, JSON salient-scalar priority under alphabetical crowding, and the budget=0 truncator edge case. Adding these would lock in the most likely regression vectors.

No fixes applied (per the audit instructions); report only.
