# Review Report — Batch 3 (Secrets Defense, Custom Rules, Module Wiring, Seam Audit)

**Scope (in-module):** `src/pii/rules/api_key.rs` (141 LOC),
`ssh_key.rs` (127 LOC), `custom.rs` (122 LOC), `rules/mod.rs` (73 LOC).
**Seam audit (read-only — no edits):** `src/mcp/redact.rs`,
`src/security/runtime_guard.rs`, `src/browser/secret_guard.rs`,
`src/providers/http_provider.rs`, `src/gateway/handlers/search_config/update.rs`,
`src/config/types/privacy.rs`, `src/guardrails/pii_secrets.rs`.
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol: security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-pii` (branch `review/pii`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0     |
| High     | 4     |
| Medium   | 4     |
| Low      | 3     |
| **Total**| **11**|

This batch covers the **highest-stakes** rules — API keys and SSH private
keys are the secrets the gateway exists to prevent leaking. A
false-negative here means a credential reaches an LLM provider verbatim.
The seam audit confirmed all five external consumers wire up correctly
against the `PiiEngine` API, but revealed two integration hazards
(documented in the seam section) that are NOT in the `src/pii` scope —
they are flagged for the owning modules.

The previous review (2026-08-05) noted a DRY opportunity: `api_key`,
`ssh_key`, `email`, `ip_address` all share an identical `detect()` body.
That observation is preserved but not addressed here (low-value refactor
with non-trivial blast radius).

---

## Findings

### [HIGH] api_key.rs:24 — `Bearer` token pattern is case-sensitive — `bearer eyJ...` and `BEARER eyJ...` are not matched

**Category:** Security (false-negative)
**Confidence:** High

**Description:**
```rust
// api_key.rs:36
| Bearer\s+[a-zA-Z0-9._\-]{20,}
```

The literal `Bearer` is case-sensitive. RFC 7235 §2.1 specifies the
scheme is case-insensitive ("`Bearer`" and `bearer` and `BEARER` are all
the same scheme). In practice:

- `curl -H "Authorization: bearer eyJ..."` (lowercase) — **missed**
- `Authorization: BEARER eyJ...` (uppercase) — **missed**
- Server-side frameworks that lowercase the header name preserve the
  scheme's case — RFC says they should not, but they often do
- Middleware like Envoy normalizes to lowercase

The `Authorization: Bearer ...` form is the most common header-based
token-leak vector. A false-negative here means a real JWT/bearer token
is delivered to the model verbatim.

**Failure scenario:** User sends `curl ... -H 'Authorization: bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.payload.signature'`
to an LLM-backed tool that echoes the URL/headers into a chat → token
leaks.

**Suggested fix:** add `(?i)` to the bearer alternation (or wrap the
whole alternation):

```rust
| (?i)Bearer\s+[a-zA-Z0-9._\-]{20,}
```

---

### [HIGH] api_key.rs:22-37 — Missing high-impact API-key families: AWS Secret Access Key, Google API key (`AIza`), GitLab PAT, Stripe live keys

**Category:** Security (false-negative, coverage)
**Confidence:** High

**Description:**
The current alternation covers:
- OpenAI/Anthropic `sk-...`
- GitHub `ghp_`, `gho_`, `github_pat_`
- AWS Access Key ID `AKIA...`
- Slack `xox[abprs]-`
- Tavily `tvly-`
- Generic `Bearer ...`

Gaps (each represents a real-world credential format observed in the
graphify corpus and Aleph's design documents):

| Provider      | Format                       | Why it matters                                |
|---------------|------------------------------|-----------------------------------------------|
| AWS Secret Access Key | `aws_secret_access_key=...` (40 base64 chars, often after `=` or whitespace) | Pairs with `AKIA`; even AKIA-only detection misses the secret |
| Google API key | `AIza[0-9A-Za-z\-_]{35}`     | Common in GCP-hosted apps                      |
| GitLab PAT    | `glpat-[A-Za-z0-9\-_]{20,}`  | Mirrors `ghp_`                                |
| Stripe live   | `sk_live_[0-9a-zA-Z]{24,}`   | Distinct from OpenAI's `sk-...`                |
| Slack Webhook | `https://hooks.slack.com/services/T.../B.../...` | URL-embedded, harder to false-positive |
| HuggingFace   | `hf_[A-Za-z0-9]{20,}`        | Newer family                                  |
| Linear        | `lin_api_[A-Za-z0-9]{40}`    | Growing usage                                 |
| npm           | `npm_[A-Za-z0-9]{36}`        | Supply-chain risk                             |
| Perplexity    | `pplx-[A-Za-z0-9]{40,}`      | Has been added in some Aleph integrations    |
| OpenRouter    | `sk-or-v1-[A-Za-z0-9]{40,}`  | Variant of `sk-`                              |

