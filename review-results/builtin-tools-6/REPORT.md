# Builtin Tools Batch 6 — small modules + single-file tools

**Date**: 2026-08-11
**Path**: `src/builtin_tools/{pdf_generate,generation,web_fetch,skill_reader,media_tools,voice_tools,pim}/*` + 60+ single-file tools under `src/builtin_tools/*.rs` (~13k lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    1 |     2 |   4 |    7 |

---

## Findings

### [HIGH] pdf_generate/* — `PdfGenerateArgs.content: String` has no per-call size cap at the dispatcher
- **Category**: DoS
- **Description**: `pdf_generate/mod.rs` accepts arbitrary-length Markdown content and forwards it to either the browser engine (HTML → print) or the native engine (`native_engine::wrap_text` with text-wrapping loop). The native path *does* have a `max_width_mm` and font size, but neither engine caps input size: a 100 MB Markdown string is parsed in full by `pulldown-cmark` (browser path) or scanned char-by-char (`native_engine::wrap_text`) before any output cap fires. Browser engines that load the wrapped HTML will OOM.
- **Suggested fix**: Add `const MAX_PDF_CONTENT_CHARS: usize = 4 * 1024 * 1024;` at the top of `pdf_generate/mod.rs` (4 MiB is generous for any real document; the practical ceiling is single-digit MB), check `args.content.chars().count() > MAX_PDF_CONTENT_CHARS` in `call`, return a structured refusal pointing at chunked `pdf_generate` calls for genuinely large documents.

### [MEDIUM] generation/image_generate.rs:115, video_generate.rs:78 — prompt length is `args.prompt.len()` (bytes) — clipped to the nearest valid UTF-8 boundary by the downstream, but no early reject
- **Category**: DoS
- **Description**: `let i = self.config.max_chars.unwrap_or(MAX_CHARS).map_or(args.prompt.len(), |(i, _)| i);` truncates to `max_chars`, but `max_chars` is an *advisor* cap on output — there is no upper bound on the *prompt* itself at the tool dispatcher. A 100 MB prompt passes through to the image provider API, which either times out, charges for the request, or returns an error opaque to the model.
- **Suggested fix**: Add `const MAX_GENERATE_PROMPT_CHARS: usize = 32 * 1024;` at the top of `generation/mod.rs` (or `image_generate.rs`), check `args.prompt.chars().count() > MAX_GENERATE_PROMPT_CHARS` at the top of both `image_generate` and `video_generate` and refuse with a message naming the cap.

### [MEDIUM] crawl4ai.rs — `fetch_markdown(url)` is invoked from `web_fetch`, which already has `MAX_RESPONSE_BYTES`, but `Markdown::Text(String)` is not size-bounded before returning to the model
- **Category**: DoS
- **Description**: `crawl4ai`'s `fit_markdown` / `raw_markdown` field can be tens of MB on a single page. The web_fetch dispatcher clips to `MAX_RESPONSE_BYTES = 10 MiB`, but the *markdown* field is parsed *before* that clip — a hostile crawl4ai server can return a 100 MB `fit_markdown` and the parse step blows up.
- **Suggested fix**: In `crawl4ai::Markdown::into_text`, truncate to the same `MAX_RESPONSE_BYTES` cap (`take(MAX_RESPONSE_BYTES)` on the resulting String) so the parse step is bounded. This is defense-in-depth; the upstream cap should fire but a refusal to parse 100 MB is cheap.

### [LOW] voice_tools/* — `LocalVoiceArgs` has no transcript-length cap on the model side
- **Category**: DoS
- **Description**: `LocalVoiceTool` is mostly a config-status tool (`active provider / model / endpoint`). It does not accept arbitrary transcripts; the *consumer* (`media_tools/transcribe`) is the one that returns the transcript. The medium is gated at `media_tools::extract.rs`, so this is observation only.
- **Suggested fix**: None — flagging only so the next reviewer does not double-flag.

### [LOW] skill_install.rs / skill_status.rs — install tools are intentionally agent-driven via `hub/install_run`; `skill_install` is a thin dispatcher
- **Category**: architecture (positive observation)
- **Description**: The single-file `skill_install.rs` delegates to `hub/install_run::gate`, which is the security core. Reading both confirms the gate is the only path to install — there is no shortcut in `skill_install` itself.
- **Suggested fix**: None.

### [LOW] code_check.rs — `MAX_DIAGNOSTICS = 50` is correct, but `tail(s, max_bytes)` uses `s.len() - max_bytes` and can panic at a non-UTF-8 boundary on pathological compiler output
- **Category**: robustness
- **Description**: `tail` slices `s` from a byte offset; if `max_bytes` lands mid-codepoint the resulting `&str` panics on construction (`byte index … is not a char boundary`). `cargo --message-format=json` is ASCII, but lint tools (`tsc`, `eslint`, `clippy` with `clippy::pedantic`) sometimes emit UTF-8 paths and messages.
- **Suggested fix**: Walk back from the slice start to the previous char boundary (`while !s.is_char_boundary(start) { start -= 1; }`). Two extra lines.

### [LOW] loop_manage.rs, goal.rs, moa_manage.rs — iteration caps (`pursuit_max_iterations`, etc.) are caller-supplied and not upper-bounded
- **Category**: DoS
- **Description**: All three accept `Option<u32>` for iteration caps without an upper bound. `goal.rs` is the most exposed (`pursuit_max_iterations: Option<u32>`). A model setting `pursuit_max_iterations = u32::MAX` then submitting a pursuit that does not converge keeps the worker thread alive for days.
- **Suggested fix**: Add `MAX_GOAL_ITERATIONS: u32 = 10_000;` (or similar) at the top of each module and clamp at the top of `call`. Pure perf hardening.

---

## Strengths

- `code_exec.rs` and `bash_exec.rs` route every subprocess through `Arc<dyn Sandbox>` with capability approval, per-session workspace, OS seatbelt/bwrap profile, and a `FOREGROUND_MAX_TIMEOUT_SECS = 170` clamp that runs *before* the tool budget wrapper fires. Comment chain at lines 49-65 is the right shape.
- `web_fetch/mod.rs` has both a per-call `MAX_RESPONSE_BYTES = 10 MiB` (memory bound) and `DEFAULT_MAX_CONTENT_LENGTH = 10 KiB` (tool-result bound), with `extract::validate_html_safety` between them so an oversized body never enters the parser unchecked.
- `pdf_generate/styles.rs::html_escape` is called at the boundary before the browser engine sees the content; XSS-style injection is closed at the right layer.
- `pim/mod.rs:430` clamps `args.limit` to `[1, 200]` regardless of caller input. The other surface in the same module mirrors it.
- `skill_reader/read.rs` enforces a `5 MiB max_file_size` per file, with the same canonicalize-and-recheck path that `file_ops/path_utils.rs` uses — the two layers agree on what "inside the skill dir" means.
- `cron_manage.rs:404` clamps `runs_limit` to `[1, MAX_RUNS_LIMIT = 50]`; the cap and the clamp are spelled out, not implicit.
- `process_registry.rs:64` and `:74` define `MAX_ENTRIES = 64` and `MAX_RUNNING_PER_SESSION = 8`; the comment at lines 68-72 explains the rationale ("64 matches the registry's `MAX_ENTRIES`, so a restart still fits"). The constants are the right shape.
- `code_check.rs::MAX_DIAGNOSTICS = 50` plus the `truncated` flag at line 356 is the right contract — model sees the cap explicitly.
- `media_tools/*`, `voice_tools/*`, `pim/*` are uniformly thin adapters over platform capabilities with consistent validation patterns.

---

## Recommended Single Fix

A shared `src/builtin_tools/limits.rs` (or extending the existing `MAX_*` constants in each module) exporting `MAX_PDF_CONTENT_CHARS`, `MAX_GENERATE_PROMPT_CHARS`, and `MAX_GOAL_ITERATIONS` would close HIGH #1, MEDIUM #2, and LOW #5/#7 in one small refactor. The shared module is the natural home for the constants Batch 5 also needs (`MAX_CROSS_SESSION_MESSAGE_CHARS`, `MAX_NOTE_BODY_CHARS`), so one module closes 4 batches' worth of size-cap gaps.