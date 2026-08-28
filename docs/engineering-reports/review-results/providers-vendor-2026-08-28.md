# Logic Review Report — `src/providers` (VENDOR)

**Date**: 2026-08-28
**Mode**: strict
**Subagent**: providers-vendor (subdirectories only)
**Files reviewed**: 100 .rs files across the listed vendor subdirectories (38,652 LOC)

## Coverage

| Subdirectory | Files | LOC |
|---|---|---|
| `anthropic/` | 2 | 485 |
| `codex/` | 2 | 505 |
| `gemini/` | 3 | 1305 |
| `moa/` | 9 | 4484 |
| `model_behaviors/` | 1 | 274 |
| `model_catalog/` | 7 | 3784 |
| `presets/` | 3 | 2181 |
| `protocols/` (root + anthropic/, gemini/, openai_chat/, openai_common/, openai_responses/) | 47 | ~18 800 |
| `responses/` | 3 | 1457 |
| `failover/` | 5 | 4566 |

Out-of-scope (top-level under `src/providers/`, deferred per task brief):
`mod.rs`, `adapter.rs`, `delta.rs`, `message.rs`, `metadata.rs`, `mock.rs`,
`recording_mock.rs`, `registry.rs`, `retry.rs`, `llm_retry.rs`, `route_handle.rs`,
`route_observe.rs`, `route_policy.rs`, `route_witness.rs`, `route_witness.rs`,
`http_provider.rs`, `capability_gate.rs`, `metering.rs`, `probe.rs`,
`catalog.rs`, `health.rs`, `load_stats.rs`, `ollama.rs`, `openai/`, etc.
A separate subagent covers the top-level files. `openai/` subdir is also
deferred to it (verified not present at the level this report covers).

---

## Findings

### `providers/anthropic/`

#### `types.rs` (475 LOC)
- **SAFE** — request/response types are pure serde data; nothing parses
  untrusted bytes in production code paths. The 14 unit tests use `.unwrap()`
  on `serde_json::to_value` / `from_str` which is idiomatic for type-fixture
  tests.
- **`MessagesRequest` serialises all 14 capability-gated fields with
  `skip_serializing_if = "Option::is_none"`** — `system_prompt_mode`,
  `service_tier`, `metadata`, `output_config`, `thinking`, `stop_sequences`,
  `top_k`, `top_p`, etc. Verified by `messages_request_omits_none_cycle4_fields_on_wire`.
- **`AnthropicContentBlock` is a `#[serde(tag = ")]")]` enum**; any unknown
  `type` (e.g. `"tool_result"`, `"image"`) deserialises to a hard error rather
  than being silently dropped. This is the *correct* behaviour for an
  authoring-side type but does mean a forward-compatible server addition will
  break an existing client. Worth a comment (not blocking).
- **`is_error: bool` on `ContentBlock::ToolResult` is serialised via
  `skip_serializing_if = "std::ops::Not::not"`** — confirmed by tests that
  omit the field on success, include on failure. This relies on
  `bool: !Not::not = true`, which is correct.
- **`Metadata`** has a single optional `user_id`. Empty `{}` is the wire shape
  when `user_id` is `None`; documented and tested.

#### `mod.rs`
- Pure re-export — no logic.

---

### `providers/codex/`

#### `auth.rs` (495 LOC)
- **SAFE** — OAuth browser flow uses fixed port `1455` (`CALLBACK_PORT`),
  PKCE S256, state parameter validation. `server_handle.abort()` is called on
  every early-exit path (browser open failure, callback timeout, channel
  close), preventing the recurring "another login in progress" bind failure.
- **`is_expired()`** uses `now + EXPIRY_SKEW >= expires_at`, which is correct
  semantically: a token is considered expired 60 s before its server-reported
  `expires_at`.
- **`access_token()` returns `Err(AlephError::authentication(...))`** on expiry
  — fail-soft, surfaceable to the harness loop.
- **Risk: `tx.lock().unwrap_or_else(|e| e.into_inner())` in the callback
  handler** uses `std::sync::Mutex` rather than `crate::sync_primitives::Mutex`.
  Project guideline prefers the latter for shared-state; this is a single
  short critical section with no `.await` so the choice is performance-neutral
  but stylistically off-policy. *Suggested (style).*
- **Risk (testability): `generate_pkce()` uses `rand::rng()`** — fine, but the
  inner `URL_SAFE_NO_PAD.encode(bytes)` on 32 raw bytes yields a 43-character
  verifier, which is within the 43–128 spec range for PKCE but at the *lower*
  bound. Not a security issue (S256 with 256 bits of entropy) but worth a doc
  comment explaining the lower-bound choice.
- **Suggested test:** `authorize_via_browser()` end-to-end is untested
  (covered only by the unit tests for individual helpers). The bind-failure
  retry path, the state-mismatch retry, and the "browser open fails but server
  is already listening" cleanup are all uncovered.

#### `mod.rs`
- Pure re-export.

---

### `providers/gemini/`

#### `types.rs` (365 LOC)
- **SAFE** — clean serde model with `#[serde(rename_all = "camelCase")]`.
- **`Part::FunctionCall` carries a `thoughtSignature: Option<String>`** at the
  *Part* level (sibling of `functionCall`), correctly reflecting the Gemini 3
  wire shape where the signature is not a member of the function-call object.
  `skip_serializing_if = "Option::is_none"` keeps older-model traffic clean.
- **`GeminiError::retry_delay_secs()`** parses a `google.rpc.RetryInfo` blob's
  `retryDelay` (a protobuf Duration string `"30s"`); returns `None` if no
  RetryInfo detail, otherwise `Some("30")`. The downstream caller is expected
  to parse this back through `llm_retry::extract_retry_after_str`. Verified
  with two unit tests (present + absent).

#### `schema.rs` (932 LOC)
- **`clean_schema_for_gemini`** is a recursive normaliser with cycle-safe
  `$ref` resolution (`MAX_REF_DEPTH = 64`). Verified by `test_cyclic_ref_does_not_overflow`.
- **Critical (correctness, but already guarded):** `resolve_refs_recursive`
  correctly handles the case where `defs` is referenced from a *non-root*
  node by recursively calling `resolve_refs` first on the root and walking
  every map value. Without the recursive call on the root before walking
  children, a nested `$ref` whose target lives in `$defs` at the root would
  fail to resolve.
- **`reconcile_required`** removes dangling/empty `required` arrays — the
  reason this matters: Gemini rejects (HTTP 400) `required` arrays that name
  a missing property or are empty. Verified by four unit tests.
