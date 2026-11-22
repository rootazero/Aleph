# Occam's Razor Classification Report: Batch PROVIDERS_PROTOCOLS_OPENAI

- **Batch Target**: `PROVIDERS_PROTOCOLS_OPENAI`
- **Base Directory**: `src/providers/protocols/openai_common`, `src/providers/protocols/openai_chat`, `src/providers/protocols/openai_responses`
- **Doctrine**: `/rust-occams-razor`
- **Review Date**: 2026-11-11

---

## 1. Executive Summary

This static analysis sweep inspected 18 files covering `openai_common`, `openai_chat`, and `openai_responses`. The primary Occam's Razor anti-patterns identified in this batch involve:
1. **Streaming SSE Buffer Overhead**: Redundant allocations (`Vec<u8>` and `String`) for every newline in `sse_event_stream` and `stream_deltas` across three separate implementations (`openai_common/sse.rs`, `openai_chat/adapter.rs`, `openai_responses/mod.rs`).
2. **Schema In-Place Manipulation Redundancy**: Cloned map structures before `.clear()` in strict and lenient schema rewrites (`openai_strict_schema.rs`), when `std::mem::take` would eliminate deep clones of schema objects.
3. **Redundant Key Allocations**: Allocating `Vec<String>` of map keys just to iterate `.values_mut()` (`tools.rs::ensure_properties_recursive`).
4. **Intermediate AST Struct Duplication**: Mapping `ContentBlock::ToolCall` -> `OpenAiToolCall` -> `OpenAiToolCallOut` in `proto_impl.rs`.
5. **String/Allocation Overhead in Common Lookups**: `to_ascii_lowercase()` and `to_string()` on static strings in `model_id.rs`, `reasoning_effort.rs`, and `tools.rs`.

### Metric Overview

| Taxonomy Category | Candidate Count |
|-------------------|-----------------|
| `REDUNDANT-ALLOCATION` | 20 |
| `ACCIDENTAL-COMPLEXITY` / `INDIRECTION` | 5 |
| `CONTROL-FLOW` | 1 |
| **Total Findings** | **26** |

| Action Tag | Count |
|------------|-------|
| `FIX-NOW` | 20 |
| `FIX-CAREFULLY` | 6 |
| `DEFER` | 0 |
| `REJECT` | 0 |

---

## 2. Taxonomy Classification & Routing

### Category: REDUNDANT-ALLOCATION

