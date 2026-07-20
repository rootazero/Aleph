# Model Behavior Governance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Aleph's harness govern weak/disobedient models (esp. Kimi/Minimax over the anthropic protocol) via per-vendor prompt coaching + elicitation, while adding graceful retreat (timeout grace, soft landing) so weak models aren't managed to death — all in prompt/scaffolding, zero loop cognition.

**Architecture:** A single `resolve_behavior(provider)` collapses two duplicated identity resolutions into one behavior name that drives BOTH the robustness watchdog thresholds AND the `ProviderGuidanceLayer` coaching. Providers self-identify their vendor via a new `AiProvider::behavior_hint()` computed from their own `base_url` (mirroring the existing `protocol()` delegation), so Kimi/Minimax resolve to a new tight `"strict"` family regardless of wire protocol. Per-family elicitation content lives in overridable `model_behaviors/*.md` deltas appended by the layer. WS2 adds a `GraceReason::Timeout` grace turn on per-turn/stall timeouts and fixes the consecutive-failure counter. WS3 flips the Kimi/Minimax presets to the anthropic endpoints.

**Tech Stack:** Rust (tokio + serde), the existing `src/thinker` prompt-layer pipeline, `src/verification` robustness profiles, `src/harness/agent` Think→Act loop, `src/providers` provider trait + presets.

## Global Constraints

- **R10 — no harness/thinker growth:** Do NOT add files to `src/harness/` (12-file budget) and do NOT add a new thinker layer. Reuse `ProviderGuidanceLayer`; put the resolver in `src/orchestrator/harness_bridge/`; coaching content is data (`*.md`).
- **R9/R7 — coaching is prompt, enabling not prescribing:** All control/elicitation is static prompt text selected by behavior name. Use "you may / for complex tasks…" phrasing; never hardcode user-facing reply templates. Control = structural counters/thresholds only; NO semantic judgment in the loop.
- **P8 — machine-stable matching only:** `vendor_identity` matches lowercased `base_url` domain / model-id substrings (machine identifiers), never natural-language regex routing.
- **Byte-compat scope:** Resolution (protocol→behavior for the existing 4 families) and robustness profiles for `anthropic`/`ollama`/`conservative` stay byte-identical. Prompt *content* changes intentionally (new deltas + strict family).
- **cargo discipline (user standing preference — 极度节制):** Do NOT run the full suite. Per task, run only the targeted module test (`cargo test -p alephcore <module>::tests::<name>`). At most one `cargo check -p alephcore --lib` at the end of WS1 and once after WS2.
- **Commits:** English, `<scope>: <description>` (e.g. `providers: add behavior_hint vendor self-identification`).
- **`docs/superpowers/` is gitignored** — do not attempt to `git add` plan/spec files.

---

## File Structure

**Create:**
- `src/providers/model_behaviors/strict.md` — strict-family coaching delta (tight control + minimal elicitation).
- `src/orchestrator/harness_bridge/behavior_resolve.rs` — `resolve_behavior(provider)` unified resolver.

**Modify:**
- `src/providers/model_behaviors/mod.rs` — add `vendor_identity()`, `BUILTIN_STRICT`, extend `builtin_behavior()`.
- `src/providers/model_behaviors/{anthropic,openai,gemini,ollama}.md` — rewrite as appended deltas (existing family tails + L2 elicitation).
- `src/providers/mod.rs` — add `AiProvider::behavior_hint()` (default `None`).
- `src/providers/http_provider.rs` — impl `behavior_hint()` from `config.base_url` + `default_model()`.
- `src/providers/metering.rs`, `src/providers/model_override_provider.rs`, `src/providers/failover/provider.rs` — delegate `behavior_hint()`.
- `src/verification/robustness_profile.rs` — add `"strict"` arm to `for_behavior`.
- `src/orchestrator/harness_bridge/mod.rs` — declare `mod behavior_resolve;`.
- `src/orchestrator/harness_bridge/runner_impl.rs:250-255` — resolve robustness via `resolve_behavior`.
- `src/gateway/execution_engine/run_loop/inner.rs:532-554` — delete the loaded-then-discarded block.
- `src/thinker/prompt_layer.rs` — rename field `provider_protocol`→`behavior_name`; add `model_behavior_delta`; builders.
- `src/thinker/prompt_builder/mod.rs` + `cache.rs` — rename + thread the two fields.
- `src/orchestrator/harness_bridge/prompt_build.rs:440-443` — feed `resolve_behavior` + pre-load delta.
- `src/thinker/layers/provider_guidance.rs` — dispatch by `behavior_name`; add `strict`; append delta.
- `src/harness/agent/think.rs` — `GraceReason::Timeout` + `GRACE_NUDGE_TIMEOUT`.
- `src/harness/agent.rs` — fire timeout grace; consecutive-failure counter fix + soft-landing.
- `src/providers/presets/registry.rs` — Kimi/Minimax anthropic-primary presets.

---

## Task 1: vendor_identity table + strict family content

**Files:**
- Modify: `src/providers/model_behaviors/mod.rs`
- Create: `src/providers/model_behaviors/strict.md`

**Interfaces:**
- Produces: `pub fn vendor_identity(base_url: Option<&str>, model_id: &str) -> Option<&'static str>`; `builtin_behavior("strict")` returns the strict markdown.

- [ ] **Step 1: Create `strict.md`** (the new strict-family delta)

```markdown
## Strict Operating Mode (open-weight / weaker instruction-following model)

You are running under strict harness governance because this model family
benefits from tight, explicit rails. Follow these exactly:

- **One tool call at a time.** Make a single tool call, wait for its result,
  read it, then decide the next call. Do not batch many calls blindly.
- **Exact tool format.** Emit tool calls in the required structured format with
  valid JSON arguments. Never wrap a tool call in prose or invent fields.
- **No repetition.** If a tool call fails or returns the same result twice,
  STOP repeating it. Change the arguments, switch tools, or summarize what you
  have — repeating an identical failing call never helps.
- **No fabrication.** Never invent file contents, command output, URLs, dates,
  or results. If you need a fact, call a tool to get it; if a tool cannot get
  it, say so plainly.
- **Plan in one line, then act.** For a multi-step task, state your plan in a
  single short line, then execute it step by step. Do not over-think or write
  long planning monologues.
- **Finish concretely.** When the task is done, give a short, direct final
  answer. Do not keep calling tools after you have the answer.
```

- [ ] **Step 2: Write the failing tests** in `src/providers/model_behaviors/mod.rs` (append into the existing `#[cfg(test)] mod tests`)