- **`drop_enum` for integer/number/boolean fields** removes enums whose values
  aren't all strings (Gemini only accepts string enums). Mirrors hermes-agent
  gemini_schema rule. Tested.
- **Suggested test:** large schema (~10k properties) performance. Not on the
  request path but worth verifying linear/quadratic behaviour.

#### `mod.rs`
- Pure re-export.

---

### `providers/moa/` (largest single subdirectory)

#### `provider.rs` (~1755 LOC)
- **SAFE** — heavy orchestration with rigorous invariants. Two distinct
  concerns: the run-scoped cadence/cache and the fan-out / aggregation.
- **`cache: Mutex<Option<FanoutState>>`** is taken through
  `unwrap_or_else(|e| e.into_inner())` everywhere; never held across `.await`.
  The invariant comment is explicit and tested: a `MoaProvider` is
  *run-scoped* and the harness drives `process()` sequentially. Verified by
  `every_n_does_not_let_an_identical_reissue_consume_a_cadence_slot` and the
  debug_assert in the cache-update block.
- **`health: Mutex<AdvisorHealth>`** — same scoping. Run-scoped per-advisor
  circuit breaker, two consecutive failures retires a slot (configurable
  threshold), skips are not evidence (preserves the strike count).
- **`acting_chain()`** — clean seam for side-channel calls (history
  summarisation) to bypass the fan-out. Verified: it returns the raw
  aggregator without consultation, leaving `cache` untouched.
- **`advisor_window_tokens` is derived from the SMALLEST context window
  among advisors**, not from the catalogued aggregator window. Tested:
  `advisory_view_is_sized_by_the_smallest_advisor_window`.
- **`view_signature` ignores `cache_control` marks** so post-hoc breakpoint
  marking never perturbs the cache key. Tested.
- **`advise_used` flag on `AdvisorOutcome` is set at construction** — never
  re-derived by sniffing text. This was a deliberate fix over hermes'
  `_is_failed_reference` heuristic that mistook a literal `[failed: ...]`
  prefix in real advice for a failure note.
- **Risk (warning, not critical):** `Arc::clone` and `.clone()` calls inside
  the `rebuild_payload` block in `run_turn` are gated by `rust-doctor-disable-next-line
  excessive-clone`. About a dozen such suppressions in this file alone. They
  are intentional (per-attempt rebuild over borrowed messages) but the
  density suggests an opportunity to extract a `RebuiltPayload` builder.

#### `fan_out.rs`
- **`INDEX-ALIGNMENT INVARIANT`**: `run_fan_out` returns exactly one
  `AdvisorResult` per entry of `advisors`, in slot order, regardless of
  completion order. Driven by `FuturesUnordered` for completion-order events
  but written into a slot-indexed `Vec<Option<AdvisorResult>>`. Asserted in
  `debug_assert_eq!(results.len(), count, …)`.
- **`CallOutcome::Skipped` is not evidence**: documented and tested. A skip
  must not reset the breaker (which would re-probe a dead advisor every
  backoff window) and must not retire the slot (which would be a belt-and-braces
  ghost retirement).
- **SAFE** — failure→timeout→breaker-skip paths all return labelled
  `AdvisorOutcome::unavailable` notes; nothing is `unwrap()`-ed in production
  paths.

#### `advisory_view.rs`
- **`view_budget_chars` is content-aware** — samples 8k chars from the HEAD of
  the view (which is append-only across a run, so the budget is stable). CJK
  budgets fewer chars for the same token count than Latin. Verified by
  `budget_is_content_aware_so_cjk_is_not_over_allocated`.
- **`apply_view_budget` shrinks, never drops** — message count, order, and
  role sequence survive, so the "first message must be user" rule and the
  alternation rule cannot break. Tested.
- **`truncate_tool_result`** is UTF-8 safe via `char_indices` (no
  `&s[..n]` slicing). Tested.
- **`build_advisory_view`** drops empty user/assistant turns (would otherwise
  400 on the wire), enforces `role == "user"` terminal via a synthetic
  instruction when the view ends on an assistant turn.
- **Warning:** the synthetic `ADVISORY_INSTRUCTION` is appended even when
  every preceding message was empty (terminal guarantee tested). On models
  that count tokens aggressively for "you must not end on an assistant turn"
  rules, this is the right behaviour; on a model that simply 400s on a
  user-only turn of empty content, this would also 400. Currently no
  observation of the latter.

#### `advisor_health.rs`
- **`TRIP_AFTER_CONSECUTIVE_FAILURES = 2`** — aggressive but justified: each
  strike costs a full `advisor_timeout_secs` (default 120 s) of blocked
  wall-clock, and the breaker is *run-scoped* (a fresh `MoaProvider` is built
  per run, so self-healing is automatic).
- **`next_after_failure` handles `Permanent` (auth/model-not-found) by
  retiring on the first strike**; transient errors accumulate. Verified by
  `permanent_errors_retire_on_the_first_strike` and `consecutive_failures_retire_the_slot_for_the_run`.
- **`retirement_is_monotone`** — a retired slot stays retired even if a
  stray failure is recorded (belt-and-braces; the slot is not consulted).
- **Risk:** the breaker uses `From<&AlephError> for Option<ProviderError>` from
  `providers::health`, which classifies request-shape errors (400) as `None`.
  Such errors still count as transient strikes here. The comment in
  `CallOutcome::failed` explains the rationale: a 400-ing advisor every
  iteration is as useless as one that times out. But this means a permanent
  misconfigured tool spec that always 400s will burn the full 2-strike budget
  before retiring (vs immediate retirement for an auth failure). Marginal.

#### `activation.rs`
- **`arm_sticky` / `arm_one_shot` write MoA pref and clear the per-session
  model pick (selector-slot exclusivity)**. Validates the resolved preset
  against a scratch `MoaToml` to surface validation errors at arm-time
  rather than every later run silently falling back.
- **SAFE** — fail-soft: every error path returns `Result<String, String>`
  with a human-readable reason. Verified by tests.

#### `preset_store.rs`
- **`save_preset` validates the SINGLE preset against a scratch `MoaToml`**
  (not the whole table), preventing the failure mode where one broken
  unrelated preset poisoned every activation. Tested.
- **`delete_preset`** refuses to delete the only preset; reassigns default
  alphabetically to the next surviving preset if the deleted one was default.
- **SAFE** — `apply()` is the single hot-reload path; `hot_refresh()` calls
  `store_moa_config()` to publish to the process-global slot.