| ID | File | Lines | Description | Routing |
|---|---|---|---|---|
| RA-01 | `src/providers/protocols/openai_common/model_id.rs` | 44 | `core.to_ascii_lowercase()` allocates heap `String` on every model normalization call even when lowercase or non-matching. | `FIX-NOW` |
| RA-02 | `src/providers/protocols/openai_common/model_id.rs` | 54-58 | `Cow::Owned(c.to_string())` for static string literals and `Cow::Owned(core.to_string())` for borrowed subslices when `Cow::Borrowed` works zero-allocation. | `FIX-NOW` |
| RA-03 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 160-165 | `intersection.intersection(&set).cloned().collect()` re-allocates a new `HashSet<String>` on each iteration instead of in-place `retain`. | `FIX-NOW` |
| RA-04 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 252-277 | `map.get("type")...cloned()` clones entire `Vec<Value>` upfront; mapping key/value pairs into `non_null_branch` before `map.clear()` deep-clones entire schema objects instead of using `std::mem::take(map)`. | `FIX-NOW` |
| RA-05 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 366-391 | Same deep clone of all map keys/values before `map.clear()` in `normalize_node`. | `FIX-NOW` |
| RA-06 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 487-498 | `collect_defs` deep-clones all definitions into a HashMap, then caller strips `$defs` from schema. Can take/remove definitions directly. | `FIX-CAREFULLY` |
| RA-07 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 652-684 | `infer_type_from_structure` and `infer_type_from_value` return owned `String` for static types (`"string"`, `"object"`), allocating in `HashSet<String>`. | `FIX-NOW` |
| RA-08 | `src/providers/protocols/openai_common/openai_strict_schema.rs` | 426, 436, 446 | `format!("{path}.properties.{k}")` allocates pointer path on every node even when schema is valid and no error is emitted. | `FIX-CAREFULLY` |
| RA-09 | `src/providers/protocols/openai_common/prompt_cache.rs` | 74 | `tool.parameters.to_string().as_bytes()` serializes whole JSON schema to heap `String` before updating hasher. | `FIX-NOW` |
| RA-10 | `src/providers/protocols/openai_common/prompt_cache.rs` | 110-121 | `clamp_prompt_cache_key`: `Some(clamped) => Cow::Owned(clamped.to_string())` allocates `String` for subslice `&key[..byte_idx]`. | `FIX-CAREFULLY` |
| RA-11 | `src/providers/protocols/openai_common/reasoning_effort.rs` | 45-51 | `normalize_for_family` allocates `String` via `to_ascii_lowercase()` and a second `String` via `head.to_string()`. | `FIX-NOW` |
| RA-12 | `src/providers/protocols/openai_common/reasoning_effort.rs` | 77 | `rest.chars().take_while(...).collect::<String>().parse()` allocates intermediate `String` instead of parsing string slice directly. | `FIX-NOW` |
| RA-13 | `src/providers/protocols/openai_common/sse.rs` | 72-84 | `sse_event_stream`: allocates `line_bytes: Vec<u8>` and `line: String` on every single newline in the SSE stream. | `FIX-CAREFULLY` |
| RA-14 | `src/providers/protocols/openai_common/tools.rs` | 74-76 | `let keys: Vec<String> = obj.keys().cloned().collect();` clones all keys into a vector, yet the loop only accesses `obj.values_mut()`. | `FIX-NOW` |
| RA-15 | `src/providers/protocols/openai_common/tools.rs` | 9-19 | `sanitize_tool_name`: always allocates `String` via `.collect()` even when name is already valid `[a-zA-Z0-9_-]`. | `FIX-CAREFULLY` |
| RA-16 | `src/providers/protocols/openai_common/usage_limit.rs` | 15 | `body.to_ascii_lowercase()` allocates full `String` copy of HTTP error body for substring matching. | `FIX-NOW` |
| RA-17 | `src/providers/protocols/openai_chat/adapter.rs` | 123-127 | `raw.split(',').map(|s| s.trim().to_string()).filter(...)` allocates owned `String` before filtering empty entries. | `FIX-NOW` |
| RA-18 | `src/providers/protocols/openai_chat/adapter.rs` | 420-430 | `stream_deltas`: allocates `line_bytes: Vec<u8>` and `line: String` on every single streamed SSE line. | `FIX-CAREFULLY` |
| RA-19 | `src/providers/protocols/openai_chat/proto_impl.rs` | 141, 155, 224 | `.collect::<Vec<_>>().join("\n")` allocates intermediate vectors of `&str` just to join. Single block doesn't need join. | `FIX-NOW` |
| RA-20 | `src/providers/protocols/openai_chat/proto_impl.rs` | 260-271 | `map_think_level` returns `Option<String>` allocating 6 static string literals when caller only needs `&str`. | `FIX-NOW` |
| RA-21 | `src/providers/protocols/openai_chat/sse.rs` | 167-187 | `id` string slice converted via `id.to_string()` 3 separate times in tool call start and pending arg emission. | `FIX-NOW` |
| RA-22 | `src/providers/protocols/openai_responses/mod.rs` | 180-191 | `raw.split(',').map(|s| s.trim().to_string()).filter(...)` maps to owned `String` before filtering empties. | `FIX-NOW` |
| RA-23 | `src/providers/protocols/openai_responses/mod.rs` | 475-485 | Responses `stream_deltas`: allocates `line_bytes: Vec<u8>` and `line: String` on every single streamed SSE line. | `FIX-CAREFULLY` |
| RA-24 | `src/providers/protocols/openai_responses/mod.rs` | 633-640 | `item_to_call.get(&item_id).cloned()` clones `call_id` upfront, then clones again for `ToolCallArgsComplete`. | `FIX-NOW` |

### Category: ACCIDENTAL-COMPLEXITY / INDIRECTION

| ID | File | Lines | Description | Routing |
|---|---|---|---|---|
| AC-01 | `src/providers/protocols/openai_common/tools.rs` | 21-25 | `desanitize_tool_name(name: &str) -> String { name.to_string() }` is an identity function posing as reverse-sanitization logic. | `FIX-NOW` |
| AC-02 | `src/providers/protocols/openai_chat/proto_impl.rs` | 159-177, 204-214 | Content tool calls are converted into `OpenAiToolCall`, then immediately mapped to `OpenAiToolCallOut`. `OpenAiToolCall` is an unneeded intermediate structure. | `FIX-NOW` |

### Category: CONTROL-FLOW

| ID | File | Lines | Description | Routing |
|---|---|---|---|---|
| CF-01 | `src/providers/protocols/openai_chat/sse.rs` | 200-207 | `tracker.get(index).map(str::to_string)` called ahead of `match` to sidestep early borrow checker limitation; Rust 2021 NLL allows `if let Some` without cloning. | `FIX-NOW` |

---

## 3. Action Routing Detailed Recommendations

### FIX-NOW (20 items)