```rust
    #[test]
    fn vendor_identity_matches_kimi_by_url_and_model() {
        assert_eq!(
            vendor_identity(Some("https://api.moonshot.cn/anthropic"), "kimi-k2-0905-preview"),
            Some("strict")
        );
        assert_eq!(vendor_identity(None, "kimi-k2-turbo-preview"), Some("strict"));
        assert_eq!(vendor_identity(Some("https://api.moonshot.ai/v1"), "moonshot-v1-8k"), Some("strict"));
    }

    #[test]
    fn vendor_identity_matches_minimax_including_abab_by_url() {
        // MiniMax-M2.5 self-identifies by model id…
        assert_eq!(vendor_identity(Some("https://api.minimaxi.com/anthropic"), "MiniMax-M2.5"), Some("strict"));
        // …but legacy `abab*` ids do NOT contain "minimax" — domain must catch it.
        assert_eq!(vendor_identity(Some("https://api.minimaxi.com/anthropic"), "abab6.5s-chat"), Some("strict"));
        assert_eq!(vendor_identity(None, "abab6.5s-chat"), Some("strict"));
    }

    #[test]
    fn vendor_identity_matches_other_oss_families() {
        assert_eq!(vendor_identity(Some("https://api.deepseek.com"), "deepseek-chat"), Some("strict"));
        assert_eq!(vendor_identity(None, "qwen-max"), Some("strict"));
        assert_eq!(vendor_identity(Some("https://dashscope.aliyuncs.com"), "qwq-32b"), Some("strict"));
        assert_eq!(vendor_identity(None, "glm-4-plus"), Some("strict"));
    }

    #[test]
    fn vendor_identity_ignores_strong_models() {
        assert_eq!(vendor_identity(Some("https://api.openai.com/v1"), "gpt-4o"), None);
        assert_eq!(vendor_identity(Some("https://api.anthropic.com"), "claude-sonnet-4-6"), None);
        assert_eq!(vendor_identity(None, "gemini-2.5-pro"), None);
    }

    #[test]
    fn builtin_strict_loads() {
        let content = builtin_behavior("strict").unwrap();
        assert!(content.contains("Strict Operating Mode"));
        assert!(content.contains("One tool call at a time"));
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p alephcore providers::model_behaviors::tests::vendor_identity -- --nocapture`
Expected: FAIL — `cannot find function vendor_identity` / `builtin_behavior` returns None for "strict".

- [ ] **Step 4: Implement `vendor_identity` + `BUILTIN_STRICT`** in `src/providers/model_behaviors/mod.rs`

Add the const near the other `BUILTIN_*` consts (after line 9):

```rust
const BUILTIN_STRICT: &str = include_str!("strict.md");
```

Add `"strict"` to `builtin_behavior` (inside the `match name` at line 48):

```rust
        "strict" => Some(BUILTIN_STRICT.to_string()),
```

Add the new function after `protocol_to_behavior` (after line 68):

```rust
/// Self-identify a weak/open-weight vendor from its endpoint and model id so
/// it can be governed with the tight `"strict"` profile even when it shares a
/// wire protocol with a strong model (e.g. Kimi/Minimax over the anthropic
/// protocol). Matching is on machine-stable identifiers only — base-URL host
/// substrings and model-id substrings, lowercased (P8: never natural language).
///
/// `base_url` is the more reliable signal (one provider → one endpoint;
/// Minimax's legacy `abab*` model ids do not contain the vendor name), so it
/// is checked first. Returns the behavior name (`"strict"`) or `None` for
/// unrecognized / strong vendors.
#[must_use]
pub fn vendor_identity(base_url: Option<&str>, model_id: &str) -> Option<&'static str> {
    // (signal substring, behavior). Domain signals first, then model-id signals.
    const URL_SIGNALS: &[&str] = &[
        "moonshot.cn",
        "moonshot.ai",
        "minimaxi.com",
        "minimax.io",
        "api.deepseek.com",
        "dashscope.aliyuncs.com",
        "open.bigmodel.cn",
    ];
    const MODEL_SIGNALS: &[&str] = &[
        "moonshot", "kimi", "minimax", "abab", "deepseek", "qwen", "qwq", "glm", "chatglm",
    ];
    if let Some(url) = base_url {
        let url = url.to_ascii_lowercase();
        if URL_SIGNALS.iter().any(|s| url.contains(s)) {
            return Some("strict");
        }
    }
    let model = model_id.to_ascii_lowercase();
    if MODEL_SIGNALS.iter().any(|s| model.contains(s)) {
        return Some("strict");
    }
    None
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore providers::model_behaviors::tests -- --nocapture`
Expected: PASS (all vendor_identity + builtin_strict tests + the pre-existing tests).

- [ ] **Step 6: Commit**

```bash
git add src/providers/model_behaviors/mod.rs src/providers/model_behaviors/strict.md
git commit -m "providers: vendor_identity table + strict behavior family"
```

---

## Task 2: AiProvider::behavior_hint() — provider self-identification

**Files:**
- Modify: `src/providers/mod.rs` (trait default), `src/providers/http_provider.rs` (concrete impl), `src/providers/metering.rs`, `src/providers/model_override_provider.rs`, `src/providers/failover/provider.rs` (delegations)

**Interfaces:**
- Consumes: `vendor_identity` (Task 1).
- Produces: `fn behavior_hint(&self) -> Option<Cow<'_, str>>` on `AiProvider`.

- [ ] **Step 1: Write the failing test** — append to `#[cfg(test)] mod tests` in `src/providers/model_override_provider.rs`

```rust
    #[test]
    fn delegates_behavior_hint_to_inner() {
        struct HintInner;
        impl AiProvider for HintInner {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::text_only("inner".to_string())) })
            }
            fn name(&self) -> &str { "inner" }
            fn color(&self) -> &str { "#000" }
            fn behavior_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
                Some(std::borrow::Cow::Borrowed("strict"))
            }
        }
        let wrapped = ModelOverrideProvider::new(Arc::new(HintInner), "m");
        assert_eq!(wrapped.behavior_hint().as_deref(), Some("strict"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p alephcore providers::model_override_provider::tests::delegates_behavior_hint_to_inner`
Expected: FAIL — `no method named behavior_hint`.

- [ ] **Step 3: Add the trait default** in `src/providers/mod.rs` (after `model_behavior_override`, before `as_http_provider`, ~line 633)

```rust
    /// Self-identified governance behavior name derived from the provider's
    /// own endpoint/model (e.g. Kimi/Minimax → `"strict"`). Sits ABOVE the
    /// protocol fallback but BELOW the explicit config `model_behavior`
    /// override in `resolve_behavior`. Default `None` = "no opinion, use the
    /// protocol default". Wrappers delegate; `HttpProvider` computes it.
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        None
    }
```

- [ ] **Step 4: Implement on `HttpProvider`** in `src/providers/http_provider.rs` (after `model_behavior_override`, ~line 633)

```rust
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        crate::providers::model_behaviors::vendor_identity(
            self.config.base_url.as_deref(),
            self.config.default_model(),
        )
        .map(Cow::Borrowed)
    }
```

- [ ] **Step 5: Delegate in the three wrappers**

`src/providers/metering.rs` (after `model_behavior_override`, ~line 104):

```rust
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.behavior_hint()
    }
```

`src/providers/model_override_provider.rs` (after `model_behavior_override`, ~line 69):

```rust
    fn behavior_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.inner.behavior_hint()
    }
```