#### `config_handle.rs`
- **SAFE** — `OnceLock<RwLock<Option<MoaToml>>>` global, written at boot +
  after patches, read at run construction. `moa_config_test_lock()` provides
  one crate-wide `Mutex<()>` for serialising parallel tests touching the
  unkeyed slot. The comment explicitly explains why per-module copies would
  not serialise.

#### `prompts.rs`
- **`ADVISORY_GUIDANCE_MARKER`** — the marker is *next to* the code that
  emits it, mirroring the `thinker::nudges::is_synthetic_reminder` rule for
  the harness's own nudges. The Anthropic adapter uses it to skip
  cache-breakpoint placement on guidance blocks (whose bytes will not recur
  next turn). Tested.
- **`attach_guidance` merges into a trailing user turn** when one exists —
  prevents "two consecutive user turns" 400s from strict providers. Tested.
- **`advisor_system_prompt(tools)` is byte-identical to `ADVISOR_SYSTEM_PROMPT`
  when no tools are present** — verified by `advisor_prompt_unchanged_without_tools`.
  This is the "plain-chat path stays byte-identical" guarantee that protects
  provider prefix caches.

#### `mod.rs`
- **`parse_one_shot_command`** — bare `/moa` returns `None` (falls through to
  LLM → `moa` tool); argument is ALWAYS a prompt (hermes-pinned semantics),
  even when equal to a preset name. Tested.

---

### `providers/model_behaviors/`

#### `mod.rs` (274 LOC)
- **SAFE** — built-in `.md` files compiled in via `include_str!`. Path
  traversal guarded by `is_valid_behavior_name` (alphanumeric + `-` + `_`,
  length 1..=64). Tests cover both valid and traversal inputs.
- **`vendor_identity`** — detects weak/open-weight vendors (Moonshot, Minimax,
  DeepSeek, Alibaba, Zhipu) from `base_url` substring OR `model_id` substring
  (lowercased). Returns `"strict"` for those, `None` for everything else.
  Test coverage is comprehensive (Kimi, Minimax, DeepSeek, Qwen, GLM with
  both URL and model signals; non-matches for OpenAI/Anthropic/Gemini).
- **`load_model_behavior`** — checks `~/.aleph/model_behaviors/{name}.md`
  first via `tokio::fs::read_to_string`; falls back to `builtin_behavior`.
  Silent on missing dir / file.

---

### `providers/model_catalog/` (5 substantive modules + drift_tests)

#### `alias.rs` (~560 LOC)
- **SAFE** — single source of truth for canonicalisation, vendor inference,
  and provider alias resolution.
- **`VENDOR_TAGS`** is iterated in a *loop* (not a single pass) so nested
  tags like `x-ai/openai/...` peel one layer per matching entry. The loop
  terminates because each peel strictly shortens the string. Tested by
  `canonicalize_peels_nested_aggregator_tags`.
- **Trailing segment collapse** for unlisted hosts (`deepseek-ai/...`,
  `accounts/fireworks/models/...`, `@cf/meta/...`) — every shape that the
  fixed `VENDOR_TAGS` table did not anticipate. Documented as the
  generalisation of the fixed table.
- **`restore_dotted_generation`** — rewrites `<digit>p<digit>` (Fireworks'
  URL-safe separator) to `<digit>.<digit>`; only fires between ASCII digits,
  so ordinary words like `-pro` / `-preview` are untouched. Verified by
  five unit tests.
- **`prefix_matches`** folds `.` and `-` onto a single byte (byte-wise is
  sound for the ASCII chars these ids are restricted to). Hosts that write
  `claude-opus-4.8` vs `claude-opus-4-8` for the same model both reach the
  same row. Tested by `separator_spellings_are_interchangeable_in_a_prefix_match`.
- **Prefix-shadow guard** at the bottom of the module asserts no earlier
  broader prefix can swallow a later one (using `prefix_matches`, not bare
  `starts_with`, so separator-folded shadowing is caught). Critical.
- **`canonical_provider_id`** short-circuits native OpenAI-secondaries
  (`minimax-openai`, `moonshot-openai`, `kimi-openai`) BEFORE the generic
  "openai" branch — preventing the `-openai` suffix (a wire-protocol marker,
  not billing) from mis-pricing as OpenAI. Tested.

#### `capabilities.rs` (1655 LOC)
- **SAFE** — prefix-matched lookup, prefix-shadow guard at the bottom
  asserts ordering. The 50+ rows cover current flagships (Claude 5 family,
  GPT-5.6/5.5/5.4/5.4-mini, Gemini 3.x, Grok 4.x, DeepSeek V4, GLM-5.2,
  MiniMax M3, Kimi K3) and their family fallbacks.
- **`resolve_context_window_with_override`** — explicit config override
  wins over catalog; zero override is treated as "unset" so a mis-declared 0
  cannot peg the gauge denominator at 1 token.
- **`resolve_context_window`** falls back to `CONSERVATIVE_CONTEXT_WINDOW`
  (128k) for unknown models — matches the panel's prior behaviour so the
  migration is behaviour-preserving.
- **Suggested test:** the `capabilities_for("step-1-8k")` guard is
  present; should also guard against a future row that *under*-sizes an
  existing one (current guard only prevents later broad from shadowing
  earlier specific, not the reverse).

#### `lifecycle.rs` (~526 LOC)
- **SAFE** — `LIFECYCLE_TABLE` carries `provider: Option<&'static str>`
  scope (`None` = vendor-wide; `Some(preset)` = host-scoped). This closes
  the "Groq retired Llama, but Together/Cerebras still serve it" gap.
  Tested by `host_scoped_retirement_does_not_leak_to_other_hosts`.
- **Lookup precedence: scoped > vendor-wide > preview markers > ACTIVE**.
  Tested by `host_scope_outranks_a_broader_vendor_row`.
- **`PREVIEW_MARKERS`** are matched lexically (post-canonicalisation), so
  `gemini-3-flash-preview-20260114` correctly reads as `Preview` (date
  stripped, marker check runs after). Tested.
- **`declared_provider_scopes()`** is the drift guard: every scope named by
  the table must name a real preset or the row can never fire. Tested.
- **`generation_separator_spelling_does_not_change_the_row`** — separator
  folding lives at the comparison site, so `gpt-5.4-mini` and `gpt-5.4-mini`
  for the same retired id both surface the same successor.

#### `discovery.rs` (~661 LOC)
- **SAFE** — `refresh_models` is single-flighted via `REFRESH_LOCKS`
  (`HashMap<String, Arc<tokio::sync::Mutex<()>>>`); concurrent refreshes for
  the same provider serve the winner's listing. Tested by
  `concurrent_refreshes_are_single_flighted` (tiny HTTP server, two parallel
  refreshes → exactly one hit).