1. **`openai_common/model_id.rs:44`**: Replace unconditional `core.to_ascii_lowercase()` with `eq_ignore_ascii_case` checks against the 6 typo candidates.
2. **`openai_common/model_id.rs:54-58`**: Replace `Cow::Owned(c.to_string())` with `Cow::Borrowed(c)` and `Cow::Owned(core.to_string())` with `Cow::Borrowed(core)`. Completely zero-allocation.
3. **`openai_common/openai_strict_schema.rs:160-165`**: Replace `intersection = intersection.intersection(&set).cloned().collect()` with `intersection.retain(|item| set.contains(item))`.
4. **`openai_common/openai_strict_schema.rs:252-277`**: In `lenient_rewrite_node`, borrow `map.get("type").and_then(|v| v.as_array())` without cloning. Extract `other_type`, then use `let mut non_null_branch = std::mem::take(map); non_null_branch.remove("type");` to avoid deep cloning all keys/values.
5. **`openai_common/openai_strict_schema.rs:366-391`**: In `normalize_node`, apply the same `std::mem::take(map)` pattern to avoid deep cloning all schema properties.
6. **`openai_common/openai_strict_schema.rs:652-684`**: Return `&'static str` for `infer_type_from_structure` and `infer_type_from_value`, and use `HashSet<&'static str>` in `infer_type_from_values`.
7. **`openai_common/prompt_cache.rs:74`**: Stream JSON bytes to hasher via `serde_json::to_writer` with a small `Write` adapter over `Sha256`, avoiding intermediate schema string allocation.
8. **`openai_common/reasoning_effort.rs:45-51`**: In `normalize_for_family`, run `strip_date_suffix` on trimmed slice directly and avoid redundant lowercase String allocations when string is already ASCII lowercase.
9. **`openai_common/reasoning_effort.rs:77`**: In `minor_of`, parse `&rest[..end]` directly instead of collecting characters into a `String`.
10. **`openai_common/tools.rs:21-25`**: Inline `name.to_string()` and eliminate the misleading `desanitize_tool_name` stub.
11. **`openai_common/tools.rs:74-76`**: In `ensure_properties_recursive`, remove `let keys: Vec<String> = obj.keys().cloned().collect();` and iterate `for v in obj.values_mut()`.
12. **`openai_common/usage_limit.rs:15`**: Perform case-insensitive search or only lowercase when uppercase characters are detected.
13. **`openai_chat/adapter.rs:123-127`**: In `stop_sequences` parsing, filter `!s.is_empty()` before allocating `String`.
14. **`openai_chat/proto_impl.rs:141, 155, 224`**: Avoid intermediate `Vec<&str>` allocation before `.join("\n")`, especially when `content.len() == 1`.
15. **`openai_chat/proto_impl.rs:159-177, 204-214`**: Directly construct `OpenAiToolCallOut` from `ContentBlock::ToolCall`, removing the intermediate `OpenAiToolCall` vector allocation.
16. **`openai_chat/proto_impl.rs:260-271`**: Change `map_think_level` return type to `Option<&'static str>` since caller only consumes `&str`.
17. **`openai_chat/sse.rs:167-187`**: Create `let id_str = id.to_string();` once and reuse it across `tracker.track` and `ToolCallStart`.
18. **`openai_chat/sse.rs:200-207`**: Replace `tracker.get(index).map(str::to_string)` with `if let Some(call_id) = tracker.get(index)` using modern NLL.
19. **`openai_responses/mod.rs:180-191`**: Filter `!s.is_empty()` before `to_string()` in `stop_sequences` parsing.
20. **`openai_responses/mod.rs:633-640`**: Borrow `item_to_call.get(&item_id)` instead of cloning upfront.

### FIX-CAREFULLY (6 items)

1. **`openai_common/openai_strict_schema.rs:487-498`**: Take `$defs` / `definitions` map directly from `schema.as_object_mut()` rather than deep cloning all definitions and stripping them afterwards. Requires verifying definition references are strictly acyclic.
2. **`openai_common/openai_strict_schema.rs:426, 436, 446`**: Lazy path construction for `normalize_node` so that string paths are only allocated on `Incompatible` return branches. Requires checking JSON pointer path compatibility with existing tests.
3. **`openai_common/prompt_cache.rs:110-121`**: Make `clamp_prompt_cache_key` return `Cow::Borrowed(clamped)` (or `&str`). Requires updating unit test `over_limit_truncated_to_64_chars` which asserted `Cow::Owned`.
4. **`openai_common/sse.rs:72-84`**: Validate UTF-8 slice `&buf_guard[..newline_pos]` in-place before draining, avoiding intermediate `Vec<u8>` allocation per line.
5. **`openai_common/tools.rs:9-19`**: Convert `sanitize_tool_name` to return `Cow<'_, str>` so valid names are zero-copy. Requires checking caller types.
6. **`openai_chat/adapter.rs:420-430` & `openai_responses/mod.rs:475-485`**: Shared zero-copy line parsing for streaming SSE `line_buf` in both Chat and Responses stream loops.

---

## 4. Verification Directives

- Zero public API breaks for external-facing types.
- All SSE parsing changes must maintain exact framing behavior for multi-byte UTF-8 boundaries split across HTTP chunk boundaries.
- Characterization tests in `openai_chat/tests.rs` and `openai_responses/tests.rs` must pass verbatim.