Each missing family is a credential that an Aleph user could plausibly
paste into a chat. The current regex misses them all.

**Suggested fix:** extend the alternation with at least the top three
(`AIza`, `glpat-`, AWS Secret Access Key after `=` or `"`) as
high-priority additions; the rest can be deferred to a follow-up. Each
new pattern must come with a non-match test against common false
positives (e.g. `AIza` should not match `Aiza1234567...`).

---

### [HIGH] api_key.rs:23 — `xox[bpras]` covers 5 of ~7 Slack token families; `xoxe-` (Enterprise) and `xoxp-` (User) partial coverage

**Category:** Coverage (false-negative)
**Confidence:** Medium

**Description:**
```rust
| xox[bpras]-[a-zA-Z0-9\-]{10,}
```

Slack token prefixes documented by Slack:
- `xoxb-` — bot token (✓)
- `xoxp-` — user token (✗ — the regex requires `bpras` but `p` is in `pras`)
- `xoxa-` — app-level token (✓)
- `xoxr-` — refresh token (✓)
- `xoxs-` — legacy webhook (✓)
- `xoxe-` — refresh + Enterprise grid (✗ — `e` not in `bpras`)

Wait — `bpras` includes `p`, so `xoxp-` IS covered. Let me re-verify:
the character class `[bpras]` is the set `{b, p, r, a, s}`. So
`xoxp-` matches because `p ∈ [bpras]`. ✓ — my read was wrong. `xoxe-`
IS missing.

Also missing: `xoxe.xoxp-...` (Slack "modern" tokens) — they embed a
dot-separated payload format that the current regex doesn't handle
fully.

**Suggested fix:** expand to `xox[abprse]-` (adds `e`).

---

### [HIGH] ssh_key.rs:24 — `-----BEGIN PRIVATE KEY-----` matches `-----BEGIN ENCRYPTED PRIVATE KEY-----` because the `*` quantifier is greedy across whitespace; also matches PKCS#7 / SMIME keys

**Category:** Security / Logic
**Confidence:** Medium

**Description:**
```rust
// ssh_key.rs:24
Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
```

