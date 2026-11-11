# Tie-break verdicts (third family: minimax-cn/MiniMax-M3.1-Flash)

### IF1 IF1-07
- A: PARTIAL — The supplied diff contains no identifiable change corresponding to IF1-07, so the finding cannot be considered fixed.
- B: (absent) — 
- **TIE-BREAK: NOT_FIXED** — The evidence contains only “(no finding text found)” and “(no excerpt),” so no source line demonstrates that the failure mode was fixed.

### IF1 IF1-03
- A: PARTIAL — The supplied diff contains no identifiable change corresponding to IF1-03, so the finding cannot be considered fixed.
- B: (absent) — 
- **TIE-BREAK: NOT_FIXED** — The decisive line is `(no excerpt)`: no source change is provided that demonstrates the failure mode was removed.

### IF1 IF1-10
- A: PARTIAL — The supplied diff contains no identifiable change corresponding to IF1-10, so the finding cannot be considered fixed.
- B: (absent) — 
- **TIE-BREAK: NOT_FIXED** — The source excerpt is `(no excerpt)`, so no changed line demonstrates that the failure mode is gone. The finding text is also absent, providing no basis to conclude IF1-10 was fixed.

### IF1 IF1-18
- A: PARTIAL — The supplied diff contains no identifiable change corresponding to IF1-18, so the finding cannot be considered fixed.
- B: (absent) — 
- **TIE-BREAK: NOT_FIXED** — The source contains only “(no excerpt)” and the finding states “(no finding text found),” so there is no evidence that the failure mode was fixed.

### E5 E5-03
- A: PARTIAL — The diff restricts the placeholder to the path and rechecks the final URL (`kernel.check_http_request(&method, &egress_url)`), but it still replaces every occurrence in the path (`let path = parsed.path().replace(encoded_placeholder, &encoded);`) rather than a single parsed path segment, so a placeholder occurring in multiple segments can be injected in multiple locations; the diff does not prove the required final-URL check is performed immediately before sending or that the existing header/query/rate-limit behavior is preserved.
- B: FIXED — The diff replaces the raw `url.replace(placeholder, &encoded)` at `credential_injector.rs:173` with a path-segment-only splice: it rejects placeholders containing `/` or empty, encodes the placeholder via `set_path` to obtain its encoded form, verifies `parsed.path().split('/').any(|segment| segment.contains(encoded_placeholder))`, then replaces only within `parsed.path()` and rebuilds via `modified.set_path(&path)` — so the scheme/authority/query/fragment can no longer be altered. Additionally `host_functions.rs` now re-runs `kernel.check_http_request(&method, &egress_url)` immediately before egress with the comment "Credential injection may change the path; validate the final target too", satisfying the second half of the suggested fix.
- **TIE-BREAK: FIXED** — `"let path = parsed.path().replace(encoded_placeholder, &encoded);"` confines replacement to the parsed path, so the placeholder can no longer rewrite the scheme or authority and produce `https://evil.example.com/v1/data`.

### IF1 IF1-17
- A: REGRESSION — The replacement leaves a duplicated/braced tail: after the new loop it contains `if !image_buf.is_empty() { ... } }` followed by another `if !image_buf.is_empty()`, which is malformed/braces do not align and can fail compilation; the ordering logic itself also lacks the promised immediate-send guarantee until the diff is syntactically corrected.
- B: FIXED — `delivery.rs:send_attachments` now sends each non-image attachment immediately after flushing the preceding image album (`send_attachment(bot, chat_id, thread_id, att).await?;` inside the `else` branch), eliminating the `singles` buffer that reordered documents after trailing images.
- **TIE-BREAK: FIXED** — The non-image branch now executes `send_attachment(bot, chat_id, thread_id, att).await?;` immediately after flushing the preceding image album, so documents are no longer deferred behind trailing images by a `singles` buffer.

### IF6 IF6-15
- A: PARTIAL — The diff replaces the unbounded fetch with bounded UID/body windows and `is_complete_imap_body`, but it still performs `session.uid_search(...)` over every sequence window and can collect up to 32 fetched bodies per batch; more importantly, the `SMALLER` search condition is not an authorization or cumulative-memory bound, and `mailbox.exists` is used as a sequence-number bound rather than a bounded UID set. The stated unbounded-mailbox fetch/collect failure is materially reduced but not fully eliminated/proven.
- B: FIXED — `poll_imap_once` now iterates `(1..=mailbox.exists).step_by(MAX_IMAP_BATCH_MESSAGES)` with `uid_search("{first}:{last} UNSEEN SMALLER {cap}")`, fetches via `uid_fetch(..., "(BODY.PEEK[]<0.{cap}> UID RFC822.SIZE)")`, streams with `while let Some(result) = messages_stream.next().await`, and `is_complete_imap_body` rejects bodies whose `RFC822.SIZE` mismatch — no unbounded `.collect()` remains.
- **TIE-BREAK: FIXED** — `for first in (1..=mailbox.exists).step_by(MAX_IMAP_BATCH_MESSAGES)` and `let uid_set = Self::imap_uid_batch(search_result);` replace the unbounded whole-mailbox fetch set with bounded batches; the excerpt contains no whole-stream `.collect()`.