`src/providers/failover/provider.rs` (after `model_behavior_override`, ~line 795) — mirror the `current()`→owned pattern:

```rust
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .behavior_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p alephcore providers::model_override_provider::tests`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/providers/mod.rs src/providers/http_provider.rs src/providers/metering.rs src/providers/model_override_provider.rs src/providers/failover/provider.rs
git commit -m "providers: behavior_hint() vendor self-identification (+ wrapper passthrough)"
```

---

## Task 3: strict robustness profile

**Files:**
- Modify: `src/verification/robustness_profile.rs`

**Interfaces:**
- Produces: `ModelRobustnessProfile::for_behavior("strict")` returns the tightest profile.

- [ ] **Step 1: Write the failing test** — append to `#[cfg(test)] mod tests`

```rust
    #[test]
    fn for_behavior_strict_is_tightest() {
        let strict = ModelRobustnessProfile::for_behavior(Some("strict"));
        let ollama = ModelRobustnessProfile::for_behavior(Some("ollama"));
        assert!(strict.repeat_threshold <= ollama.repeat_threshold);
        assert!(strict.steer_max <= ollama.steer_max);
        assert!(strict.novelty_min >= ollama.novelty_min);
        assert!(!strict.silence_required);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p alephcore verification::robustness_profile::tests::for_behavior_strict_is_tightest`