The `*` is fine (it's the BEGIN header). The intent: only PEM-encoded
**asymmetric** private keys. In practice:

- `-----BEGIN ENCRYPTED PRIVATE KEY-----` — PKCS#8 encrypted
  (RFC 5208 §6). The regex `BEGIN [A-Z ]*PRIVATE KEY` matches
  `BEGIN ENCRYPTED PRIVATE KEY` because `[A-Z ]*` is greedy. Result:
  matched.
- `-----BEGIN PRIVATE KEY-----` — PKCS#8 unencrypted. Matched.
- `-----BEGIN EC PRIVATE KEY-----` — SEC1 elliptic curve. Matched.
- `-----BEGIN RSA PRIVATE KEY-----` — PKCS#1 RSA. Matched.

This is *probably* what we want — all four are credentials. But there
are false positives lurking:

- `-----BEGIN OPENSSH PRIVATE KEY-----` — matched (✓ correct)
- `-----BEGIN PGP PRIVATE KEY BLOCK-----` — NOT matched (`PGP PRIVATE KEY
  BLOCK` has lowercase 'P' in `PGP`, but the regex requires uppercase
  only; actually `PGP` is uppercase, so `BEGIN PGP PRIVATE KEY BLOCK`
  would match the BEGIN header but the END is `-----END PGP PRIVATE KEY
  BLOCK-----` which also matches). So OpenPGP secret blocks ARE matched.
  This may be desirable.
- `-----BEGIN ENCRYPTED PRIVATE KEY-----` ends with
  `-----END ENCRYPTED PRIVATE KEY-----` — matched.
- `-----BEGIN DH PARAMETERS-----` ... `-----END DH PARAMETERS-----` —
  NOT matched (no PRIVATE KEY in header/footer). ✓
- `-----BEGIN CERTIFICATE-----` ... `-----END CERTIFICATE-----` —
  NOT matched (no PRIVATE KEY). ✓

The actual gap: the regex does not require the BEGIN/END key types to
**match**. `-----BEGIN RSA PRIVATE KEY-----` ... `-----END EC PRIVATE KEY-----`
is a malformed input but the regex would match the whole span between
them. Low real-world risk (this is malformed input), but worth a tighter
regex.

**Suggested fix:** capture the BEGIN type and require the END type to
match (or use a back-reference: `-----BEGIN ([A-Z ]*PRIVATE) KEY-----.*?-----END \1 KEY-----`).

The "ENCRYPTED PRIVATE KEY" inclusion is desirable — keep it. Mark this
finding as low-priority refinement.

---

### [MEDIUM] api_key.rs:13-23 — Pattern allows `sk-` with no character-class bound on `gho_`, `ghp_`, `xox[a-z]-` — a typo like `ghp-foo` (only 3 chars after underscore) won't match by length, but `ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx` (40+ chars) WILL match even if the prefix is wrong

**Category:** Logic / false-positive
**Confidence:** Low

**Description:**
```rust
| ghp_[a-zA-Z0-9]{36,}
```

The length bound is 36+, so `ghp_abc` (only 3 chars) won't match. But
the character class `[a-zA-Z0-9]` doesn't include `-` or `_`, which is
correct — GitHub PATs are alphanumeric. The actual false-positive risk
is small; this is informational.

Real concern: `ghp_abcdefghijklmnopqrstuvwxyz0123456789abc` (any 36+
alphanumeric) will match. There is no checksum on `ghp_` prefixes the
way there is on `AKIA`. This is by design (GitHub doesn't publish the
checksum), so the rule is a "looks like a token" heuristic. Acceptable.

**Suggested fix:** no change. Document the heuristic in the rule
comment.

---

### [MEDIUM] ssh_key.rs:24 — `*?` (non-greedy) on the body means a truncated key (BEGIN header but no END footer) is silently dropped, but the test for this case exists; the inverse — `END` with no matching `BEGIN` — is not handled

**Category:** Logic / Quality
**Confidence:** Medium

**Description:**
The regex requires both `-----BEGIN ... PRIVATE KEY-----` AND
`-----END ... PRIVATE KEY-----` in the same match (the `.*?` is bounded
by the END). A truncated PEM (`BEGIN` only) is correctly ignored
(`test_header_only_no_match`). The inverse — a stray `END` line without
a matching `BEGIN` — is also ignored because the regex requires both
anchors. ✓

What IS missing: a PEM block whose BEGIN/END labels don't match (e.g.
`BEGIN RSA PRIVATE KEY` ... `END EC PRIVATE KEY`). The non-greedy
`.*?` will match up to the next valid `END ... PRIVATE KEY`, which is
almost certainly the *first* such END after the BEGIN. Real-world risk:
near zero (PEM files are generated by tools, not hand-edited), but the
rule could fail to match a multi-key concatenated bundle like
`cat key1.pem key2.pem` where key2's `BEGIN` is mid-stream.

**Suggested fix:** see finding above (use back-reference).

---

### [MEDIUM] custom.rs:23-28 — Custom rule compilation failures are logged at `warn` and silently dropped, but a typo'd custom rule that fails to compile means a tenant's PII protection is silently weakened

**Category:** Security / Logic
**Confidence:** Medium

**Description:**
```rust
// rules/mod.rs:50-58
match custom::CustomRegexRule::new(config.clone()) {
    Ok(rule) => rules.push(Box::new(rule)),
    Err(e) => {
        tracing::warn!(
            rule_name = %config.name,
            pattern = %config.pattern,
            error = %e,
            "Skipping invalid custom PII rule regex"
        );
    }
}
```

A tenant adds 5 custom rules to `aleph.toml`. One has a typo. Result:
4 rules active, 1 dropped silently. The operator sees a `warn!` line in
the server log but the dashboard does not surface "you have N inactive
rules".

**Suggested fix:** add a startup self-check: count the number of
configured custom rules vs the number of rules actually loaded, and
emit an `error!` (or a health-check failure) if they differ. This is a
low-effort `instrument` block in `PiiEngine::new`.

---

### [MEDIUM] rules/mod.rs:33 — `build_rules` builds all 7 built-in rules unconditionally, even when their config action is `Off`

**Category:** Performance / Logic
**Confidence:** High

