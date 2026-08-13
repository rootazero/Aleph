# Review Report — Batch 1 (Core Engine, Allowlist, Module Entry)

**Scope:** `src/pii/engine.rs` (562 LOC), `src/pii/allowlist.rs` (82 LOC), `src/pii/mod.rs` (16 LOC)
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol: security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-pii` (branch `review/pii`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0     |
| High     | 3     |
| Medium   | 4     |
| Low      | 4     |
| **Total**| **11**|

The PII engine is the gateway-level privacy gate that scrubs sensitive strings
out of every outbound LLM call. The previous review (2026-08-05, see
`docs/engineering-reports/review-results/pii.md`) already fixed the poisoned-lock
and UTF-8 slicing hazards. This pass covers correctness of the **platform-aware
filter path** and the **allowlist hot path** — the two places where the engine
plumbs new policy features in and where unbounded allocations are easy to miss.

---

## Findings

### [HIGH] engine.rs:140-180 — `effective_config` clones the entire `PrivacyConfig` for every call, even when `platform = None`

**Category:** Performance / Logic
**Confidence:** High

**Description:**
`filter_with_platform` (line 316) always calls `self.effective_config(platform)`,
which begins with:

```rust
// engine.rs:144-145
// rust-doctor-disable-next-line excessive-clone
let mut cfg = self.config.clone();
```

The clone happens unconditionally — even when `platform` is `None` (the
common path) and when the platform policy lookup returns no overrides. The
clone deep-copies `Vec<CustomPiiRule>` (each rule holds a `String` pattern +
`String` placeholder + `String` name), the entire `HashMap<String, PlatformPiiPolicy>`
of platform overrides, and the `Vec<String>` of `exclude_providers`. On every
outbound message this is paid.

This path is exercised on every LLM call by `runtime_guard::process_outbound`
(`security/runtime_guard.rs:266-278`), which holds the engine read-lock while
it pays the clone. The cost is bounded but unnecessary: `filter()` itself
takes `&self.config` directly without cloning.

**Failure scenario:** in production, the global `PiiEngine` holds a config
with N platform overrides and M custom rules; every `filter_with_platform`
call produces an O(N + M) allocation. The cost compounds when the engine is
shared across async tasks.

**Suggested fix:** make the clone conditional on platform overrides being
present, and (better) skip the clone entirely when there is nothing to merge:

```rust
fn effective_config<'a>(&'a self, platform: Option<&'a str>) -> Cow<'a, PrivacyConfig> {
    let p = match platform.and_then(|p| self.config.platform_policies.get(p)) {
        Some(p) => p,
        None => return Cow::Borrowed(&self.config),
    };
    // Only clone if at least one override is set.
    if p.pii_filtering.is_none()
        && p.id_card.is_none() && p.bank_card.is_none()
        && p.phone.is_none() && p.api_key.is_none() && p.ssh_key.is_none()
        && p.email.is_none() && p.ip_address.is_none()
    {
        return Cow::Borrowed(&self.config);
    }
    Cow::Owned(/* apply overrides */)
}
```

`filter_with_config` would then accept `Cow<PrivacyConfig>`.

---

### [HIGH] engine.rs:140-160 — `action_for_rule` is O(M) per match, where M = `custom_rules.len()`

**Category:** Performance
**Confidence:** High

**Description:**
For each detected `PiiMatch`, the engine resolves the rule action:

```rust
// engine.rs:140-160
fn action_for_rule<'a>(config: &'a PrivacyConfig, rule_name: &str) -> &'a PiiAction {
    match rule_name {
        "phone" => &config.phone,
        ...
        _ => {
            config.custom_rules.iter()
                .find(|r| r.name == rule_name)
                .map_or_else(|| &PiiAction::Block, |r| &r.action)
        }
    }
}
```

The built-in path is constant-time. The custom-rule fallback does a linear
scan of `custom_rules` on every call. The engine invokes this **twice per
match** — once during the priority sort (line 228) and once during the
replacement loop (line 264) — making it O(M) per match, where M is the
number of custom rules.

For a text with K matches and M custom rules, the cost is O(K × M). With
the existing 7 built-ins, K is usually small, but M can grow when users
load tenant-specific patterns (the API contract advertises custom rules
in `docs/superpowers/specs/.../4.1_custompiirulessection`).

**Failure scenario:** A tenant loads 50 custom rules; an outbound message
containing 200 PII tokens (e.g. a log dump) makes `action_for_rule` perform
10 000 string comparisons, with the read-lock held the whole time.

**Suggested fix:** precompute a `HashMap<&str, &PiiAction>` keyed by rule
name once when the engine is constructed (and rebuild on `reload`). The
built-ins map to fields and the custom rules map to `r.action`. The
hot-path lookup becomes O(1).

---

### [HIGH] engine.rs:316 + :141 — Platform-key matching is case-sensitive, leaking PII to excluded providers when casing drifts

**Category:** Logic / Security
**Confidence:** High

**Description:**
`is_platform_excluded` and `effective_config` look up the platform policy
by an exact-string `HashMap::get`:

```rust
// engine.rs:148  — effective_config
if let Some(p) = platform {
    if let Some(policy) = self.config.platform_policies.get(p) {
```

```rust
// engine.rs:140  — is_platform_excluded (via self.config.platform_policies.get)
if let Some(p) = platform {
    if let Some(policy) = self.config.platform_policies.get(p) {
        if let Some(ref excluded) = policy.exclude_providers {
            if excluded.iter().any(|e| e.eq_ignore_ascii_case(provider)) {
                return true;
            }
        }
    }
}
```

Provider names are case-insensitive (`is_provider_excluded` uses
`eq_ignore_ascii_case`), but platform names are case-sensitive. If a
config writes `[platform_policies.Telegram]` and the runtime passes
`"telegram"`, the policy is silently skipped — falling back to the global
config. Worse, if Telegram policy says `phone = "off"` (i.e. the platform
asks for relaxed filtering on that platform), the global stricter setting
wins, **blocking** messages that the platform expected to let through.

The reverse is also a defect: an operator who writes the platform key as
`"Discord"` and the runtime passes `"discord"` will see the global config
applied — which can mean PII that the operator deliberately allowed on
Discord is now blocked, breaking the user-visible behavior.

**Failure scenario:** Operator adds
```toml
[platform_policies.telegram]
phone = "off"
exclude_providers = ["local-llm"]
```
expecting local-llm to bypass phone filtering on Telegram. Runtime passes
`platform = Some("Telegram")` (capital T from the upstream enum) → lookup
misses → global `phone = "block"` wins → phone is redacted on Telegram even
though the operator asked otherwise.

**Suggested fix:** normalize platform keys at config-load time (lower-case
once, store as lower-case) so runtime lookups are case-sensitive but
canonical. Add a regression test covering `"Telegram"` vs `"telegram"`.

---

### [MEDIUM] engine.rs:281, 296 — `warn!` per PII match floods logs in normal traffic

**Category:** Quality / Performance
**Confidence:** High

**Description:**
Every Block and every Warn action emits a `tracing::warn!`:

```rust
// engine.rs:281
warn!(rule = %detection.rule_name, severity = %detection.severity,
      "PII detected and blocked before API call");
// engine.rs:296
warn!(rule = %detection.rule_name, severity = %detection.severity,
      "PII detected in outbound message (warn mode)");
```

PII detection can fire on every outbound message — a typical chat contains
1-3 emails or phones. A 100-RPS gateway emits ~200-300 warn lines per
second. Tracing-subscriber file appenders do flush per write, so this is
also an I/O hot spot.

**Suggested fix:** keep the `info`/`warn` for low-frequency events (Warn
mode, high-severity blocks), but demote Block-mode detection to `debug!`,
or sample at 1/N. Alternatively, aggregate per-call: emit one summary
`info!` per `filter()` with `blocked_count` and `warned_count` and stop
emitting per-match lines.

---

### [MEDIUM] allowlist.rs:32-50 — `test_phones()` and `local_ips()` reallocate on every `Default::default()`

**Category:** Performance
**Confidence:** High

**Description:**
```rust
// allowlist.rs:32-37
fn test_phones() -> HashSet<String> {
    [
        "13800138000", "18888888888", "13900001111",
        "13800000000", "15800000000", "18900000000",
    ]
    .iter().map(|s| s.to_string()).collect()
}

// allowlist.rs:44-50
fn local_ips() -> HashSet<String> {
    [
        "127.0.0.1", "0.0.0.0", "192.168.0.1",
        "192.168.1.1", "10.0.0.1", "172.16.0.1",
    ]
    .iter().map(|s| s.to_string()).collect()
}
```

Each `PiiAllowlist::default()` (called from `PiiEngine::new`, which runs at
boot and on every `reload()`) reallocates two `HashSet<String>`s and
clones the system-email regex `Vec` (`allowlist.rs:65`). The engine is
created once per `init()` call but `reload()` (`engine.rs:111-115`)
re-creates only the rules, not the allowlist — so the bigger cost is at
boot. Still, this is a low-effort fix.

**Suggested fix:** mirror the `SYSTEM_EMAIL_PATTERNS: OnceLock<Vec<Regex>>`
pattern — make `TEST_PHONES` and `LOCAL_IPS` `OnceLock<HashSet<String>>`
and borrow them in `Default::default()`. HashSet lookup is also faster on
`&'static str` than on `String` — store the static strings and have
`is_allowed` take `&str`, matching against the borrowed slice.

---

### [MEDIUM] allowlist.rs:65 — `system_email_patterns().clone()` in `Default` deep-clones the regex `Vec`

**Category:** Performance
**Confidence:** High

**Description:**
```rust
// allowlist.rs:65
system_email_patterns: system_email_patterns().clone(),
```

`Regex` is not `Copy`; cloning each one bumps the inner refcount and
re-walks the automaton. With 5 patterns this is small but the pattern
should mirror the `OnceLock<Arc<Vec<Regex>>>` idiom used elsewhere.

**Suggested fix:** store the patterns as `Arc<Vec<Regex>>` in the
`OnceLock`; `PiiAllowlist` keeps an `Arc<Vec<Regex>>` field. Cloning the
allowlist becomes a refcount bump.

---

### [MEDIUM] engine.rs:228-242 — `sort_by_key` + `dedup_overlapping` are correct but the algorithm is two passes over `Vec<PiiMatch>` for a job that can be one

**Category:** Architecture / Performance
**Confidence:** Medium

**Description:**
After collecting all matches, the code sorts by `(blocks, severity)` and
then runs a separate O(N²) dedup loop. With typical N=1-5 per outbound
message this is fine, but the documentation comment claims the sort is
what guarantees the right overlap winner — and a future contributor who
adds a third dimension to the sort key could silently break dedup
without realizing it.

**Suggested fix:** either (a) extract the priority tuple into a named
struct with an explicit contract ("lower number = higher priority = wins
overlaps") and unit-test it independently, or (b) fold dedup into the
priority sort by using a `BTreeMap<(start, Reverse(end))>` keyed by
match range, then collect. (a) is the lower-risk change.

---

### [LOW] engine.rs:97-100 — `PiiEngine::init` second-call path is silent and untested

**Category:** Quality
**Confidence:** High

**Description:**
```rust
// engine.rs:97-100
pub fn init(config: PrivacyConfig) {
    let engine = Arc::new(RwLock::new(Self::new(config)));
    if PII_ENGINE.set(engine).is_err() {
        warn!("PiiEngine already initialized, ignoring duplicate init call");
    }
}
```

If `init()` is called twice (e.g. two test harnesses in the same process,
or a hot-reload path that mistakenly calls `init` instead of `reload`),
the second config is silently dropped with only a warn. There is no test
that exercises the duplicate path.

**Suggested fix:** add a `#[test] fn test_init_idempotent_warns` that
asserts the second init does not change the rules of the global engine.

---

### [LOW] engine.rs:130 — `is_provider_excluded` does a linear scan over `exclude_providers` every call

**Category:** Performance
**Confidence:** High

**Description:**
```rust
// engine.rs:130
self.config.exclude_providers.iter()
    .any(|p| p.eq_ignore_ascii_case(provider_name))
```

`exclude_providers` is small in practice but the same linear scan runs
for every outbound message. For consistency with the engine-construction
hot path, this should be a `HashSet<String>` (case-folded at insert time).

**Suggested fix:** when constructing `PiiEngine`, fold
`exclude_providers` into a `HashSet<String>` of lower-cased entries;
rebuild on `reload`. The check becomes `set.contains(&name.to_ascii_lowercase())`.

---

### [LOW] engine.rs:289 — Replacement loop's `warn!` for "invalid offsets" is dropped silently after the warn

**Category:** Logic / Quality
**Confidence:** Medium

**Description:**
```rust
// engine.rs:267-282
if detection.start < detection.end
    && detection.end <= result.len()
    && result.is_char_boundary(detection.start)
    && result.is_char_boundary(detection.end)
{
    result.replace_range(...);
    blocked_count += 1;
    warn!(...);
} else {
    warn!(rule = %detection.rule_name, start = detection.start, end = detection.end,
          text_len = result.len(),
          "PII match has invalid offsets, skipping replacement");
}
```

The else branch logs but does not increment a counter, so an upstream
audit cannot tell how often this happens. If a bug surfaces where
detection offsets drift (e.g. after a future refactor that mutates
`text` before this loop), the only signal is the warn stream.

**Suggested fix:** add a `skipped_count: usize` to `FilterResult` and
increment it on the else branch. The audit path in `runtime_guard`
already inspects `blocked_count` / `warned_count`; adding `skipped_count`
makes the same surface useful for triage.

---

### [LOW] mod.rs:9-12 — Re-exports reference `crate::config` rather than the local `crate::pii` hierarchy

**Category:** Architecture
**Confidence:** High

**Description:**
```rust
// mod.rs:9
pub use crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig};
```

The three types re-exported here live in `crate::config`. Re-exporting
them from `crate::pii` is convenient but creates a second public
surface for the same types — `crate::pii::PiiAction` and
`crate::config::PiiAction` are now both reachable. The browser
secret_guard uses `crate::pii::{rules::build_rules, PiiMatch, PiiRule, PiiSeverity}`
(not these re-exports), and `runtime_guard` uses
`crate::pii::engine::{FilterResult, PiiEngine}` directly. The re-exports
have **zero external callers** in this repo's source (`rg "use crate::pii::PiiAction" src`
returns no hits).

**Suggested fix:** CUT the re-exports — `pub use` lines are dead code
from a wire-audit perspective. Callers that need the types go through
`crate::config::PiiAction` etc. directly, which is the upstream source.

---

## Architecture Compliance Snapshot

| Redline | Status | Note |
|---------|--------|------|
| R3 (Core minimalism) | OK | engine has no heavy deps; uses `regex`, `OnceLock`, `tracing` |
| R7 (One core, many shells) | OK | PiiEngine is the single chokepoint; all callers go through it |
| R8 (LLM for routing, regex for formats) | OK | all rules are regex + checksum validation |

## Cross-batch handoff

- Batch 2 covers detection precision for `phone`, `id_card`, `bank_card`, `email`, `ip_address` — those findings are independent.
- Batch 3 covers `api_key`, `ssh_key`, `custom`, and the external seam (mcp/redact, runtime_guard, secret_guard, http_provider, search_config). Cross-batch collisions are unlikely.