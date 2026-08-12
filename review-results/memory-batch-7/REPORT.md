# Memory Batch 7 — `src/memory/{project_scope,insights,streaming_scrubber,reembed,content_scanner,embedding_*,explain,scratchpad,session_memory_mode,namespace,proptest_enums,loom_concurrency,integration_tests}/*` Code Review

**Date**: 2026-08-12
**Path**: 16 top-level + 1 integration_tests file, ~5 000 lines
**Reviewer**: static (security / logic / architecture / quality)

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     5 |    5 |   12 |

---

## Findings

### [HIGH] `project_scope.rs:155-200` — `list_note_corpora` reads `memory_dir` once but the loop yields `flatten()`'d errors silently
- **Category**: logic
- **Description**: `let Ok(entries) = std::fs::read_dir(memory_dir) else { return Vec::new() }; let mut ids: Vec<String> = entries.flatten()...`. The `flatten()` on `Result<DirEntry, io::Error>` silently drops per-entry errors (e.g. a permissions error on a sub-corpus). For a vault with a partial permissions issue, the function returns a partial list and the dream fan-out silently skips the un-listed corpora.
- **Suggested fix**: `let mut ids = Vec::new(); for entry in entries { match entry { Ok(e) => ids.push(...), Err(e) => tracing::warn!(?e, "list_note_corpora: entry read failed") } }`. The error path becomes observable.

### [HIGH] `insights.rs:120-150` — `aggregate_tool_usage` reads `get_raw_by_source` with `fetch_limit` and silently drops rows beyond it
- **Category**: DoS
- **Description**: `fetch_tool_invocation_rows` reads at most `fetch_limit` rows (default 50 000). A high-traffic agent with 200 000 invocations sees the *newest* 50 000; the oldest 150 000 are silently dropped, and the window analysis under-counts every tool. The result is a "the daemon is fine" summary that hides the most important data.
- **Suggested fix**: Surface the truncation as a return value: `aggregate_tool_usage` returns `(report, truncated: bool)`. The admin RPC renders the flag; the dream stage can opt into a different fetch mode.

### [MEDIUM] `streaming_scrubber.rs:155-180` — `loop {}` reading the streaming buffer has no iteration cap
- **Category**: DoS
- **Description**: The scrubber reads a streaming input and tokenises `<...>` blocks. The `loop {}` body has a `break;` on end-of-stream, but a stream that never ends (a hostile LLM emitting `<noise>` until cut) runs the scrubber forever.
- **Suggested fix**: Add a per-call `MAX_TOKENS = 100_000` counter; the loop breaks when reached and emits a `truncated: true` flag on the result.

### [MEDIUM] `reembed.rs:1-100` — `reembed_all` walks the entire note vault in a single pass with no rate limit
- **Category**: DoS
- **Description**: A vault with 50 K notes triggers 50 K embedding calls. The embedding provider (an HTTP API) has its own rate limit, but the function does not respect it; the rate limit is enforced by HTTP 429s, which the function then retries naively.
- **Suggested fix**: Token-bucket or per-second rate limit. Read from a config knob (`reembed.rate_per_second`).

### [MEDIUM] `embedding_manager.rs:22-36` — `Arc<RwLock<...>>` is held across `async fn` calls
- **Category**: architecture
- **Description**: The `RwLock` is `tokio::sync::RwLock`; the `async fn` reads inside `should_run` hold the read guard across an `await`. The pattern is correct for `tokio::sync::RwLock` but a future refactor to `parking_lot::RwLock` would silently introduce a cross-await blocking lock.
- **Suggested fix**: Add a `// SAFETY: must be tokio::sync::RwLock; do not switch to std::sync::RwLock` comment, or wrap the read in a `try_read` + `await` pattern that drops the guard before the await.

### [MEDIUM] `content_scanner.rs:10, 200-260` — `regex::Regex` for content scanning is unanchored
- **Category**: logic
- **Description**: The scanner looks for secrets in user content. The regexes are not all anchored; a secret token that contains a substring of an allowed pattern is miscounted. The function returns a `Vec<Finding>` with the matched span; a miscounted match is a false positive.
- **Suggested fix**: Anchor every regex at the start and end. Add a test for the substring case.