### IF1 IF1-13
- A: PARTIAL — Lane writes now stop previews when `!stream_cfg.enabled`, and resolver merges group/topic disable state, but the diff does not show emitter selection enforcing the effective disabled state; a feature-enabled route can still select the orchestrator and produce final delivery behavior without proving the required non-streaming fallback.
- B: FIXED — `config_resolver.rs` now merges `block_streaming` into `streaming.enabled = false` at both group and topic levels (`if group.block_streaming == Some(true) { group_resolved.streaming.enabled = false; }` and the topic equivalent), and `StreamingMode::Disabled` also forces `enabled = false`. `lane_handle.rs` additionally short-circuits `write_chunk` when `!stream_cfg.enabled \|\| mode == Disabled`, so preview/edit writes are suppressed.
- **TIE-BREAK: ?** — 

### H1 H1-04
- A: FIXED — `handle_create` now returns `INVALID_PARAMS` on malformed `active_hours`, and the added update test verifies malformed updates return `INVALID_PARAMS` while absent, `null`, and valid values preserve the intended semantics.
- B: NOT_FIXED — The diff fixes `handle_create` (now returns INVALID_PARAMS on parse failure), but the reviewer flagged `handle_update:386` which still uses `updates.active_hours = serde_json::from_value(val.clone()).ok();` — the diff for heartbeat.rs shows no change to `handle_update`, so malformed values on update are still silently dropped.
- **TIE-BREAK: NOT_FIXED** — `updates.active_hours = serde_json::from_value(val.clone()).ok();` still converts a malformed `active_hours` value to `None`, allowing `handle_update` to return success instead of `INVALID_PARAMS`.

### GX1 GX1-07
- A: FIXED — The three synchronous `touch_device` paths are now routed through `cleanup::touch_device_last_seen`, which uses `spawn_blocking` (`tokio::task::spawn_blocking(move || store.touch_device(&device_id))`) and preserves the best-effort error semantics.
- B: PARTIAL — The `touch_device` call at line 876 is now wrapped via `cleanup::touch_device_last_seen`, and the node-connect path at line 1139 is also wrapped, but `cleanup.rs:74-83` (`run_cleanup`) still contains the synchronous `store.touch_device(&node_id)` call — the diff only adds the helper and uses it in `mod.rs`, not in `run_cleanup` itself.
- **TIE-BREAK: NOT_FIXED** — `cleanup.rs:74-83` still calls `store.touch_device(&node_id)` synchronously, so cleanup can still block the runtime worker on the mutex/SQLite write.

### IF1 IF1-02
- A: PARTIAL — The new `poll_updates` advances only after `handler.dispatch(...)`, but `OffsetTracker::for_account` creates a different key from the account’s later `advance(..., "polling")`, and `initialize("polling")` is not the account-specific initialization used by `for_account`; therefore durable per-account progress and one-time initialization are not actually connected.
- B: FIXED — `polling.rs` now uses a custom `poll_updates` loop that calls `tracker.advance(update_id, "polling").await` only *after* `handler.dispatch(...)` completes (line 175-181 of diff), and `first_start` is gated on `tracker.initialize("polling").await` (a persisted one-time fact) rather than `offset == 0`; the diagnostic `get_updates` no longer calls `advance`, so probe results are not treated as durable completion.
- **TIE-BREAK: ?** — 

### IF6 IF6-06
- A: PARTIAL — The diff adds text escaping via `let mut html = Self::escape_html(markdown);` and protects inline code, but `convert_links` is still unchanged and operates on the already globally escaped source; because `escape_html` escapes `"` to `&quot;`, a link destination containing `&quot;` is no longer the original URL, while a destination containing a quote may not be safely parsed/encoded as an attribute value. The reviewer’s requirement for a correctly escaped link destination is therefore not proven.
- B: FIXED — `let mut html = Self::escape_html(markdown);` escapes the source before any tag insertion, `extract_inline_code` wraps inline code in `<code>...</code>` placeholders so inner markup stays literal, and `convert_links` now receives pre-escaped text so `href="..."` values carry `&quot;`/`&amp;` — the test asserts `List&lt;T&gt;`, `&lt;img src=x&gt;`, `<code>**&lt;tag&gt;**</code>`, and `href="https://example.com/?a=&quot;quoted&quot;&amp;b=1"`.
- **TIE-BREAK: FIXED** — `let mut html = Self::escape_html(markdown);` (line 127) escapes `<`, `>`, `"` and `&` across the whole source *before* any tag is inserted, so plain text, inline code, and the destination string that `convert_links` subsequently matches are already entity-encoded — a quote in a URL can only reach `href="..."` as `&quot;` and cannot terminate the attribute.

### GX1 GX1-01
- A: REGRESSION — The diff cannot compile: `authenticate` contains an extra closing brace after `if let Some(manager) = shared_token { ... }` (`+    }` followed by `+    let manager = device_tokens?;`), so the function body is prematurely closed; it also does not address the first-wave requirement to use the per-request instance credential consistently beyond the manager API shown.
- B: PARTIAL — The global-validate issue is fixed (`manager.validate(token)` at the new `authenticate` helper), and the loopback flag now uses `client.local` from `resolve_client`, but the visibility predicate is now applied unconditionally (the `if visibility::visible_owner_filter().is_some()` guard was removed), which changes behavior for single-owner deployments that previously skipped the check. The core unauthenticated-read bypass is closed by the `StatusCode::UNAUTHORIZED` denial.
- **TIE-BREAK: ?** — 