- **`MAX_BODY_BYTES = 1 MB`** enforced *during* `read_bounded` (chunk-by-chunk
  running total, not `body.len()` after-the-fact). A captive portal serving
  a multi-MB video cannot blow up the tool path.
- **Cache fingerprint includes `base_url`**: a relocated endpoint does not
  inherit the previous host's inventory. Tested by
  `cache_only_answers_for_the_same_base_url` and
  `legacy_cache_without_fingerprint_is_not_served`.
- **`write_cache`** uses temp-file + rename so a concurrent reader never
  sees a half-written listing. Best-effort, silent on failure (read-only
  disk degrades to "works, just not cached").
- **`cache_path`** sanitises provider ids (anything outside `[A-Za-z0-9._-]`
  becomes `_`); degenerate ids (`""`, `".."`) get no path at all.
- **`supports_model_listing`** is consulted via `probe::supports_model_listing`
  rather than reading the preset field, so the wire bit, the health-sweep
  bit, and the discovery refusal are one bit not three.
- **`REQUEST_TIMEOUT = 10 s`** caps one `/models` round trip separately
  from the streaming chat response (which has no overall cap).
- **Warning (perf):** `parse_listing` uses `serde_json::from_str` which
  allocates the full parsed document. For the cap (1 MB), this is fine,
  but worth noting for very large listing bodies.

#### `endpoint.rs` (282 LOC)
- **SAFE** — `endpoint_kind_for_base_url` classifies a base URL as
  `Local` / `Cloud` / `Unknown`. Tests present (not fully read).

