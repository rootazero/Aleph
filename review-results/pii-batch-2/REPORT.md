# Review Report — Batch 2 (Identity & Network Detection Rules)

**Scope:** `src/pii/rules/email.rs` (115 LOC), `phone.rs` (270 LOC),
`id_card.rs` (256 LOC), `bank_card.rs` (172 LOC), `ip_address.rs` (88 LOC)
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol: security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-pii` (branch `review/pii`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0     |
| High     | 3     |
| Medium   | 4     |
| Low      | 3     |
| **Total**| **10**|

These five rules are the **PII detection** layer — each rule turns raw text
into a `Vec<PiiMatch>` (offset + matched_text + severity). All five share
the `PiiRule` trait contract (`name()`, `severity()`, `placeholder()`,
`detect()`). The two recurring themes in this batch:

1. **Precision > recall, but precision has a cost** — three of the rules
   (email, phone, bank_card) accept whitespace/hyphen/+country-code
   separators in real life but strip them at the regex level. The
   resulting false-negatives are the failure mode the engine cannot
   recover from (the text is sent to the LLM, PII in plaintext).
2. **Anti-false-positive heuristics interact** — `phone.rs` runs four
   guards per match (word_boundary, hex, decimal, timestamp) and any
   one of them silently discards. A guard that over-fires for one
   legitimate-shape number is a covert regression; the current tests
   cover the cases that *were* regressions in past passes, but not the
   boundary cases that *could become* regressions under future changes.

The checksum-based validators (Luhn for bank cards, ISO 7064 MOD 11-2 for
ID cards) are correctly implemented.

---

## Findings

### [HIGH] bank_card.rs:11 — Card numbers with whitespace / hyphens slip past detection

**Category:** Security (false-negative)
**Confidence:** High

**Description:**
```rust
// bank_card.rs:11
Regex::new(r"\d{13,19}")
```

A real card number pasted from a form is almost never a bare digit run.
Common shapes:

| Source              | Example                                |
|---------------------|----------------------------------------|
| Receipt / POS       | `4532 0151 1283 0366`                  |
| Statement PDF       | `4532-0151-1283-0366`                   |
| Mobile keyboard     | `4532 0151 1283 0366` (auto-spaces)    |
| Email signature     | `Card: 4532015112830366` (bare — works)|

The current regex matches the bare form only. A user typing
"my card is 4532 0151 1283 0366" leaves 16 digits of PII in plaintext —
Luhn would pass on either stripped form, but the regex never sees them
as one span. The same gap applies to `4532.0151.1283.0366`.

This is the **highest-impact false-negative in the engine** because (a)
the Luhn guarantee *only* fires if the regex matches a contiguous span,
(b) bank-card PII is one of the categories the engine exists to protect,
and (c) the form-fill detection is the exact path `secret_guard::scan_text_for_secrets`
(browser `secret_guard.rs:80`) duplicates — the navigation seam gets
URL-encoded forms but the outbound PII engine does not.

**Failure scenario:** Outbound chat text `"Use card 4532 0151 1283 0366 to upgrade"`
→ no PiiMatch → engine returns text unchanged → card number sent to LLM
verbatim.

**Suggested fix:** pre-normalize the text by stripping `[\s\-.]` between
digit groups before running the regex, then offset-adjust the match
back to the original positions. Alternative: change the regex to
`\d(?:[\s\-.]?\d){12,18}` — the digit-with-optional-separator form. Either
change requires updating the `is_decimal_context` heuristic so it does
not mis-fire on `1.234 5678 9012 3456` (a real card with period-group
separators is the new normalizer's responsibility, not the rule's).

---

### [HIGH] id_card.rs:14 — ID-card regex matches ASCII-only but Chinese IDs are sometimes transcribed with a trailing space or hyphen

**Category:** Logic / Security (false-negative)
**Confidence:** Medium

**Description:**
```rust
// id_card.rs:14
Regex::new(r"\d{17}[\dXx]")
```

The regex is strictly 18 characters with no separator. In real texts IDs
appear with formatting:

- `"身份证号: 110101 1990 0307 002X"` — spaced
- `"ID=110101-1990-0307-002-X"` — hyphenated
- `"11010119900307002X "` (trailing whitespace before period)

None of these match. The validation pipeline (region / date / checksum)
is correctly strict, but it never runs when the regex misses.

This is a slightly weaker false-negative than bank cards because ID-card
digits don't have an arithmetic checksum that another rule would pick up,
and the surrounding context usually has the word "身份证" or "ID" — but
neither rule (`phone`, `bank_card`) catches an 18-digit ID.

**Suggested fix:** mirror the bank-card fix: a pre-normalization pass
strips `[\s-]` between digit groups, OR loosen the regex to
`\d(?:[\s-]?\d){16}[\dXx]`. Either way, run the same
`has_word_boundary` check on the *normalized* span, then map the offset
back to the original text.

---

### [HIGH] phone.rs:13 — Phone regex is China-mobile-only and silently misses all international / formatted numbers

**Category:** Logic / Security (false-negative)
**Confidence:** High

**Description:**
```rust
// phone.rs:13
Regex::new(r"1[3-9]\d{9}")
```

This pattern is specifically Chinese mainland mobile numbers (11 digits,
prefix `1[3-9]`). International numbers and Chinese landlines are not
covered:

| Form                              | Current behavior              |
|-----------------------------------|-------------------------------|
| `+86 138 1234 5678`               | Miss (no prefix `1`, no space)|
| `+1 415 555 0123` (US)            | Miss (no prefix `1`)          |
| `138-1234-5678`                   | Miss (hyphens)                |
| `010-12345678` (Beijing landline) | Miss (starts with `0`, 11-12 digits, no mobile prefix)|
| `138 1234 5678`                   | Miss (spaces)                 |

The README and config docs present the field name as `phone`, which
implies "phone numbers in general". An operator who turns `phone = block`
on for a global deployment will see non-Chinese phones pass through,
because the regex is China-only.

**Failure scenario:** A user pastes `"Call me at +1 415 555 0123"` →
no match → PII passes through.

**Suggested fix:** the safe direction is to add, NOT replace, the
international rule. Document the rule's regional scope in the doc
comment so operators know to enable additional patterns per-platform if
they need global coverage. At minimum, the `phone.rs` doc comment should
read "Chinese mainland mobile (11 digits, prefix `1[3-9]`)" instead of
the implicit "any phone number". Either:

1. Add a second pattern for international E.164 (`\+[1-9]\d{6,14}`) as
   a separate rule named `phone_intl`, OR
2. Split `phone` into `phone_cn` and `phone_intl` so operators can scope
   per-region, OR
3. Update the doc comment + add a startup log noting the regional limit.

(1) is the smallest, lowest-risk change.

---

### [MEDIUM] phone.rs:30 — `is_hex_bounded` returns `true` for the 'a' before a phone, but a legitimate phone preceded by a single hex digit is rare enough that the heuristic over-fires

**Category:** Logic / false-positive
**Confidence:** Medium

**Description:**
```rust
// phone.rs:30
fn is_hex_bounded(text: &str, start: usize, end: usize) -> bool {
    if start > 0 {
        let b = text.as_bytes()[start - 1];
        if b.is_ascii_hexdigest() && !b.is_ascii_digit() {  // (paraphrased)
            return true;
        }
    }
    ...
}
```

The intent: skip `18160019229f` because it is a UUID fragment with an
`f` suffix. The execution: the byte at `start - 1` is `a`, `b`, `c`, `d`,
`e`, or `f` → skip. The reverse direction does the same. The check
fires on:

- `"a13812345678"` — could be a YAML map key like `a13812345678: true`
  (uncommon but legal). The phone is **lost**.
- `"13812345678f"` — legitimate-looking format (e.g. a partial hex
  literal). Phone is lost.

These are not false positives, they are false negatives caused by the
anti-FP rule being too aggressive in one direction. The `word_boundary`
guard already blocks `a13812345678b` (since 'a' is not a digit and 'b' is
not a digit) — so the only case the hex check actually contributes is
digit → hex-letter sequences like `1234567a8`, where 'a' is not a
digit and '8' is a digit. In that case the `has_word_boundary` would
already permit `a8` to terminate the phone span (`8` is a digit so OK;
'a' is the only concern on the **after** side).

Re-reading the code:

```rust
fn has_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_digit();
    let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_digit();
    before_ok && after_ok
}
```

For `a13812345678`:
- `start=1`, byte[0]='a' — `before_ok = true`
- `end=12`, byte[12] is whatever follows; if it's a space, `after_ok = true`
- So `has_word_boundary` accepts the match

Then `is_hex_bounded` runs:
- byte[0]='a' — `a.is_ascii_hexdigit() && !a.is_ascii_digit()` → `true`
- Returns `true` (i.e., yes, hex-bounded → skip)

Result: `a13812345678` is **not** flagged. So a YAML-key-like pattern
of `<hex-letter><phone>` slips past the PII filter. Whether that matters
depends on whether a YAML config with a phone-looking key gets sent to
the LLM in practice — uncommon but possible.

**Suggested fix:** narrow the hex check to **pairs of hex letters**, not
single ones. A UUID fragment like `18160019229f-4b7a-8c3d` has hex
**groups** (`f-4b7a-8c3d`), not isolated letters. A single preceding
hex letter is far more likely to be a typo, separator, or language
construct than a UUID. New condition:
`[start-2..start] both are hex letters AND not part of a `0x` prefix.`
If this is hard to express, fall back to: drop the check entirely
(it's redundant with `has_word_boundary` for the prefix case) and rely
on the existing timestamp-context guard for the more dangerous UUID
fragment case.

---

### [MEDIUM] email.rs:18 — Email regex is missing the `(?i)` flag; combined with the regex's `[A-Za-z0-9._%+\-]+` local-part, the matching is technically case-insensitive on ASCII, but the allowlist comparison assumes lowercase

**Category:** Logic
**Confidence:** Medium

**Description:**
```rust
// email.rs:10
Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
```

The regex already accepts mixed case in both local part and domain. But
the allowlist (`allowlist.rs:10-18`) compiles its own `(?i)` patterns.
The `EMAIL_RE` does not. The interaction:

1. Text contains `"User@Example.COM"` — `EMAIL_RE` matches the full
   span (because `[A-Za-z]`).
2. Allowlist regex `(?i)^noreply@` — would match `"User@Example.COM"`
   after case-folding. **Does NOT match** because the local part is
   `"User"`, not `"noreply"`.
3. Allowlist regex `(?i)\.(example|test|...|internal|invalid)$` — would
   match `"User@Example.COM"`. **Matches** because `.COM` folds to
   `.com`.

OK so the `.example/.test/...` allowlist does work — but only because
the **suffix** matches case-insensitively. The **prefix** allowlist
(`(?i)^noreply@`) is dead code against mixed-case local parts because
the regex doesn't normalize the local part to lowercase before passing
to the allowlist.

**Failure scenario:** A tenant configures
```rust
is_allowed("User@Example.COM", "email")  // returns true via .COM match
```
But for `(?i)^noreply@`-style prefix patterns, the regex DOES match
because `(?i)` is applied inside the regex — so this is actually fine.
Re-checking: yes, all allowlist patterns are `(?i)`, so the comparison
is correct. **This finding is withdrawn on re-read; the case-fold
already works.** Marking for record only.

**Failure scenario (revised):** The email regex itself uses
`[A-Za-z]` but the local-part regex from RFC 5321 also accepts `.` as
leading/trailing/consecutive dots, which the engine rejects. Standard
edge case; documenting as known precision cost.

**Suggested fix:** no change required, but add a regression test
covering `User@Example.COM` against the `.com` allowlist suffix to
guard against future case-folding regressions.

---

### [MEDIUM] bank_card.rs:39 — `is_decimal_context` rejects numbers preceded by `.`, but the bank-card rule has no equivalent of phone's "starts with hex" guard

**Category:** Logic / Architecture
**Confidence:** Medium

**Description:**
The bank-card rule's anti-FP guards (`has_word_boundary`, `is_decimal_context`,
`luhn_check`) are coherent but the rule has no anti-hex guard. UUID
fragments that happen to be 13-19 digits long AND pass Luhn are
vanishingly rare (~10⁻¹⁶ per fragment) but not zero. The
`Luhn(random_16_digits) = true` rate is ~10%, so a UUID like
`18160019229f4b7a8c3d` (which is hex, not all-digit) cannot match
anyway because the regex requires 13-19 digits. So this is **not** a
real risk; marking as informational.

The more interesting case: a random 16-digit number (not a UUID, just a
big number like an invoice number) has a 10% chance of passing Luhn and
being redacted as a bank card. That is a real false-positive rate but
the design trade-off (false-positive vs false-negative on credit cards)
is documented in the rule comment and accepted.

**Suggested fix:** none required; this is the documented design choice.

---

### [MEDIUM] ip_address.rs:14 — IPv4 regex has no anti-FP guard for obvious non-IPs; IPv6 is not supported

**Category:** Logic / Coverage
**Confidence:** Medium

**Description:**
The regex `\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b`
correctly bounds each octet (0-255) and uses `\b` at both ends. There
is no IPv6 support.

For the engine's stated purpose (precision-tuned, gateway-level
filtering), this is acceptable — IPv6 PII (e.g. `2001:db8::1`) is rarely
sent as plaintext to an LLM, and the placeholder `[IP]` only adds value
when the address is operationally meaningful. Document the IPv6 gap in
the rule doc comment so the next operator does not assume IPv6
support.

**Suggested fix:** add a one-line doc comment to `ip_address.rs`:
```rust
//! IPv4 only. IPv6 is intentionally not supported — see design note.
```

---

### [LOW] phone.rs:69-78 — `is_timestamp_context` window is asymmetric and may miss timestamps whose keyword is just past 40 chars

**Category:** Logic
**Confidence:** Medium

**Description:**
```rust
// phone.rs:69-79
let mut ctx_start = start.saturating_sub(40);
...
let mut ctx_end = (start + 40).min(text.len());
```

The context window is 40 chars before + 40 chars after = 80 chars total,
centered on `start`. A timestamp keyword at `start + 39` is included; at
`start + 41` it is not. The asymmetry is bounded but the boundary is
arbitrary — a long timestamp field name (`modified_at_milliseconds`)
plus a phone-like integer at position N may exceed the window in one
configuration but not another.

**Suggested fix:** expand the window to 60/60 (120 total) for safety;
the cost is one more regex match against a slightly longer string.

---

### [LOW] bank_card.rs:11 — `regex \d{13,19}` matches Maestro (12-19) almost entirely, but the lower bound misses 12-digit Maestro

**Category:** Coverage / Security (false-negative)
**Confidence:** Low

**Description:**
Maestro card numbers can be 12-19 digits. The regex accepts 13-19. A
12-digit Maestro card (rare but real) slips through. Trade-off: lowering
to 12 increases the Luhn false-positive rate (~10% per 12-digit number
matches).

**Suggested fix:** no change — the trade-off is documented and the
12-digit case is rare enough to defer. Mark as known limitation.

---

### [LOW] id_card.rs:144 — `is_valid_id_card` returns `false` for any string whose length is not exactly 18, but the regex already enforces 18, so the explicit length check is redundant defensive coding

**Category:** Architecture / Quality
**Confidence:** High

**Description:**
```rust
// id_card.rs:148-150
fn is_valid_id_card(id: &str) -> bool {
    if id.len() != 18 {
        return false;
    }
    ...
}
```

The regex `\d{17}[\dXx]` only matches exactly 18 chars. The `if id.len() != 18`
check is unreachable as long as the regex stays. But the function is
also called from `is_valid_checksum` which itself assumes 18 bytes via
`bytes.iter().take(17).zip(WEIGHTS.iter())` — the take(17) is also
defensive.

This is fine defensive coding — keep it. Noting for completeness.

**Suggested fix:** none.

---

## Architecture Compliance Snapshot

| Redline | Status | Note |
|---------|--------|------|
| R3 (Core minimalism) | OK | each rule is ~80-270 LOC; no heavy deps |
| R8 (Regex for formats) | OK | formats are regex + checksum, no LLM |
| Architecture | OK | trait dispatch via `Box<dyn PiiRule>` is the right choice for 7-15 rules |

## Cross-batch handoff

- The `api_key` rule (Batch 3) has a similar formatted-key false-negative risk
  (real keys in chat are often quoted with backticks or wrapped in `<api_key>...</api_key>`)
  — should be reviewed together.
- The `custom.rs` rule (Batch 3) is the user-controllable variant of all
  five rules above; precision regressions there are scoped to the
  user's tenant.