Expected: FAIL — strict currently resolves to `conservative()` (repeat_threshold 5 > ollama's 3).

- [ ] **Step 3: Add the `"strict"` arm** to `for_behavior` in `src/verification/robustness_profile.rs` (inside the `match name`, before the `_ =>` arm at line 60)

```rust
            // Open-weight / weaker instruction-followers (Kimi, Minimax,
            // DeepSeek, Qwen, GLM) self-identified by `vendor_identity`.
            // Tightest leash: steer early, few chances, tolerate little thrash.
            Some("strict") => Self {
                repeat_threshold: 3,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 5,
                novelty_min: 0.6,
                silence_required: false,
            },
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore verification::robustness_profile::tests`
Expected: PASS (new test + existing `conservative_matches_legacy_behavior` etc. unchanged).

- [ ] **Step 5: Commit**

```bash
git add src/verification/robustness_profile.rs
git commit -m "verification: strict robustness profile for weak-model family"
```

---

## Task 4: resolve_behavior unified resolver

**Files:**
- Create: `src/orchestrator/harness_bridge/behavior_resolve.rs`
- Modify: `src/orchestrator/harness_bridge/mod.rs` (add `mod behavior_resolve;` + `pub use`)

**Interfaces:**
- Consumes: `AiProvider::{model_behavior_override, behavior_hint, protocol}`, `model_behaviors::protocol_to_behavior`.
- Produces: `pub fn resolve_behavior(provider: &dyn AiProvider) -> std::borrow::Cow<'static, str>`.

- [ ] **Step 1: Create `src/orchestrator/harness_bridge/behavior_resolve.rs`**

```rust
//! Single source of truth for a provider's governance behavior name.
//!
//! Collapses the two previously-duplicated resolutions (robustness thresholds
//! in `runner_impl` and the discarded diagnostic block in the gateway run
//! loop) into one function. The returned behavior name drives BOTH the
//! `ModelRobustnessProfile` watchdog thresholds AND the `ProviderGuidanceLayer`
//! coaching, so they can never drift.
//!
//! Precedence (highest first):
//!   1. explicit per-provider config `model_behavior` override
//!   2. vendor self-identification (`behavior_hint`, e.g. Kimi/Minimax →
//!      "strict") — MUST sit above protocol so a weak model on the anthropic
//!      wire protocol is not mistaken for Claude.
//!   3. protocol → behavior auto-mapping (anthropic/openai/gemini/ollama)
//!   4. "unknown" (conservative thresholds + non-anthropic baseline coaching)

use std::borrow::Cow;

use crate::providers::AiProvider;

/// Resolve the governance behavior name for `provider`. Always returns an
/// owned-or-static `Cow` so callers can feed it to both the robustness
/// profile (`for_behavior`) and the prompt builder without lifetime grief.
#[must_use]
pub fn resolve_behavior(provider: &dyn AiProvider) -> Cow<'static, str> {
    if let Some(over) = provider.model_behavior_override() {
        return Cow::Owned(over.into_owned());
    }
    if let Some(hint) = provider.behavior_hint() {
        return Cow::Owned(hint.into_owned());
    }
    if let Some(name) = crate::providers::model_behaviors::protocol_to_behavior(&provider.protocol())
    {
        return Cow::Borrowed(name);
    }
    Cow::Borrowed("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::message::UnifiedMessage;
    use std::future::Future;
    use std::pin::Pin;

    struct StubProvider {
        protocol: &'static str,
        override_: Option<&'static str>,
        hint: Option<&'static str>,
    }
    impl AiProvider for StubProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("x".to_string())) })
        }
        fn name(&self) -> &str { "stub" }
        fn color(&self) -> &str { "#000" }
        fn protocol(&self) -> Cow<'_, str> { Cow::Borrowed(self.protocol) }
        fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
            self.override_.map(Cow::Borrowed)
        }
        fn behavior_hint(&self) -> Option<Cow<'_, str>> {
            self.hint.map(Cow::Borrowed)
        }
    }

    fn p(protocol: &'static str, override_: Option<&'static str>, hint: Option<&'static str>) -> StubProvider {
        StubProvider { protocol, override_, hint }
    }

    #[test]
    fn override_wins_over_everything() {
        let _ = UnifiedMessage::user("warmup");
        assert_eq!(resolve_behavior(&p("anthropic", Some("openai"), Some("strict"))), "openai");
    }

    #[test]
    fn hint_wins_over_protocol_kimi_over_anthropic() {
        // THE headline case: Kimi on the anthropic wire protocol must resolve
        // to "strict", NOT "anthropic" (Claude's loose leash).
        assert_eq!(resolve_behavior(&p("anthropic", None, Some("strict"))), "strict");
    }

    #[test]
    fn protocol_fallback_when_no_override_no_hint() {
        assert_eq!(resolve_behavior(&p("openai", None, None)), "openai");
        assert_eq!(resolve_behavior(&p("anthropic", None, None)), "anthropic");
    }

    #[test]
    fn unknown_protocol_falls_back_to_unknown() {
        assert_eq!(resolve_behavior(&p("some-custom", None, None)), "unknown");
    }
}
```

- [ ] **Step 2: Register the module** in `src/orchestrator/harness_bridge/mod.rs`

Find the `mod` declarations block and add (alphabetically near the others):

```rust
mod behavior_resolve;
pub(crate) use behavior_resolve::resolve_behavior;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p alephcore harness_bridge::behavior_resolve::tests`
Expected: PASS (4 tests, incl. `hint_wins_over_protocol_kimi_over_anthropic`).

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/harness_bridge/behavior_resolve.rs src/orchestrator/harness_bridge/mod.rs
git commit -m "harness_bridge: resolve_behavior single source of truth"
```

---

## Task 5: wire resolver into robustness + delete discarded block

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs:250-255`
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:532-554`

**Interfaces:**
- Consumes: `resolve_behavior` (Task 4).

- [ ] **Step 1: Replace the robustness resolution** in `runner_impl.rs` (lines 250-255)

Old:

```rust
        let robustness_profile = crate::verification::ModelRobustnessProfile::for_behavior(
            llm.model_behavior_override().as_deref().or_else(|| {
                crate::providers::model_behaviors::protocol_to_behavior(&llm.protocol())
            }),
        )
        .clamped();
```

New:

```rust
        let behavior_name = crate::orchestrator::harness_bridge::resolve_behavior(llm.as_ref());
        let robustness_profile =
            crate::verification::ModelRobustnessProfile::for_behavior(Some(&behavior_name)).clamped();
```

- [ ] **Step 2: Delete the loaded-then-discarded block** in `inner.rs` (lines 532-554, the entire `// Resolve model behavior: …` braced block that ends by only logging). Remove it wholesale — the real resolution now lives in `resolve_behavior` consumed by `runner_impl` (robustness) and `prompt_build` (coaching, Task 6). If `load_model_behavior` / `protocol_to_behavior` imports become unused in `inner.rs`, remove those imports too.

- [ ] **Step 3: Compile-check the two crates touched**

Run: `cargo check -p alephcore --lib`
Expected: PASS (no unused-import / type errors). If `protocol_to_behavior` or `load_model_behavior` is now an unused import in `inner.rs`, delete it and re-run.

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/harness_bridge/runner_impl.rs src/gateway/execution_engine/run_loop/inner.rs
git commit -m "harness: drive robustness via resolve_behavior; drop discarded behavior load"
```

---

## Task 6: rename provider_protocol→behavior_name + thread coaching delta

**Files:**
- Modify: `src/thinker/prompt_layer.rs`, `src/thinker/prompt_builder/mod.rs`, `src/thinker/prompt_builder/cache.rs`, `src/orchestrator/harness_bridge/prompt_build.rs`

**Interfaces:**
- Consumes: `resolve_behavior` (Task 4), `model_behaviors::load_model_behavior`.
- Produces: `LayerInput.behavior_name: Option<&'a str>`, `LayerInput.model_behavior_delta: Option<&'a str>` + `with_behavior_name[_opt]` / `with_model_behavior_delta[_opt]`; `PromptBuilder::with_behavior_name` / `with_model_behavior_delta`.

> The `provider_protocol` field has exactly one consumer (`ProviderGuidanceLayer`) and is already fed `override-or-protocol`. Rename it `behavior_name` (compiler-enforced, can't miss a site) and add a parallel `model_behavior_delta` string for the pre-loaded `.md` content.

- [ ] **Step 1: In `src/thinker/prompt_layer.rs`** rename the `LayerInput` field and add the delta field (line 139):

Old:

```rust
    pub provider_protocol: Option<&'a str>,
```

New:

```rust
    /// Resolved governance behavior name (`resolve_behavior`: anthropic /
    /// openai / gemini / ollama / strict / unknown). Drives
    /// `ProviderGuidanceLayer`'s baseline-block selection. `None` keeps the
    /// layer silent (capture / snapshot / tests).
    pub behavior_name: Option<&'a str>,
    /// Pre-loaded per-family coaching delta from `model_behaviors/{name}.md`
    /// (overridable at `~/.aleph/model_behaviors/`). Appended verbatim by
    /// `ProviderGuidanceLayer` after the shared baseline blocks. `None` = no
    /// delta for this family.
    pub model_behavior_delta: Option<&'a str>,
```

In each of the 4 constructors (`basic`, `hydration`, `soul`, `context` — at lines 173, 199, 229, 255 the `provider_protocol: None,` line), replace `provider_protocol: None,` with:

```rust
            behavior_name: None,
            model_behavior_delta: None,
```

Rename the builder methods (lines 401, 409):

```rust
    #[must_use]
    pub const fn with_behavior_name(mut self, name: &'a str) -> Self {
        self.behavior_name = Some(name);
        self
    }

    #[must_use]
    pub const fn with_behavior_name_opt(mut self, name: Option<&'a str>) -> Self {
        self.behavior_name = name;
        self
    }

    #[must_use]
    pub const fn with_model_behavior_delta_opt(mut self, delta: Option<&'a str>) -> Self {
        self.model_behavior_delta = delta;
        self
    }
```

- [ ] **Step 2: In `src/thinker/prompt_builder/mod.rs`** rename the field + builder + add the delta field/builder, and thread both at every `with_provider_protocol_opt` site.

Rename the field (line 181) and add delta:

```rust
    behavior_name: Option<String>,
    model_behavior_delta: Option<String>,
```

In the `Default`/constructor block (line 216, `provider_protocol: None,`):

```rust
            behavior_name: None,
            model_behavior_delta: None,
```

Rename the builder (line 324):

```rust
    pub fn with_behavior_name(mut self, name: impl Into<String>) -> Self {
        self.behavior_name = Some(name.into());
        self
    }

    pub fn with_model_behavior_delta(mut self, delta: Option<String>) -> Self {
        self.model_behavior_delta = delta;
        self
    }
```

At EACH of the threading sites (lines 374, 399, 427, 453, 477 — `with_provider_protocol_opt(self.provider_protocol.as_deref())`), replace with:

```rust
            .with_behavior_name_opt(self.behavior_name.as_deref())
            .with_model_behavior_delta_opt(self.model_behavior_delta.as_deref())
```

- [ ] **Step 3: In `src/thinker/prompt_builder/cache.rs`** (line ~76) replace the `with_provider_protocol_opt` thread:

```rust
            .with_behavior_name_opt(self.behavior_name.as_deref())
            .with_model_behavior_delta_opt(self.model_behavior_delta.as_deref())
```

- [ ] **Step 4: In `src/orchestrator/harness_bridge/prompt_build.rs`** (lines 440-443) replace the `provider_protocol` computation with the resolver + delta pre-load. `build_system_prompt` is `async`, so the load is fine here:

Old:

```rust
        let provider_protocol = provider
            .model_behavior_override()
            .map_or_else(|| provider.protocol().into_owned(), |s| s.into_owned());
        builder = builder.with_provider_protocol(provider_protocol);
```

New:

```rust
        // Resolve the governance behavior name once (same source of truth as
        // the robustness profile) and pre-load its overridable coaching delta.
        let behavior_name = crate::orchestrator::harness_bridge::resolve_behavior(provider);
        let behavior_delta =
            crate::providers::model_behaviors::load_model_behavior(&behavior_name).await;
        builder = builder
            .with_behavior_name(behavior_name.into_owned())
            .with_model_behavior_delta(behavior_delta);
```

- [ ] **Step 5: Compile-check** (the rename touches several files; the consumer in `provider_guidance.rs` is updated in Task 7, so expect ONE error there — that is acceptable mid-rename).

Run: `cargo check -p alephcore --lib 2>&1 | grep -A3 "provider_protocol\|behavior_name" | head`
Expected: the ONLY remaining reference to the old name is in `src/thinker/layers/provider_guidance.rs` (fixed next task). Everything else compiles.

- [ ] **Step 6: Commit**

```bash
git add src/thinker/prompt_layer.rs src/thinker/prompt_builder/mod.rs src/thinker/prompt_builder/cache.rs src/orchestrator/harness_bridge/prompt_build.rs
git commit -m "thinker: rename provider_protocol->behavior_name; thread coaching delta"
```

---

## Task 7: re-key ProviderGuidanceLayer + author per-family deltas

**Files:**
- Modify: `src/thinker/layers/provider_guidance.rs`
- Modify: `src/providers/model_behaviors/{anthropic,openai,gemini,ollama}.md`

**Interfaces:**
- Consumes: `LayerInput.behavior_name`, `LayerInput.model_behavior_delta` (Task 6).

> Keep the two SHARED baseline consts (`TOOL_USE_ENFORCEMENT`, `TOOL_PERSISTENCE_DOCTRINE`) so existing per-family baseline output stays byte-identical. MOVE the per-family tails (`OPENAI_EXECUTION_DISCIPLINE_TAIL`, `GOOGLE_OPERATIONAL_DIRECTIVES`) into their `.md` deltas, and add L2 elicitation. Dispatch by `behavior_name` (adds `strict`); append the delta last.

- [ ] **Step 1: Rewrite the `.md` deltas** (these are appended verbatim after the baseline consts).

`src/providers/model_behaviors/openai.md` (ports `OPENAI_EXECUTION_DISCIPLINE_TAIL` + L2):

```markdown
## Execution Discipline — OpenAI Family

**Act, don't ask** — when a question has an obvious default interpretation, act on it. Only ask for clarification when the ambiguity genuinely changes what tool you would call.

**Verify before finalizing**: correctness, grounding (factual claims backed by tool outputs), formatting, safety (confirm scope before side-effecting actions).

## Working at Full Capability

For genuinely complex, multi-step work you may briefly plan before acting: decompose the goal into steps, then execute them. Use higher reasoning effort where the task warrants it. Before declaring done, re-read the original request and confirm every part is satisfied.
```

`src/providers/model_behaviors/gemini.md` (ports `GOOGLE_OPERATIONAL_DIRECTIVES` + L2):

```markdown
## Google Model Operational Directives

- **Absolute paths**: always construct and use absolute file paths for all file-system operations.
- **Verify first**: read the file or search the project before making changes — never guess at contents.
- **Dependency checks**: never assume a library is available; check package.json / requirements.txt / Cargo.toml first.
- **Conciseness**: narrate each step in one short line (what + why); no paragraphs.
- **Parallel tool calls**: when independent operations are needed (reading several files, for example), make all the calls in a single response rather than sequentially.
- **Non-interactive commands**: pass flags like `-y`, `--yes`, `--non-interactive` to prevent CLI tools from hanging on prompts.
- **Keep going**: work autonomously until the task is fully resolved — don't stop with a plan, execute it.

## Working at Full Capability

For complex multi-step work, briefly decompose the goal before executing, and verify the result against the original request before finishing.
```

`src/providers/model_behaviors/ollama.md` (keep the existing tool-usage guide — it is good rails for weak local models; trim duplicate "use tools" lines already covered by the baseline):

```markdown
## Local / Open-Weight Model Guide

You are a local open-weight model. Stay on rails:

1. Read each tool description carefully before calling it.
2. Call ONE tool at a time with valid parameters, then wait for the result.
3. If a tool call fails, read the error and retry with corrected parameters — do not repeat the same failing call.
4. Execute step by step; keep text responses short.
5. Do not invent information. If you need data, use a tool to get it.

For a multi-step task, state your plan in one short line, then carry it out.
```

`src/providers/model_behaviors/anthropic.md` (minimal — Claude's training already self-plans; one light enabling line, keeping the file near-empty per the existing intent):

```markdown
<!-- Claude's alignment already favors proactive, planned execution; keep coaching minimal. -->

For genuinely complex tasks you may use extended reasoning to plan before acting.
```

- [ ] **Step 2: Update the failing tests** in `src/thinker/layers/provider_guidance.rs`. The existing tests build `LayerInput` with `with_provider_protocol_opt(...)`. Replace each with `with_behavior_name_opt(...)`, drop the assertions for the MOVED tails (`"Act, don't ask"`, `"Absolute paths"`) from the no-delta tests, and add new tests proving (a) the delta is appended and (b) the `strict` arm. Replace the whole `#[cfg(test)] mod tests` body's relevant tests with:

```rust
    #[test]
    fn silent_when_behavior_missing() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn anthropic_baseline_is_persistence_only() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_behavior_name_opt(Some("anthropic"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Execution Discipline — Persistence"));
        assert!(!out.contains("## Tool-Use Enforcement"));
    }

    #[test]
    fn non_anthropic_baseline_has_tool_use_and_persistence() {
        for behavior in ["openai", "gemini", "ollama", "strict", "unknown"] {
            let layer = ProviderGuidanceLayer;
            let config = PromptConfig::default();
            let tools = vec![];
            let input = LayerInput::basic(&config, &tools).with_behavior_name_opt(Some(behavior));
            let mut out = String::new();
            layer.inject(&mut out, &input);
            assert!(out.contains("## Tool-Use Enforcement"), "{behavior}");
            assert!(out.contains("## Execution Discipline — Persistence"), "{behavior}");
        }
    }

    #[test]
    fn delta_is_appended_after_baseline() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let delta = "## Execution Discipline — OpenAI Family\nAct, don't ask";
        let input = LayerInput::basic(&config, &tools)
            .with_behavior_name_opt(Some("openai"))
            .with_model_behavior_delta_opt(Some(delta));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        let baseline_at = out.find("## Tool-Use Enforcement").unwrap();
        let delta_at = out.find("Act, don't ask").unwrap();
        assert!(delta_at > baseline_at, "delta must follow the baseline");
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p alephcore thinker::layers::provider_guidance`
Expected: FAIL — `no method named with_behavior_name_opt` / `inject` still reads `provider_protocol`.

- [ ] **Step 4: Rewrite `ProviderGuidanceLayer::inject`** in `src/thinker/layers/provider_guidance.rs`. Replace the whole `fn inject` body (lines 50-105) and DELETE the now-unused `OPENAI_EXECUTION_DISCIPLINE_TAIL` (lines 154-165) and `GOOGLE_OPERATIONAL_DIRECTIVES` (lines 167-184) consts (their text moved to `.md`):

```rust
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let behavior = match input.behavior_name {
            Some(b) => b,
            None => return,
        };
        // Shared baseline, selected by behavior family. Anthropic skips the
        // heavy tool-use enforcement (native tool_use is well-trained); every
        // family — Anthropic included — gets the persistence doctrine.
        if behavior != "anthropic" {
            output.push_str(TOOL_USE_ENFORCEMENT);
            output.push_str("\n\n");
        }
        output.push_str(TOOL_PERSISTENCE_DOCTRINE);
        output.push_str("\n\n");
        // Per-family delta (OpenAI tail / Google directives / ollama rails /
        // strict control / L2 elicitation) — data-driven, user-overridable at
        // `~/.aleph/model_behaviors/{behavior}.md`.
        if let Some(delta) = input.model_behavior_delta {
            let delta = delta.trim();
            if !delta.is_empty() {
                output.push_str(delta);
                output.push_str("\n\n");
            }
        }
    }
```

Update the module doc-comment (lines 1-13) to say it dispatches on the resolved behavior name (not raw protocol) and that per-family content is the overridable `.md` delta.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore thinker::layers::provider_guidance`
Expected: PASS.

- [ ] **Step 6: Whole-crate compile check** (closes the Task 6 mid-rename gap)

Run: `cargo check -p alephcore --lib`
Expected: PASS — no remaining `provider_protocol` references.

- [ ] **Step 7: Commit**

```bash
git add src/thinker/layers/provider_guidance.rs src/providers/model_behaviors/anthropic.md src/providers/model_behaviors/openai.md src/providers/model_behaviors/gemini.md src/providers/model_behaviors/ollama.md
git commit -m "thinker: ProviderGuidanceLayer dispatch by behavior name + per-family .md deltas"
```

---

## Task 8: GraceReason::Timeout

**Files:**
- Modify: `src/harness/agent/think.rs`

**Interfaces:**
- Produces: `GraceReason::Timeout` (+ `GRACE_NUDGE_TIMEOUT`), consumed by Task 9.

- [ ] **Step 1: Write the failing test** — append to `#[cfg(test)] mod tests` in `think.rs`

```rust
    #[test]
    fn grace_nudge_timeout_is_distinct_and_addresses_user() {
        assert_eq!(GraceReason::Timeout.nudge(), GRACE_NUDGE_TIMEOUT);
        assert_ne!(GRACE_NUDGE_TIMEOUT, GRACE_NUDGE_MAX_ITERATIONS);
        assert!(GRACE_NUDGE_TIMEOUT.contains("summar"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p alephcore harness::agent::think::tests::grace_nudge_timeout`
Expected: FAIL — `no variant Timeout` / `GRACE_NUDGE_TIMEOUT` undefined.

- [ ] **Step 3: Add the const** (after `GRACE_NUDGE_TOOL_LOOP_HALT`, ~line 110):

```rust
/// Ephemeral nudge for the grace turn fired when a per-turn or stall timeout
/// trips — likely a slow or stuck step. The model gets ONE tool-less, short-
/// budgeted chance to deliver a partial result instead of the run ending with
/// no terminal text. The model writes the actual content (R7 — no template).
const GRACE_NUDGE_TIMEOUT: &str =
    "The time budget for this step was exhausted (a step may be slow or stuck) \
     and the run is wrapping up. Do NOT call any more tools. Respond now with a \
     short summary for the user: what you accomplished, what remains, and any \
     partial result you can deliver right now.";
```

- [ ] **Step 4: Add the variant** to `enum GraceReason` (after `ToolLoopHalt`, ~line 172):

```rust
    /// Per-turn / stall timeout tripped — salvage a partial deliverable under a
    /// dedicated short budget instead of cold-terminating on a hung step.
    Timeout,
```

And the `nudge()` arm (in the `match self`, ~line 183):

```rust
            Self::Timeout => GRACE_NUDGE_TIMEOUT,
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p alephcore harness::agent::think::tests::grace_nudge_timeout`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/harness/agent/think.rs
git commit -m "harness: GraceReason::Timeout nudge for timeout salvage"
```

---

## Task 9: fire timeout grace under a dedicated short budget

**Files:**
- Modify: `src/harness/agent.rs`

**Interfaces:**
- Consumes: `GraceReason::Timeout` (Task 8), existing `fire_boundary_grace_turn`.

> `fire_boundary_grace_turn` is already fail-soft and races the LLM call against cancel + turn_timeout. A hung provider could make the grace call wait another full `turn_timeout`, so bound it with an outer `tokio::time::timeout(GRACE_TIMEOUT_BUDGET, …)`. Worst case = today (no summary); best case = partial delivery.

- [ ] **Step 1: Add the budget const** near the top of `src/harness/agent.rs` (with the other consts, e.g. just below the `impl AgentHarness` constants or module consts). Search for an existing `const` block; add:

```rust
/// Dedicated short budget for the timeout-salvage grace turn. A per-turn /
/// stall timeout means a step is likely slow or hung, so the salvage call must
/// not itself wait another full `turn_timeout`. Fail-soft on expiry.
const GRACE_TIMEOUT_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
```

- [ ] **Step 2: Fire grace in the stall-watchdog block** (lines 478-492). Insert the grace call before `callback.on_complete();` (line 490):

```rust
                    let _ = tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await;
                    callback.on_complete();
```

- [ ] **Step 3: Fire grace in the per-turn-timeout block** (`Err(HarnessError::StalledTurn …)`, lines 505-519). Insert the same grace call before `callback.on_complete();` (line 517):

```rust
                    let _ = tokio::time::timeout(
                        GRACE_TIMEOUT_BUDGET,
                        self.fire_boundary_grace_turn(
                            &current_session,
                            callback,
                            iterations,
                            crate::harness::agent::think::GraceReason::Timeout,
                            cancel,
                        ),
                    )
                    .await;
                    callback.on_complete();
```

- [ ] **Step 4: Compile-check**

Run: `cargo check -p alephcore --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/harness/agent.rs
git commit -m "harness: best-effort grace salvage on per-turn/stall timeout"
```

---

## Task 10: consecutive-failure counter fix + soft-landing

**Files:**
- Modify: `src/harness/agent.rs`

**Interfaces:**
- Consumes: existing `consecutive_failure_cap`, `fire_boundary_grace_turn`, the session event log.

> Two changes inside the `executed == 0 && !vetoed` failure block (lines 541-593): (a) count failures vs successes so a *majority-failure* mixed turn also increments, and an interleaved single success no longer zeroes a churning streak; (b) emit ONE soft-landing reminder one turn before the cap. Pure structural counting — no progress/semantic judgment (R10-safe).

- [ ] **Step 1: Write the failing test** — append to `#[cfg(test)] mod tests` in `agent.rs`. (Counts are pure-function logic, so test the extracted helper.)

```rust
    #[test]
    fn failure_streak_counts_majority_failure_not_just_total_failure() {
        // (executed, errors) -> should this turn increment the streak?
        assert!(super::is_failure_turn(0, 2));   // total failure
        assert!(super::is_failure_turn(1, 3));   // majority failure (1 ok, 3 err)
        assert!(!super::is_failure_turn(3, 1));  // mostly success → not a failure turn
        assert!(!super::is_failure_turn(2, 0));  // clean → not a failure turn
    }

    #[test]
    fn failure_streak_resets_only_on_clean_turn() {
        assert!(super::is_clean_turn(2, 0));     // zero errors → reset
        assert!(!super::is_clean_turn(2, 1));    // any error → hold/increment, don't reset
        assert!(!super::is_clean_turn(0, 1));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p alephcore harness::agent::tests::failure_streak`
Expected: FAIL — `cannot find function is_failure_turn` / `is_clean_turn`.

- [ ] **Step 3: Add the two pure helpers** in `src/harness/agent.rs` (module-level free fns, near the top below the consts):

```rust
/// A "failure turn" for the consecutive-failure watchdog: the turn made no net
/// progress because failures outnumber successes. Pure structural count — no
/// judgment about whether a *successful* call was actually useful (R10-safe).
const fn is_failure_turn(executed: usize, errors: usize) -> bool {
    errors > executed
}

/// A "clean turn" resets the streak: zero tool errors this turn. An interleaved
/// single success no longer zeroes a churning failure streak — only a fully
/// clean turn does.
const fn is_clean_turn(_executed: usize, errors: usize) -> bool {
    errors == 0
}
```

- [ ] **Step 4: Rewrite the failure-counting block** in `agent.rs`. Replace lines 541-593 (the `if executed == 0 && !vetoed { … } else if executed > 0 { consecutive_failure_turns = 0; }` block) with a version that counts errors and applies the new rules. The current block only looks at `had_failure` when `executed == 0`; the new block counts errors for ALL turns:

```rust
                    // Consecutive-failure watchdog (structural). Count this
                    // turn's tool errors vs successful executions over the
                    // events since the last assistant message.
                    if !vetoed {
                        let events = self
                            .deps
                            .session
                            .get_events(&current_session, None, None)
                            .await
                            .map_err(HarnessError::Session)?;
                        let last_assistant_idx = events
                            .iter()
                            .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
                            .unwrap_or(0);
                        let errors = events[last_assistant_idx..]
                            .iter()
                            .filter(|r| matches!(r.event, SessionEvent::ToolError { .. }))
                            .count();
                        if is_clean_turn(executed, errors) {
                            consecutive_failure_turns = 0;
                        } else if is_failure_turn(executed, errors) {
                            consecutive_failure_turns = consecutive_failure_turns.saturating_add(1);
                            if let Some(cap) = self.deps.consecutive_failure_cap {
                                // Soft landing: one turn before the hard cap,
                                // inject a synthetic reminder so the model can
                                // self-correct or wrap up (mirrors the G1
                                // max-steps hint). Structural trigger, R10-safe.
                                if cap > 1 && consecutive_failure_turns == cap - 1 {
                                    let warn = SessionEvent::UserMessage {
                                        turn_id: uuid::Uuid::new_v4(),
                                        content: MessageContent {
                                            text: SOFT_FAILURE_WARNING.to_string(),
                                            blocks: Vec::new(),
                                            thinking: None,
                                            thinking_signature: None,
                                        },
                                        at: crate::session::events::now_ms(),
                                        synthetic: true,
                                    };
                                    if let Err(e) = self
                                        .deps
                                        .session
                                        .emit_event(&current_session, warn)
                                        .await
                                    {
                                        tracing::warn!(?current_session, ?e, "soft-failure warning emit failed");
                                    }
                                }
                                if consecutive_failure_turns >= cap {
                                    tracing::warn!(
                                        ?current_session,
                                        cap,
                                        "consecutive failure cap reached; forcing Done",
                                    );
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    self.set_terminate_reason(
                                        TerminateReason::ConsecutiveFailureCap {
                                            consecutive: consecutive_failure_turns
                                                .try_into()
                                                .unwrap_or(u32::MAX),
                                        },
                                    );
                                    self.fire_boundary_grace_turn(
                                        &current_session,
                                        callback,
                                        iterations,
                                        crate::harness::agent::think::GraceReason::ConsecutiveFailureCap,
                                        cancel,
                                    )
                                    .await;
                                    callback.on_complete();
                                    break Ok(
                                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                                    );
                                }
                            }
                        }
                        // else: minority-failure turn — neither reset nor
                        // increment (hold the streak).
                    }
```

> Note: this replaces BOTH the old `if executed == 0 …` branch AND the old `else if executed > 0 { consecutive_failure_turns = 0; }` branch — delete both; the new block subsumes them and now runs for every non-vetoed turn (it self-resets on clean turns).

- [ ] **Step 5: Add the warning const** near the other `agent.rs` consts:

```rust
/// Soft-landing reminder injected one turn before the consecutive-failure cap
/// fires. Gives a weak model a final chance to change approach or wrap up
/// before the hard stop. The model writes the user-facing text (R7).
const SOFT_FAILURE_WARNING: &str = "<system-reminder>\nRepeated tool failures \
detected. You are one step from the safety cap stopping this run. Either change \
your approach now (different tool, arguments, or strategy), or stop calling \
tools and summarize for the user what you attempted and what is blocking you.\n\
</system-reminder>";
```

Ensure `MessageContent` and `SessionEvent` are in scope (they are already used elsewhere in `agent.rs`; if `MessageContent` is not imported, add `use crate::session::events::MessageContent;` — verify against existing imports first).

- [ ] **Step 6: Run the helper tests + compile-check**

Run: `cargo test -p alephcore harness::agent::tests::failure_streak`
Then: `cargo check -p alephcore --lib`
Expected: helper tests PASS; crate compiles.

- [ ] **Step 7: Commit**

```bash
git add src/harness/agent.rs
git commit -m "harness: majority-failure streak counting + pre-cap soft landing"
```

---

## Task 11: Kimi/Minimax anthropic-primary presets

**Files:**
- Modify: `src/providers/presets/registry.rs`

**Interfaces:**
- Consumes: existing `ProviderPreset::new` + `with_*` builders.

> Flip the primary `minimax` and `moonshot` presets to their anthropic-compatible endpoints (already recognized by `anthropic/provider_policy.rs`); keep the OpenAI endpoints as `-openai` secondaries. The vendor-identity table (Task 1) governs them as `strict` regardless of protocol — verify that here.

- [ ] **Step 1: Write the failing tests** — append to `#[cfg(test)] mod tests` in `src/providers/presets/registry.rs` (it uses `PRESETS` / `get_preset`; match the existing test style for looking up an entry):

```rust
    #[test]
    fn minimax_primary_is_anthropic_endpoint() {
        let p = PRESETS.get("minimax").expect("minimax preset");
        assert_eq!(p.protocol, "anthropic");
        assert!(p.base_url.contains("minimaxi.com/anthropic"), "{}", p.base_url);
        let openai = PRESETS.get("minimax-openai").expect("minimax-openai secondary");
        assert_eq!(openai.protocol, "openai");
    }

    #[test]
    fn moonshot_primary_is_anthropic_endpoint() {
        let p = PRESETS.get("moonshot").expect("moonshot preset");
        assert_eq!(p.protocol, "anthropic");
        assert!(p.base_url.contains("moonshot.cn/anthropic"), "{}", p.base_url);
        let openai = PRESETS.get("moonshot-openai").expect("moonshot-openai secondary");
        assert_eq!(openai.protocol, "openai");
    }

    #[test]
    fn kimi_minimax_govern_as_strict_regardless_of_protocol() {
        // Regression guard: switching the default to the anthropic protocol must
        // NOT loosen governance — vendor_identity still resolves "strict".
        use crate::providers::model_behaviors::vendor_identity;
        assert_eq!(vendor_identity(Some("https://api.moonshot.cn/anthropic"), "kimi-k2-0905-preview"), Some("strict"));
        assert_eq!(vendor_identity(Some("https://api.minimaxi.com/anthropic"), "MiniMax-M2.5"), Some("strict"));
    }
```

> Check the exact lookup API first: if entries are accessed via a helper like `get_preset("minimax")` rather than `PRESETS.get(...)`, use that helper in the asserts (grep `fn get_preset` / how `PRESETS` is typed in `registry.rs`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p alephcore providers::presets::registry::tests::minimax_primary -p alephcore`
Expected: FAIL — `minimax` is still `openai`/`api.minimax.io/v1`; `minimax-openai` missing.

- [ ] **Step 3: Edit the `minimax` entry** (lines 249-258). Change the primary to anthropic and add a secondary. Replace:

```rust
        "minimax",
        ProviderPreset::new(
            "https://api.minimax.io/v1",
            "openai",
            // …color…
            "MiniMax-M2.5",
        )
        .with_display("MiniMax")
        .with_homepage("https://www.minimax.io")
        .with_signup("https://www.minimax.io"),
```

with (keep the original color literal that is in the file):

```rust
        "minimax",
        ProviderPreset::new(
            "https://api.minimaxi.com/anthropic",
            "anthropic",
            "#ff5a5f",
            "MiniMax-M2.5",
        )
        .with_display("MiniMax")
        .with_description("Anthropic-compatible endpoint (recommended)")
        .with_homepage("https://www.minimax.io")
        .with_signup("https://www.minimax.io"),
    ),
    (
        "minimax-openai",
        ProviderPreset::new(
            "https://api.minimax.io/v1",
            "openai",
            "#ff5a5f",
            "MiniMax-M2.5",
        )
        .with_display("MiniMax (OpenAI endpoint)")
        .with_description("OpenAI-compatible endpoint")
        .with_homepage("https://www.minimax.io")
        .with_signup("https://www.minimax.io"),
```

- [ ] **Step 4: Edit the `moonshot` entry** (lines 149-167). Flip the primary to the CN anthropic endpoint, keep `kimi` alias, add the OpenAI secondary. Replace the `"moonshot"` entry's first two `ProviderPreset::new` args (`"https://api.moonshot.ai/v1", "openai"`) → (`"https://api.moonshot.cn/anthropic", "anthropic"`), add `.with_temperature_policy(super::TemperaturePolicy::Omit)` (Kimi server-manages temperature) and `.with_description("Anthropic-compatible endpoint (recommended)")`, and append a new secondary entry right after it:

```rust
    (
        "moonshot-openai",
        ProviderPreset::new(
            "https://api.moonshot.ai/v1",
            "openai",
            "#16b3a6",
            "kimi-k2-0905-preview",
        )
        .with_aliases(&["kimi-openai"])
        .with_display("Moonshot / Kimi (OpenAI endpoint)")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_description("OpenAI-compatible Kimi K2 / Moonshot chat models")
        .with_fallback_models(&[
            "kimi-k2-0905-preview",
            "kimi-k2-turbo-preview",
            "kimi-latest",
            "moonshot-v1-128k",
            "moonshot-v1-32k",
            "moonshot-v1-8k",
        ]),
    ),
```

> Keep the existing `moonshot-cn` (openai) entry as-is; it remains a valid secondary. Use the exact `#color` literals already present in the file for `moonshot` (do not invent — copy from the current entry).

- [ ] **Step 5: Verify pricing canonicalization** — grep `canonical_provider_id` and confirm `minimax-openai` / `moonshot-openai` map back to `minimax` / `moonshot` for pricing (`src/pricing.rs:571` keys MiniMax rates on `"minimax"`). If `canonical_provider_id` strips a known suffix or uses a prefix match, the `-openai` secondaries already canonicalize; if it is an exact-match table, add `"minimax-openai" => "minimax"` and `"moonshot-openai" => "moonshot"` entries. Document what you found in the commit body.

Run: `grep -n "fn canonical_provider_id" -A30 src/pricing.rs`

- [ ] **Step 6: Run the preset tests + compile-check**

Run: `cargo test -p alephcore providers::presets`
Then: `cargo check -p alephcore --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/providers/presets/registry.rs src/pricing.rs
git commit -m "providers: Kimi/Minimax anthropic-primary presets (OpenAI as -openai secondary)"
```

---

## Self-Review

**Spec coverage** (each spec section → task):
- §2/§3a resolver collapse → Task 4 + Task 5.
- §3a `behavior_hint` (the model-id-availability refinement) → Task 2.
- §3b vendor_identity (id ∪ domain) → Task 1.
- §3c strict family + robustness → Task 1 (content) + Task 3 (profile).
- §3d re-key layer + delta threading → Task 6 + Task 7.
- §3e L2 elicitation content → Task 7 (`.md` deltas).
- §4a timeout grace → Task 8 + Task 9.
- §4b soft landing → Task 10.
- §4c consecutive-failure fix → Task 10.
- §5 WS3 presets → Task 11.
- §6 R10 self-check → Global Constraints + no-new-harness-file/layer structure.

**Refinements vs spec (documented):** (1) identity resolves via `AiProvider::behavior_hint()` on the provider (from `base_url`) rather than threading `model_id` through the bridge — the bridge's common `pick_llm` path has no clean model-id string, so the spec's flagged "待确认" risk is resolved by provider self-identification, which also works in every path. (2) `provider_protocol` is *renamed* to `behavior_name` (single consumer) rather than adding a parallel field. (3) Per the byte-compat constraint, the two SHARED baseline consts stay; only the per-family tails move to `.md` (plus new L2) — this keeps the existing baseline output stable while making the growing content data-driven.

**Type consistency:** `vendor_identity(Option<&str>, &str) -> Option<&'static str>` (Task 1) — same signature in Task 2 (`HttpProvider`) and Task 11 tests. `behavior_hint() -> Option<Cow<'_, str>>` (Task 2) consumed by `resolve_behavior` (Task 4). `resolve_behavior(&dyn AiProvider) -> Cow<'static, str>` consumed by Task 5 (`for_behavior(Some(&behavior_name))`) and Task 6 (`with_behavior_name(.into_owned())` + `load_model_behavior(&behavior_name).await`). `LayerInput.{behavior_name, model_behavior_delta}` + `with_behavior_name_opt` / `with_model_behavior_delta_opt` (Task 6) consumed by Task 7. `GraceReason::Timeout` (Task 8) consumed by Task 9. `is_failure_turn` / `is_clean_turn` (Task 10) match their tests.

**Placeholder scan:** none — every code step shows full code; the two grep-verify steps (Task 5 unused-import, Task 11 `canonical_provider_id`) are explicit verification actions, not deferred work.