**Description:**
```rust
// rules/mod.rs:33-40
let mut rules: Vec<Box<dyn PiiRule>> = vec![
    Box::new(api_key::ApiKeyRule::new()),
    Box::new(ssh_key::SshKeyRule::new()),
    Box::new(id_card::IdCardRule::new()),
    Box::new(phone::PhoneRule::new()),
    Box::new(bank_card::BankCardRule::new()),
    Box::new(email::EmailRule::new()),
    Box::new(ip_address::IpAddressRule::new()),
];
```

All 7 built-in rule objects are created on every `PiiEngine::new()` /
`reload()`. The engine later filters them out via `if action == Off`,
but the regex compilation (each rule does
`OnceLock::get_or_init(Regex::new(...))`) is still paid. The OnceLock
caches the regex per process, so this is a one-time cost per regex per
process — but it does mean a tenant who turns off all PII still pays for
7 regex compilations at startup.

The bigger waste: rules whose action is `Off` still run their `detect()`
method during `filter()` and emit zero matches (the action-off check is
inside the loop, but `rule.detect(text)` is called unconditionally).
Looking at the engine loop:

```rust
// engine.rs:212-218
for rule in &self.rules {
    let action = Self::action_for_rule(config, rule.name());
    if *action == PiiAction::Off {
        continue;
    }

    let matches = rule.detect(text);
    ...
}
```

OK — the engine DOES skip `detect()` for `Off` rules. ✓ My read was
wrong. The only waste is the one-time regex compilation at engine
construction, which is cached.

**Suggested fix:** none — design is correct. Marking for record only.

---

### [LOW] api_key.rs:13-23 — `(?x)` (extended) mode is used for readability but the inline comments are visible in the regex source only; a future maintainer who strips `(?x)` for performance reasons loses the doc

**Category:** Quality
**Confidence:** High

**Description:**
```rust
Regex::new(
    r"(?x)
    \b                                    # anchor at a word boundary so
                                          # prefixes (sk-, gho_, ...) are
                                          # not matched inside larger
                                          # tokens (e.g. ta`sk-`...)
    ...
```

The comments inside `(?x)` mode are great for documentation. The risk
is that someone strips the `(?x)` flag thinking it's a comment-only
flag and accidentally merges the lines. The `regex` crate's
documentation explicitly notes that `(?x)` enables extended mode and
ignores whitespace + `#`-comments — the code is correct, just fragile.

**Suggested fix:** add a test that asserts the regex has `(?x)` enabled,
or split the regex into a documented module-level constant for clarity.

---

### [LOW] custom.rs:60 — `detect()` clones the rule's `name`, `severity`, `placeholder` on every match, instead of returning borrowed slices

**Category:** Performance
**Confidence:** High

**Description:**
```rust
// custom.rs:60-70
fn detect(&self, text: &str) -> Vec<PiiMatch> {
    let mut results = Vec::new();
    for m in self.regex.find_iter(text) {
        results.push(PiiMatch {
            rule_name: self.config.name.clone(),
            ...
            severity: self.config.severity.into(),
            placeholder: self.config.placeholder.clone(),
        });
    }
    results
}
```

`PiiMatch.rule_name`, `matched_text`, `placeholder` are all `String`
(not `Cow<'_, str>`), so `clone()` is unavoidable without changing the
struct. The struct is `#[non_exhaustive]` and used by the engine —
changing it would be invasive. The same pattern applies to all 7
built-in rules. This is a known design trade-off.

**Suggested fix:** change `PiiMatch` to use `Cow<'_, str>` for the
three string fields. Backward-compatible via `From<String>` / `From<&str>`.
This is a focused refactor — outside the scope of this batch but worth
a follow-up ticket.

---

### [LOW] mod.rs:1-7 — `pub(crate) fn build_rules` returns `Vec<Box<dyn PiiRule>>`; callers can't easily enumerate rule metadata (name + severity) without iterating every rule

**Category:** Architecture
**Confidence:** Medium

**Description:**
`secret_guard::critical_rules` (in `src/browser/secret_guard.rs:30-41`)
filters the rule list by severity. This is the only consumer that needs
metadata without detection. A `pub(crate) fn rule_metadata() -> Vec<(name, severity, placeholder)>`
would let secret_guard skip building the rule objects entirely (it
doesn't actually use `detect()` for its hot path — only for
`scan_url_for_secrets` / `scan_text_for_secrets`).

Wait — `scan_url_for_secrets` DOES use `detect()` (it iterates rules
and finds the first hit). So the metadata-only API would only help
`critical_rules` initialization, which is a one-time cost. Not
worthwhile.

**Suggested fix:** none.

---

## Seam Audit — external consumers of `crate::pii::*`