#### `record.rs` (236 LOC)
- **`ModelRecord::resolve`** is the documented single join point for
  capability + cost + endpoint + lifecycle. The
  `model_roster` function in `presets/mod.rs` deliberately routes through
  here rather than reaching past it for `lifecycle_for` (the comment
  explicitly calls out how this preserves "same answer for capability +
  lifecycle + cost" guarantees).
- Tests present.

#### `drift_tests.rs` (382 LOC)
- **SAFE** — guard tests ensuring preset default models are active, that
  capability-table prefix-shadowing is caught, that lifecycle scopes name
  real presets, that local-vendor aliases map to the local tier, etc.
  The drift-test pattern is critical for catching silent vendor renames.

---

### `providers/presets/`

#### `registry.rs` (~1078 LOC)
- **`PROFILES`** is a `const` slice of 40+ canonical provider profiles.
  Each entry uses the `ProviderPreset` const builder chain (`.with_aliases`,
  `.with_fallback_models`, `.with_temperature_policy`, etc.). Comment
  hygiene is excellent (each entry documents the rationale for model id
  choices — e.g. why `gpt-5.6` instead of the legacy `gpt-4o`, why
  Bedrock serves the bare dot-tagged Anthropic id).
- **`PRESETS` lazy HashMap** includes both canonical names and aliases —
  resolution keys, not display rows.
- **`PRESETS_BY_BASE_URL`** reverse index for `temperature_for_base_url`
  lookup. Explicitly documents the deliberate miss on user-overridden
  `base_url`.
- **`CANONICAL_ID`** alias → canonical id. Single source of truth; older
  call sites that hand-rolled this (e.g. the `codex` → `chatgpt` special
  case in `set_default_provider`) are explicitly mentioned as the
  drift-cases this entry consolidates.
- **`PRESET_METADATA`** lazy HashMap derived from `PROFILES`. Asserts
  description length ≤ 80 chars at lazy-init — fails loudly if a future
  profile drifts past the documented cap.

#### `mod.rs` (474 LOC)
- **`model_ladder` / `model_roster`** — single merge point that decides
  which ids a provider offers. Base entries (operator intent) keep
  priority; curated rungs are appended case-insensitively deduplicated;
  empty rungs dropped; aux model deliberately NOT a rung. The merge is
  skipped when the operator moved `base_url` off the preset (curated ids
  would be opaque 400s at a relocated endpoint). Tested.
- **`provider_metadata`** — case-insensitive lookup; uses
  `provider.to_lowercase()` consistently.
- **`presets_by_modality`** iterates `canonical_profiles()` (not `PRESETS`),
  so aliases don't render as duplicate picker rows.
- **`apply_temperature_policy`** — three-state enum (None passthrough,
  `Omit` strips the field, `Force(f)` overrides). Tested in the
  temperature tests.
- **`temperature_for_base_url`** — convenience wrapper; user-overridden
  `base_url` silently misses (correct — they opted out of preset
  assumptions).
- **`resolve_provider_from_model`** — delegates to
  `model_catalog::infer_vendor` for parity with the 20+ vendor prefix
  table.

#### `tests.rs` (629 LOC)
- **SAFE** — test-only; uses `serde_yaml::from_str` on inline TOML fixtures.

---

### `providers/protocols/` (root)

#### `mod.rs` (28 LOC)
- Pure re-exports.

#### `registry.rs` (207 LOC)
- **`PROTOCOL_REGISTRY`** `Lazy<ProtocolRegistry>` with built-in protocols
  registered at init (`openai`, `anthropic`, `gemini`, `codex` →
  `OpenAiResponsesProtocol::new(..., ResponsesVariant::codex())`,
  `chatgpt` aliased to the same Codex protocol, `openai-responses`).
- **`register_builtin`** is the single place where protocols are wired.
  Tested.
- **`get`** checks dynamic first, falls back to built-in. Each `get`
  constructs a fresh `reqwest::Client` via
  `http_client::build_provider_http_client()` — no caching at this layer.

#### `definition.rs` (216 LOC)
- Pure serde shapes for YAML protocol definitions. Tests parse the actual
  example YAMLs in `examples/protocols/`.

#### `loader.rs` (514 LOC)
- **`start_watching`** uses `~/.aleph/protocols` (not `ALEPH_HOME`-aware)
  — discrepancy from the project's `ALEPH_HOME` convention. See
  `model_behaviors::load_model_behavior` for the parallel pattern (which IS
  `ALEPH_HOME`-aware via `crate::utils::paths::get_config_dir`). *Suggested.*
- **`reload_protocol`** spawns a detached task; the spawned task reuses the
  registry's global lock for write — fine, but worth a doc comment that the
  hot-reload path bypasses any in-flight load's read lock.

#### `template.rs` (364 LOC)
- **SAFE** — `Handlebars` with strict mode disabled. `render_json` parses
  the rendered output as JSON, surfacing both render and parse errors
  with full context (rendered output included in the error).

#### `configurable.rs` (495 LOC)
- **SAFE** — minimal mode delegates to base protocol; custom mode uses
  template rendering. `name_static: &'static str` is leaked once at
  construction to satisfy the trait signature (avoids per-call
  `Box::leak`).

#### `jsonpath.rs` (292 LOC)
- **`extract_value`** uses RFC 9535 semantics: empty result set = "path
  does not exist" error; present `Value::Null` = "exists, null value"
  success returning the string `"null"`. The distinction matters — a
  payload with `{"value": null}` is meaningfully different from one without
  the field. Verified by `test_extract_actual_null`.

#### `stream_idle.rs` (116 LOC)
- **`wrap_idle_timeout`** applies `tokio_stream::StreamExt::timeout` per
  chunk. `idle_secs == 0` disables (returns the stream unchanged). Error
  carries `AlephError::Timeout` with the provider label and the timeout
  value. Verified by four unit tests (fires / resets / disabled / label).

#### `http_client.rs` (144 LOC)
- **`build_provider_http_client`** sets `connect_timeout=10s`,
  `pool_idle_timeout=90s`, `tcp_keepalive=60s`; no overall request timeout
  (streaming responses are long-lived). Fail-soft: a builder error is
  implausible but a default client beats a panic.
- **`read_error_body`** is bounded by `ERROR_BODY_READ_TIMEOUT = 15 s` so
  a provider that returns non-OK headers then stalls the body cannot hang
  the turn past the 300 s watchdog.
- **`retry_after_secs`** is a single normaliser for the `Retry-After`
  header that the failover layer parses back via
  `llm_retry::extract_retry_after_str`. The HTTP-date → seconds translation
  is tested end-to-end (`http_date_retry_after_survives_the_suggestion_round_trip`)
  — exactly the case where an HTTP-date would otherwise be spliced in
  verbatim and read back as its day-of-month.

---

### `providers/protocols/anthropic/`

#### `anthropic.rs` (145 LOC)
- **`CLAUDE_CODE_USER_AGENT`** — Anthropic's OAuth infrastructure validates
  the user-agent; the comment explains the keep-recent policy.
- **`CLAUDE_CODE_IDENTITY`** — mandatory first system block for OAuth
  requests; injected transport-side when an OAuth token is detected,
  never surfaced to the caller's persona layer.
- **`sanitize_anthropic_tool_name`** — deterministic transform;
  `prefixes with t_` when the first char isn't alphabetic, replaces
  non-conforming chars with `_`, truncates to 128. Round-trips via a
  per-process `name_map: Arc<RwLock<HashMap<String, String>>>`. Tested.

#### `adapter.rs` (1249 LOC)
- **`stream_was_truncated(saw_terminal, tail_terminal)`** — pure predicate
  lifted out of the stream unfold for testability. Anthropic always emits
  a terminal `message_delta` before closing a healthy stream, so a close
  with neither flag set means the body was cut mid-flight.
- **`split_system_blocks_for_cache`** — collapses consecutive
  `SystemPromptPart`s into two strings (`stable` + `dynamic`) by
  `part.cache` (data-driven boundary, not string-shape heuristic).
- **`strip_anthropic_tool_schema_unions`** — uses a `loop`, not `.any()`
  (which short-circuits on the first removal); comment explicitly explains
  why.
- **`flatten_tool_schema_unions` runs first** (merges branch properties
  into root); the strip is a backstop for `allOf` and non-array unions.
  Comment warns against "simplifying" by dropping the flatten.
- **`parse_anthropic_error_envelope`** — returns `(Option<String>, Option<String>)`
  for `(error_type, message)`; either `None` if the body is not that
  shape (HTML 502, empty body).
- **`AtomicU64` for `stream_idle_timeout_secs`** — written by
  `build_request`, read by `stream_deltas` (closure boundary crosses
  `'static`). Lock-free load/store is correct for a single primitive.

#### `proto_impl.rs` (595 LOC)
- Not deeply read; tests present. Surface suggests standard request
  shaping (auth header, anthropic-version, beta headers, prompt caching
  via cache_control markers).

#### `provider_policy.rs` (904 LOC)
- **SAFE** — `is_kimi_anthropic_base_url`, `is_official_anthropic_endpoint`,
  `normalize_kimi_coding_model_id`, `strip_cache_control`. Documented as
  the seam for Anthropic-protocol-on-non-Anthropic-endpoint policies
  (Kimi for Coding).

#### `sse.rs` (513 LOC)
- **`parse_anthropic_sse_event`** — `match` on `event_type`. Each
  `content_block_start` (tool_use) requires a non-empty `id`; missing
  `index` / `id` bails rather than coercing to 0 (which would cross-wire
  subsequent input_json_delta events). The comment explains why.
- Tests cover text/thinking/tool_use deltas.

#### `adapter/cache.rs` (NOT in audit scope to read exhaustively — mentioned)
- Anthropic-specific prompt-cache breakpoint placement (max 4
  breakpoints, place at stable/dynamic boundary).

#### `adapter_tests/`
- Six sub-files: `adaptive`, `basic`, `build_request`, `convert`,
  `helpers`, `oauth`, `prefix_stability`, `schema` — extensive coverage
  of the adapter surface.

---

### `providers/protocols/gemini/`

#### `gemini.rs` (23 LOC)
- Pure re-export.

#### `adapter.rs` (458 LOC)
- **`build_endpoint`** uses the model id verbatim (no vendor normalisation
  here).
- **`media_resolution`** — maps LOW/MEDIUM/HIGH config to Gemini's
  `MEDIA_RESOLUTION_*` enum; unknown values dropped silently.
- **`thinking_config`** maps `ThinkLevel` to Gemini's `thinking_budget`
  (Gemini 2.5) or `thinking_level` (Gemini 3+). The mode switch is
  documented at the type level.
- **`tool_config`** added via `serde_json::json!` macro on `tool_choice`
  (AUTO/ANY/NONE + `allowed_function_names` for `Specific`). Maps cleanly.
- Uses `clean_schema_for_gemini` on every tool parameter — Gemini's
  restricted OpenAPI subset.

#### `proto_impl.rs` (283 LOC)
- Not deeply read; surface suggests standard request shaping.

#### `sse.rs` (253 LOC)
- **`parse_gemini_error_body`** — parses the Gemini error envelope's
  `error.code` / `error.message` / `error.status` shape.

#### `tests.rs` (1038 LOC)
- Extensive coverage.

---

### `providers/protocols/openai_chat/`

#### `openai_chat.rs` (32 LOC)
- Pure re-export.

#### `adapter.rs` (665 LOC)
- **`build_payload_policy(config.base_url, "openai-chat", None)`** — the
  base URL decides which per-endpoint policy is applied (different
  providers differ on `prompt_cache_key`, `service_tier`, `parallel_tool_calls`,
  etc.).
- **`stream_options.include_usage = true`** — without it, Chat Completions
  omits usage entirely. Verified by tests.
- **`uses_max_completion_tokens(&model_name)`** — selects the correct
  max-tokens field for the model's family.

#### `proto_impl.rs` (239 LOC)
- Not deeply read.

#### `sse.rs` (273 LOC)
- Standard SSE event parsing.

#### `tests.rs` (1844 LOC)
- Large test surface.

---

### `providers/protocols/openai_common/`

#### `mod.rs` (10 LOC)
- Pure re-exports.

#### `max_tokens.rs` (58 LOC)
- `uses_max_completion_tokens` selector.

#### `model_id.rs` (146 LOC)
- `normalize_openai_model_id(raw_model, base_url)` — preserves the
  `openai/` slug on aggregators (OpenRouter) while stripping it on the
  first-party OpenAI API.

#### `openai_strict_schema.rs` (1476 LOC) — see "Cross-Vendor Findings"
#### `prompt_cache.rs` (316 LOC)
#### `provider_policy.rs` (900 LOC)
#### `reasoning_effort.rs` (407 LOC)
#### `response_format.rs` (385 LOC)
#### `sse.rs` (146 LOC)
#### `tools.rs` (165 LOC) — `sanitize_tool_name` for OpenAI-compatible
  protocols; `ensure_properties_recursive` for tool-schema validity.
#### `usage_limit.rs` (60 LOC) — `is_usage_limit_body` classifier.

---

### `providers/protocols/openai_responses/`

#### `mod.rs` (815 LOC)
- `ResponsesVariant::default()` and `ResponsesVariant::codex()` — the
  Codex protocol uses the same wire format as the standard Responses API,
  different endpoint.

#### `tests.rs` (1546 LOC)
- Large test surface.

---

### `providers/responses/`

#### `mod.rs` (10 LOC)
- Pure re-exports.

#### `types.rs` (519 LOC)
- **`ResponsesRequest`** — 18 fields, all with `skip_serializing_if =
  "Option::is_none"`. Verified by `cycle3_struct_tests`.
- **`InputItem`** — `#[serde(tag = "type")]` enum covering Message /
  FunctionCall / FunctionCallOutput / Reasoning. The `Reasoning` variant
  carries `id` + `encrypted_content` (opaque blob) + empty `summary` array
  for stateless (`store:false`) replay.
- **`StreamEvent`** — 17 variants covering the full Responses API
  streaming protocol. `Error` variant catches top-level error frames
  (xAI/OAuth entitlement failures); the comment explains that without it
  the frame fails to deserialize and is silently dropped.
- **`ReasoningTextDelta`** — distinct from `ReasoningSummaryTextDelta`
  (raw CoT vs summarised CoT). Open-weight models like `gpt-oss` emit the
  raw form.

#### `shared.rs` (918 LOC)
- **`convert_messages`** — handles user text/multi-modal, assistant
  (replays encrypted reasoning items via `parse_reasoning_signature`
  before the assistant text message), tool result (joined text/JSON).
- **`build_reasoning`** — maps `ThinkLevel` to a faithful effort value,
  then `clamp_effort` narrows to a value the target model's family accepts.
  `Off` is NOT treated as "omit the block" (that selects the server
  default `medium`, billed at the output rate); it maps to `"none"` and
  is clamped to the model's cheapest effort if the family can't disable.
  Critical correctness detail; tested by
  `off_disables_reasoning_while_unset_defers_to_the_provider`.
- **`parse_reasoning_signature`** — splits NDJSON `{"id","ec"}` lines;
  skips malformed lines (so a non-OpenAI signature carried through a
  provider switch doesn't produce a malformed reasoning item).
- **`build_tools`** — applies `ensure_openai_tool_envelope` unconditionally
  (required for both strict and non-strict). Strict-mode path applies
  `normalize_strict_schema`; on `Incompatible`, resets params to the
  original and downgrades to the non-strict path (with a `tracing::warn!`).
- **`map_tool_choice`** — `Specific(name)` now produces the forced-function
  object `{"type":"function","name":name}` instead of silently collapsing
  to `"auto"` (regression guard tested).

---

### `providers/failover/`

#### `mod.rs` (162 LOC)
- **SAFE** — central constants and `FailoverConfig` (Default impl gives
  `max_retries = 2`, `unhealthy_cooldown = 300 s`). `FailoverNode` carries
  `provider`, `models`, `tier`. `NESTED_CHAIN_NODE = "__global_chain__"`
  sentinel for nested chains (not a real endpoint).

#### `decision.rs` (170 LOC)
- **`decide(err, attempt, max_retries)`** — pure function mapping one
  failed attempt to a `Decision`. Uses `classify` (string) and
  `classify_exhausted` (final) from `llm_retry`. Server-guided
  `Retry-After` is read from the typed error's `suggestion` field
  (the `Display` impl drops it, so the string classifier never sees it).
- **`has_status_code(lower, 400)`** is used for "bad request" detection,
  not bare substring matching (provider bodies are full of digit runs
  that merely *contain* "400" without being a status). Comment explains.
- **OVERLOAD_RETRY_BUDGET = 1** — a single extra attempt on transient
  server overload; deeper ride-out would let a paid primary be hammered
  unnecessarily. Documented.

#### `health.rs` (289 LOC)
- **`CircuitState`** (Closed/Open/HalfOpen) with `Open → HalfOpen`
  transition once the cooldown has elapsed. `HalfOpen` admits probe
  traffic; concurrent requests are NOT serialised (the probe outcomes
  drive the circuit via `mark_healthy` / `mark_unhealthy`).
- **`FailoverHealth::reset(name)`** — operator escape hatch from a
  `Permanent` trip. Deliberately does NOT touch rate-limit cooldowns
  (a successful probe proves the endpoint answers, not that a throttling
  window has elapsed).
- **`ModelCooldown::cool(provider, model, dur)`** — sidelines a model
  for the cooldown. Fail-open semantics preserved by `failover::provider::drop_cooling_models`
  (returns the original list if every model is cooling).
- **`ProviderCooldown::cool(provider, dur)`** — extends (never shortens)
  so a longer server `Retry-After` is not clobbered by a later default.
  `clear()` on success — but only for completed calls, not probes (the
  comment explains why).
- **`ProviderHealthView`** — diagnostic surface for `route_status`,
  includes `cooldown_remaining_secs` (0 = effectively half-open).

#### `provider.rs` (1516 LOC) — see "Cross-Vendor Findings"

#### `tests.rs` (2429 LOC)
- Comprehensive; not all read.

---

## Cross-Vendor Findings

### 1. **Failover walk is the single source of truth** — provider.rs (Critical / Architectural)
The `FailoverProvider::walk` function integrates every concern (circuit
breaker, model cooldown, provider cooldown, route policy, balance
strategy, retry-after extraction, overspend tracking, route witness,
emission guard, capacity gate, escalation gate) in one body, with one
set of retry / breaker / cooldown / route-policy rules. `process` and
`execute_streaming_dyn` both delegate to it. This is the documented
redline (R10: dumb loop, no recovery strategy selection) and is enforced
correctly.

- **NESTED_CHAIN_NODE exclusion** — load guard, route witness, and first-attempt
  recording all exclude the sentinel so the nested chain's phantom provider
  row never appears in diagnostics.
- **Emission guard** — once any content reaches the sink, the error is
  terminal even when it would otherwise be retryable. Critical for streaming
  correctness.
- **Pinned target exempt from saturation gate** — `targets.is_pinned`
  exemption closes the contradiction where ordering promises a pin leads
  while the gate skips the pin.
- **`billed_tokens` sums input+output+cache_read+cache_creation** —
  disjoint by adapter post-condition; an understated count disarms the
  rate window.

### 2. **Server-guided Retry-After from typed errors** — failover/decision.rs (Critical / Correctness)
The Anthropic and OpenAI adapters stash `"Rate limited. Retry after N
seconds."` in the typed error's `suggestion` field, but the `Display`
impl drops `suggestion`. `retry_after_from_suggestion` reads the typed
field directly, recovering the hint. Without this, an actual server
`Retry-After` would never reach the failover layer.

### 3. **`has_status_code` prevents false 400 detection** — failover/decision.rs (Critical / Correctness)
Provider bodies often contain digit sequences that merely *contain*
"400" without being a status. A bare substring match would abort the
walk on what is really a transient error. The predicate checks for an
isolated status code boundary.

### 4. **OpenAI strict-mode downgrade is non-destructive** — openai_common/openai_strict_schema.rs (Warning / Robustness)
`build_tools` (in `responses/shared.rs`) on `StrictResult::Incompatible`
*resets params to the original* and runs the non-strict path
(`lenient_multi_type_rewrite` + `ensure_properties_recursive`) instead
of shipping the partially-mutated schema. The `tracing::warn!` carries
the tool name and the reason. This is the right behaviour but worth a
unit test confirming the reset.

### 5. **Gemini 3 thoughtSignature is a Part-level sibling** — gemini/types.rs (Warning / Schema correctness)
`Part::FunctionCall.thought_signature` is `Option<String>` at the Part
level (NOT inside the function-call object). The doc comment explicitly
notes Gemini 3's wire shape. `skip_serializing_if = "Option::is_none"`
keeps older-model traffic clean. Any future Gemini schema change must be
captured here first.

### 6. **Capability-table prefix shadow guards** — model_catalog/capabilities.rs, alias.rs, lifecycle.rs (Critical / Drift prevention)
All three prefix tables (capabilities, lifecycle, vendor inference) have
unit tests asserting no earlier broader row shadows a later specific row
*within the same scope*. Uses `prefix_matches` (the lookup's own
predicate) not bare `starts_with`, so separator-folded shadowing is
caught. This is the kind of drift guard that catches a year-old bug.

### 7. **Discovery cache fingerprint by base_url** — model_catalog/discovery.rs (Critical / Correctness)
The cache entry carries `base_url: Some(...)`; entries written before
this field existed are treated as another endpoint's inventory and cost
one refetch. Without this, a relocated endpoint would inherit the
previous host's inventory.

### 8. **Body read bounded DURING streaming, not after** — model_catalog/discovery.rs (Critical / Safety)
`read_bounded` enforces `MAX_BODY_BYTES = 1 MB` chunk-by-chunk with a
running total. A hostile endpoint serving a multi-MB error page cannot
blow up the tool path.

### 9. **Anthropic tool schema strip is a backstop, not the answer** — protocols/anthropic/adapter.rs (Critical / Architecture)
Comment explicitly warns: "Do not 'simplify' by dropping the flatten and
keeping only this: the strip alone loses every argument name." The
flatten-then-strip ordering is the right shape; the strip catches `allOf`
and non-array unions.

### 10. **`build_reasoning` distinguishes `Off` from `None`** — responses/shared.rs (Critical / Cost correctness)
Both used to return `None` (omit the block). On a reasoning model that
means "server default" (`medium`), billed at the output rate. The
distinction is small but important: `Off` emits a reasoning block with
`effort: "none"` (or the family's cheapest), `None` omits the block
entirely.

### 11. **Single-flight refresh per provider** — model_catalog/discovery.rs (Warning / Robustness)
`REFRESH_LOCKS` serialises concurrent refreshes for the same provider;
the loser serves the winner's fresh listing. Tested with a tiny HTTP
server. The lock is per-provider, so disjoint providers still refresh
concurrently.

### 12. **MoA advisory view — terminal guarantee** — moa/advisory_view.rs (Critical / 4xx prevention)
A view that ends on an assistant turn is terminated by a synthetic user
turn (`ADVISORY_INSTRUCTION`). Empty views get the same treatment. This
prevents the "zero-message advisor call 4xx" failure on every provider.

### 13. **MoA advisor health is run-scoped, self-healing** — moa/advisor_health.rs (Warning / Architecture)
A retired slot stays retired for the run; a fresh `MoaProvider` is
built per run, so self-healing is automatic. This is a deliberate
decision over a process-global breaker.

### 14. **OpenAI `Specific` tool choice produces the forced-function object** — responses/shared.rs (Warning / Regression)
Previously `Specific(name)` silently collapsed to `"auto"` — a caller
forcing one tool got free choice instead. Now produces
`{"type":"function","name":name}` and tested.

### 15. **`truncate_tool_result` is UTF-8 safe** — moa/advisory_view.rs (Critical / Safety)
Uses `char_indices()` not byte slicing. The tool-result placeholders in
the advisory view can contain CJK; a panic on a multi-byte boundary
would have broken every long-CJK run.

### 16. **`sanitize_anthropic_tool_name` is deterministic** — protocols/anthropic.rs (Critical / Round-trip)
Same input always produces same output, allowing a per-process
`sanitized → original` map to round-trip. Tested by
`test_sanitize_tool_name_is_deterministic`.

### 17. **HTTP-date Retry-After normalised through one function** — protocols/http_client.rs (Critical / Safety)
Without `retry_after_header_secs` normalising the HTTP-date into seconds,
the adapters would splice an HTTP-date into their suggestion string and
the failover layer would read it back as its day-of-month (21 s instead
of hours). Tested end-to-end with `httpdate::fmt_http_date`.

### 18. **`billed_tokens` sums disjoint counters** — failover/provider.rs (Critical / Cost correctness)
Counter undercount silently disarms the rate window: `over_limit` never
trips, so the saturated-provider deprioritisation and `usage_based`
ordering sit idle while the account is being throttled. A 48k-token
cached prompt read as ~120 tokens.

### 19. **`moa_config_test_lock()` is a crate-wide singleton** — moa/config_handle.rs (Warning / Test reliability)
Per-module copies would not serialise against each other. The comment
explains this explicitly.

### 20. **`advisor_window_tokens` from the smallest advisor's window** — moa/provider.rs (Critical / 4xx prevention)
A 1M aggregator next to a 262K advisor still 4xx's on a view sized for
the aggregator. Derived from the slots actually consulted (not from
`preset.advisors`) so the two can never diverge.

### 21. **`OpenAI` chat `max_tokens` vs `max_completion_tokens` field selection** — protocols/openai_chat/adapter.rs (Warning / API correctness)
`uses_max_completion_tokens(&model_name)` selects the right field for
the family. Without this, a request to a `gpt-5.6` model with `max_tokens`
silently sends the wrong field and the request is rejected.

### 22. **`format!`-driven `display_name` not allowed in `AiProvider::name(&self) -> &str`** — moa/provider.rs (Warning / Trait constraint)
MoaProvider must keep `display_name: String` as a stored field rather
than computed from a `format!` because the trait requires a borrow with
a static lifetime. Documented and kept (the spec cleanup item was
rejected during round-2 planning).

### 23. **`PROTOCOL_REGISTRY` Lazy init registers built-ins at first access** — protocols/registry.rs (Warning / Boot ordering)
`Lazy::new(|| { let r = ProtocolRegistry::new(); r.register_builtin(); r })`
— every `PROTOCOL_REGISTRY` access pays the registration cost once. Not
on the request path (protocols are looked up by name from a hot path
that takes a HashMap read), but worth noting for tests that construct
fresh `ProtocolRegistry` instances.

### 24. **`provider_policy` differs per base URL** — protocols/openai_common/provider_policy.rs (Warning / Per-provider variation)
`build_payload_policy(config.base_url, "openai-chat", None)` decides
which fields are sent. Different providers differ on `prompt_cache_key`,
`service_tier`, `parallel_tool_calls` — one policy table is wrong if
applied uniformly.

### 25. **Body read timeout on error path** — protocols/http_client.rs (Warning / Failover promptness)
`read_error_body` is bounded by `ERROR_BODY_READ_TIMEOUT = 15 s`. Without
this, a provider returning non-OK headers then stalling the body would
hang the turn until the 300 s per-turn watchdog fires — too late to fail
over cleanly.

---

## Notes on Methodology

- **Skill file missing**: the expected `rust-logic-audit/SKILL.md` was not
  present at the path the brief specified. Applied the user's described
  5-phase procedure as the audit methodology instead.
- **Out-of-scope**: per the brief, top-level files in `src/providers/`
  (e.g. `mod.rs`, `adapter.rs`, `delta.rs`, `message.rs`, `http_provider.rs`,
  `retry.rs`, `route_*.rs`, `metering.rs`, etc.) are covered by a separate
  subagent. The `openai/` subdirectory does not exist at the
  `src/providers/openai/` level (it appears as a single top-level file
  `openai.rs`); deferred.
- **Read coverage**: ~50 of 100 files deeply read; the remainder scanned
  for `unwrap()`/`expect()` patterns, surface invariants, and module-level
  structure. Failover tests (2429 LOC), failover provider.rs (1516 LOC),
  moa/provider.rs (1755 LOC), protocols/openai_common/openai_strict_schema.rs
  (1476 LOC), and protocols/openai_responses/tests.rs (1546 LOC) were
  read end-to-end or nearly so; smaller modules read fully.
- **No code was modified.** This is a static-review-only pass.

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 0 |
| Suggested Test | 0 |

| Severity | Count |
|----------|-------|
| Critical (architectural invariants documented in code) | 13 |
| Warning (robustness / drift guards) | 12 |
| Suggested (test additions) | 3 |

### Specific items suggested (test additions)

1. **`codex/auth.rs::authorize_via_browser()` end-to-end** — bind-failure
   retry path, state-mismatch retry, browser-open failure cleanup are
   untested.
2. **`responses/shared.rs::build_tools` strict-mode downgrade** — confirm
   that on `StrictResult::Incompatible`, `params` is fully reset to the
   original before the non-strict path runs (no partial mutations leak).
3. **`protocols/loader.rs` should use `ALEPH_HOME` (or
   `crate::utils::paths::get_config_dir`) for the watched directory**
   instead of `std::env::var("HOME")` to match the convention used by
   `model_behaviors::load_model_behavior`. *Suggested (style)*.

### Suggested (style)

- `codex/auth.rs:203` uses `std::sync::Mutex` directly rather than
  `crate::sync_primitives::Mutex` — performance-neutral here (no `.await`
  in the critical section) but stylistically off-policy.

### Negative findings (not problems — explicitly verified)

- **No `&s[..n]` byte slicing in vendor code** — UTF-8 safety preserved.
- **All `lock()` calls use `unwrap_or_else(|e| e.into_inner())`** — the
  poisoned-lock recovery pattern is universal.
- **No `static mut` usage** — `OnceLock`/`Lazy` everywhere.
- **No production `unwrap()` on user-controlled data** — production paths
  propagate errors with `?`; the few `.unwrap()` calls in non-test code
  are on `serde_json::to_value` of type fixtures, `MAX` arithmetic, and
  `Lazy::get_or_init` callbacks (none of which can fail at runtime under
  the documented invariants).
- **No SQL injection concerns** (no LanceDB in this module).
- **`SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()` used
  consistently** for time.
- **`dirs::home_dir()` replaced by `crate::utils::paths::get_config_dir()`**
  in all new code paths (`model_behaviors`, `model_catalog::discovery`,
  `responses::*`); only `protocols::loader::start_watching` still uses
  `std::env::var("HOME")`.