### [MEDIUM] `project_scope.rs:300-360` — `read_scope_ids` returns a `Vec<String>` per call; callers iterate the vec per call
- **Category**: performance
- **Description**: For a hot path (recall), the function is called per query. The Vec allocation is wasted when the namespace is global (returns `[base]`).
- **Suggested fix**: Return `&[String]` with a thread-local or `OnceLock` for the global case.

### [LOW] `insights.rs:200-300` — `build_report` is a 200-line function; should be split
- **Category**: quality
- **Description**: One mega-function that does per-tool aggregation, window filtering, and percentage computation. The split is mechanical.
- **Suggested fix**: Extract `aggregate_per_tool(rows) -> HashMap<String, ToolBreakdown>` and `window_filter(rows, since) -> &[RawMemory]`.

### [LOW] `scratchpad/manager.rs:1473` — `while let Ok(Some(entry)) = read_dir.next_entry().await` swallows `Err(_)` from `read_dir`
- **Category**: logic
- **Description**: A permissions error on a single subdir is dropped. The scratchpad loader silently loads a partial plan.
- **Suggested fix**: `let entry = match read_dir.next_entry().await { Ok(Some(e)) => e, Ok(None) => break, Err(e) => { tracing::warn!(?e, "scratchpad: read_dir failed"); break; } }`.

### [LOW] `embedding_resolver.rs:1-100` — `EmbeddingResolver` is a 100-line match on the model name; an unknown model panics in the default arm
- **Category**: logic
- **Description**: A user adds a new embedding model to the TOML; the resolver falls into the `_ => ...` arm and panics. The error message names the unknown model, but a panic at config-load time is too aggressive.
- **Suggested fix**: Return `AlephError::config(format!("unsupported embedding model: {name}"))` instead of panicking.

### [LOW] `explain.rs:1-100` — `ExplainedEvent` is a struct with 6 fields; the public `from_event` constructor takes a reference to the same struct
- **Category**: quality
- **Description**: The type is small enough that a tuple variant would be clearer.
- **Suggested fix**: Either rename to a clearer name or use a tuple.

### [LOW] `proptest_enums.rs:1-100` — `arb_*` for the proptest derives has no `Arbitrary` for the f32 fields; NaN is allowed
- **Category**: quality
- **Description**: A proptest that generates `f32::NAN` as a confidence value crashes downstream comparisons. The `Arbitrary` impl for `f32` is unrestricted.
- **Suggested fix**: Wrap in `confidence: prop::sample::select(vec![0.0, 0.25, 0.5, 0.75, 1.0])` for the relevant proptests.

## Cross-References

- `project_scope.rs:155-200` and `scratchpad/manager.rs:1473` — both silently drop `Err(_)` from `read_dir`. A `fs::read_dir_or_warn(path)` helper would close both.
- `insights.rs:120-150` and `reembed.rs` — both walk an unbounded row count. The truncation flag should be a return-value, not a log warning.
- `embedding_manager.rs:22-36` and `extensions/registry.rs:32-50` — both use a `RwLock` around async work. The safety comment is worth a unit test that pins the type.

## Strengths

- `project_scope.rs::list_note_corpora` is the *single source* for "which corpora exist". The doc-comment explains the unification (three previous answers → one).
- `project_scope.rs::scoped_agent_id` and `read_scope_ids` are byte-stable: the global namespace returns the base id unchanged. A pre-P1 caller gets byte-for-byte the same result.
- `insights.rs` keeps the per-tool and per-failure aggregators on the same read (`fetch_tool_invocation_rows`) so the two cannot disagree about the counts.
- `streaming_scrubber.rs` correctly handles the case where the open tag appears inside a closed block; the state machine is right.
- `reembed.rs` has a separate `reembed_one` for per-note re-embedding; the bulk path is a fan-out of the per-note path. The shape is right.
- `embedding_manager.rs` is hot-swap-capable: `RwLock<Option<Arc<dyn EmbeddingProvider>>>` allows a graceful swap mid-flight.
- `content_scanner.rs` runs as a pre-commit hook; the cost is bounded by the file size, not by the scan budget. A `MAX_FILE_BYTES` cap is the natural next step.
- `explain.rs` is small and focused. The 6-field struct is at the edge of where splitting would help.
- `scratchpad/manager.rs` is the single chokepoint for scratchpad state. The Plan/Item/Status types are owned by this module.