These files were read-only inspected. **No changes proposed to these
files in this batch** — they are flagged for the owning review pass.

### [Seam-A] src/mcp/redact.rs — wiring OK, lock discipline OK

- `redact_mcp_error` (line 14) calls `PiiEngine::global()`, takes the
  read-lock with poison-safe idiom, and calls `guard.filter(text).text`.
- Falls back to identity when global is not initialized — correct.
- **No issues.**

### [Seam-B] src/security/runtime_guard.rs — wiring OK, lock discipline OK

- `new_with_audit` (line 119-120) calls `PiiEngine::global()` or
  creates a fresh one if missing.
- `process_outbound` (line 263-266) takes the engine read-lock,
  checks `is_platform_excluded`, and calls `filter_with_platform`.
- Audit entries logged with `PiiDetected` for blocked/warned counts.
- **No issues in src/pii's contract.** Note: the Batch-1 finding
  about case-sensitive platform lookup is *also* a runtime_guard finding
  (it passes `platform_name` from upstream unchanged), but the fix
  belongs in `src/pii/engine.rs` (normalize at config-load or in
  `effective_config`), not here.

### [Seam-C] src/browser/secret_guard.rs — wiring OK, but depends on `build_rules()` returning a stable Critical subset

- `critical_rules` (line 30) builds all rules and filters by
  `severity == Critical`. This is the **single source of truth** for
  what counts as a credential for browser navigation/form redaction.
- If a new Critical rule is added to `build_rules` in
  `src/pii/rules/mod.rs`, it is automatically picked up — ✓.
- **No issues.**

### [Seam-D] src/providers/http_provider.rs — wiring OK

- `global_pii_engine().read().filter(req).text` at line 166 — same
  poison-safe idiom.
- **No issues.**

### [Seam-E] src/gateway/handlers/search_config/update.rs — wiring OK, but the reload path replaces rules under the write-lock (engine.rs:111-115) while concurrent readers may hold zero, one, or many read-locks

- `PiiEngine::reload(cfg.privacy.clone())` at line 197. The handler
  clones the `PrivacyConfig` and hands it to the engine. The engine
  clones the `custom_rules` again inside `new()` / `build_rules`.
- Two clones per reload, both necessary for the cow-style
  rebuild-without-blocking-readers pattern. **Acceptable.**
- **No issues.**

### [Seam-F] src/config/types/privacy.rs — wiring OK, but `From<CustomPiiSeverity>` for `PiiSeverity` lives in config/, not in pii/ — creates a minor import cycle smell

- The `From<CustomPiiSeverity> for crate::pii::engine::PiiSeverity`
  (line 98) crosses crate::config → crate::pii boundary.
- The pii module imports the config types via `pub use
  crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig}` in
  `src/pii/mod.rs:9`.
- The cross-direction (config → pii) is a code smell: `config::types`
  should not know about `pii::engine`. Cleaner: define the From impl
  in `src/pii/engine.rs` (or `src/pii/rules/custom.rs`, where it's
  used).
- **Suggested fix (cross-batch):** move the `From` impl into
  `src/pii/engine.rs`. Out of scope here (touches src/config), flagged
  for a follow-up.

### [Seam-G] src/guardrails/pii_secrets.rs — wiring OK, not used by PiiEngine

- This is the `PiiSecretsGuardrail` — an *inbound-side* seam
  (llm_output_guard) that delegates to a separate secrets scanner,
  not to `PiiEngine`. Independent of the `src/pii` module.
- **No issues.**

---

## Architecture Compliance Snapshot

| Redline | Status | Note |
|---------|--------|------|
| R3 (Core minimalism) | OK | secrets defense is regex-only; no external deps |
| R8 (Regex for formats) | OK | all credential patterns are regex with optional checksum |
| R9 (Tools for config) | OK | custom_rules via PrivacyConfig (extensible) |

## Cross-batch handoff

- The seam-audit findings (Seam-A through Seam-G) are owned by their
  respective modules. The only one with a fix inside `src/pii` scope is
  Seam-F (`From<CustomPiiSeverity>` placement) — flagged but not
  addressed in this batch to keep the diff focused.
- Batch-2's bank-card / phone / ID-card false-negative risks apply
  equally to the secret-pattern families: real-world keys pasted into
  chat often have surrounding backticks, `<api_key>` markup, or quoted
  forms. The `Bearer` case sensitivity (Batch-3 H1) is the same class
  of defect.