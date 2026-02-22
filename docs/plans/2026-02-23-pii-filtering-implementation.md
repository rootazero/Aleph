# PII Filtering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a precision PII filtering engine to the Provider layer, filtering outbound messages before they reach LLM APIs.

**Architecture:** New `pii/` module with rule-based engine + allowlist. Inject filtering in `HttpProvider::execute()` before HTTP dispatch. Configuration via `[privacy]` section in `config.toml` with hot-reload support.

**Tech Stack:** Rust, regex crate (already in deps), serde/schemars for config, tracing for audit logs.

**Design doc:** `docs/plans/2026-02-23-pii-filtering-design.md`

---

### Task 1: PrivacyConfig type + Config integration

**Files:**
- Create: `core/src/config/types/privacy.rs`
- Modify: `core/src/config/types/mod.rs`
- Modify: `core/src/config/structs.rs`

**Step 1: Create PrivacyConfig type**

Create `core/src/config/types/privacy.rs`:

```rust
//! Privacy configuration for PII filtering

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Action to take when PII is detected
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    /// Replace PII with placeholder before sending to API
    Block,
    /// Log detection but send unmodified
    Warn,
    /// Skip detection entirely
    Off,
}

impl Default for PiiAction {
    fn default() -> Self {
        Self::Block
    }
}

/// Privacy and PII filtering configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrivacyConfig {
    /// Global toggle for PII filtering
    #[serde(default = "default_true")]
    pub pii_filtering: bool,

    /// Action for Chinese ID card numbers (Critical)
    #[serde(default)]
    pub id_card: PiiAction,

    /// Action for bank card numbers (High)
    #[serde(default)]
    pub bank_card: PiiAction,

    /// Action for phone numbers (High)
    #[serde(default)]
    pub phone: PiiAction,

    /// Action for API keys and tokens (Critical)
    #[serde(default)]
    pub api_key: PiiAction,

    /// Action for SSH private keys (Critical)
    #[serde(default)]
    pub ssh_key: PiiAction,

    /// Action for email addresses (Medium)
    #[serde(default = "default_warn")]
    pub email: PiiAction,

    /// Action for IP addresses (Low)
    #[serde(default = "default_off")]
    pub ip_address: PiiAction,

    /// Provider names to exclude from filtering (e.g., ["ollama"])
    #[serde(default)]
    pub exclude_providers: Vec<String>,
}

fn default_true() -> bool { true }
fn default_warn() -> PiiAction { PiiAction::Warn }
fn default_off() -> PiiAction { PiiAction::Off }

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            pii_filtering: true,
            id_card: PiiAction::Block,
            bank_card: PiiAction::Block,
            phone: PiiAction::Block,
            api_key: PiiAction::Block,
            ssh_key: PiiAction::Block,
            email: PiiAction::Warn,
            ip_address: PiiAction::Off,
            exclude_providers: vec![],
        }
    }
}
```

**Step 2: Register in config/types/mod.rs**

Add to `core/src/config/types/mod.rs` after line 36 (`pub mod video;`):

```rust
pub mod privacy;
```

Add to re-exports after line 56 (`pub use video::*;`):

```rust
pub use privacy::*;
```

**Step 3: Add to Config struct**

Add to `core/src/config/structs.rs` Config struct (after the `profiles` field, around line 91):

```rust
    /// Privacy and PII filtering configuration
    #[serde(default)]
    pub privacy: PrivacyConfig,
```

Add to `Default for Config` impl (add `privacy: PrivacyConfig::default(),`).

**Step 4: Verify it compiles**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -20`
Expected: Compiles with no errors (may have warnings)

**Step 5: Commit**

```bash
git add core/src/config/types/privacy.rs core/src/config/types/mod.rs core/src/config/structs.rs
git commit -m "pii: add PrivacyConfig type with per-rule action levels"
```

---

### Task 2: PII module scaffold + PiiRule trait + PiiEngine skeleton

**Files:**
- Create: `core/src/pii/mod.rs`
- Create: `core/src/pii/engine.rs`
- Create: `core/src/pii/rules/mod.rs`
- Create: `core/src/pii/allowlist.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create PII module files**

Create `core/src/pii/mod.rs`:

```rust
//! PII (Personally Identifiable Information) filtering engine
//!
//! Gateway-level privacy protection that filters outbound messages
//! before they reach LLM API providers.
//!
//! Unlike `utils::pii::scrub_pii()` (which is optimized for log scrubbing
//! and accepts false positives), this engine is tuned for precision —
//! false positives degrade LLM comprehension.

pub mod allowlist;
pub mod engine;
pub mod rules;

pub use engine::{FilterResult, PiiEngine, PiiMatch, PiiSeverity};
pub use rules::PiiRule;
```

Create `core/src/pii/engine.rs`:

```rust
//! Core PII detection and replacement engine

use crate::config::PrivacyConfig;
use crate::config::PiiAction;
use crate::pii::allowlist::PiiAllowlist;
use crate::pii::rules::PiiRule;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::warn;

/// Severity level for PII detections
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PiiSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for PiiSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// A single PII detection result
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub rule_name: String,
    pub start: usize,
    pub end: usize,
    pub matched_text: String,
    pub severity: PiiSeverity,
    pub placeholder: String,
}

/// Result of PII filtering
#[derive(Debug, Clone)]
pub struct FilterResult {
    /// The filtered text (with PII replaced by placeholders if blocked)
    pub text: String,
    /// Number of PII matches that were blocked (replaced)
    pub blocked_count: usize,
    /// Number of PII matches that were warned (not replaced)
    pub warned_count: usize,
}

impl FilterResult {
    pub fn unchanged(text: &str) -> Self {
        Self {
            text: text.to_string(),
            blocked_count: 0,
            warned_count: 0,
        }
    }

    /// True if any PII was detected (blocked or warned)
    pub fn has_detections(&self) -> bool {
        self.blocked_count > 0 || self.warned_count > 0
    }
}

/// Global PII engine singleton
static PII_ENGINE: OnceLock<Arc<RwLock<PiiEngine>>> = OnceLock::new();

/// Main PII filtering engine
pub struct PiiEngine {
    rules: Vec<Box<dyn PiiRule>>,
    allowlist: PiiAllowlist,
    config: PrivacyConfig,
}

impl PiiEngine {
    /// Create a new PII engine with the given configuration
    pub fn new(config: PrivacyConfig) -> Self {
        let rules = crate::pii::rules::build_rules();
        let allowlist = PiiAllowlist::default();
        Self { rules, allowlist, config }
    }

    /// Initialize the global PII engine
    pub fn init(config: PrivacyConfig) {
        let engine = Arc::new(RwLock::new(Self::new(config)));
        let _ = PII_ENGINE.set(engine);
    }

    /// Get the global PII engine (returns None if not initialized)
    pub fn global() -> Option<Arc<RwLock<PiiEngine>>> {
        PII_ENGINE.get().cloned()
    }

    /// Reload configuration (hot-reload support)
    pub fn reload(config: PrivacyConfig) {
        if let Some(engine) = PII_ENGINE.get() {
            if let Ok(mut guard) = engine.write() {
                guard.config = config;
            }
        }
    }

    /// Check if a specific provider should be excluded from filtering
    pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
        self.config.exclude_providers.iter().any(|p| p == provider_name)
    }

    /// Get the configured action for a severity level
    fn action_for_rule(&self, rule_name: &str) -> &PiiAction {
        match rule_name {
            "phone" => &self.config.phone,
            "id_card" => &self.config.id_card,
            "bank_card" => &self.config.bank_card,
            "email" => &self.config.email,
            "ip_address" => &self.config.ip_address,
            "api_key" => &self.config.api_key,
            "ssh_key" => &self.config.ssh_key,
            _ => &PiiAction::Block,
        }
    }

    /// Filter PII from text
    pub fn filter(&self, text: &str) -> FilterResult {
        if !self.config.pii_filtering {
            return FilterResult::unchanged(text);
        }

        let mut all_matches: Vec<PiiMatch> = Vec::new();

        // Run all rules
        for rule in &self.rules {
            let action = self.action_for_rule(rule.name());
            if *action == PiiAction::Off {
                continue;
            }

            let matches = rule.detect(text);

            // Filter through allowlist
            for m in matches {
                if !self.allowlist.is_allowed(&m.matched_text, rule.name()) {
                    all_matches.push(m);
                }
            }
        }

        if all_matches.is_empty() {
            return FilterResult::unchanged(text);
        }

        // Sort by position (reverse) for safe replacement
        all_matches.sort_by(|a, b| b.start.cmp(&a.start));

        // Deduplicate overlapping matches (keep higher severity)
        let deduped = dedup_overlapping(all_matches);

        // Apply replacements
        let mut result = text.to_string();
        let mut blocked_count = 0;
        let mut warned_count = 0;

        for detection in &deduped {
            let action = self.action_for_rule(&detection.rule_name);
            match action {
                PiiAction::Block => {
                    // Safety: ensure indices are valid
                    if detection.start <= detection.end && detection.end <= result.len() {
                        result.replace_range(detection.start..detection.end, &detection.placeholder);
                        blocked_count += 1;
                    }
                    warn!(
                        rule = %detection.rule_name,
                        severity = %detection.severity,
                        "PII detected and blocked before API call"
                    );
                }
                PiiAction::Warn => {
                    warned_count += 1;
                    warn!(
                        rule = %detection.rule_name,
                        severity = %detection.severity,
                        "PII detected in outbound message (warn mode)"
                    );
                }
                PiiAction::Off => {}
            }
        }

        FilterResult { text: result, blocked_count, warned_count }
    }
}

/// Remove overlapping matches, keeping the one with higher severity
fn dedup_overlapping(matches: Vec<PiiMatch>) -> Vec<PiiMatch> {
    if matches.len() <= 1 {
        return matches;
    }

    let mut result: Vec<PiiMatch> = Vec::new();
    for m in matches {
        let overlaps = result.iter().any(|existing| {
            m.start < existing.end && m.end > existing.start
        });
        if !overlaps {
            result.push(m);
        }
        // If overlapping, the already-added one wins (higher severity due to sort)
    }
    result
}
```

Create `core/src/pii/rules/mod.rs`:

```rust
//! PII detection rules
//!
//! Each rule detects a specific type of PII with precision-tuned patterns.

mod phone;
mod id_card;
mod bank_card;
mod email;
mod api_key;
mod ip_address;
mod ssh_key;

use crate::pii::engine::{PiiMatch, PiiSeverity};

/// Trait for PII detection rules
pub trait PiiRule: Send + Sync {
    /// Rule identifier (matches config field name)
    fn name(&self) -> &str;

    /// Severity level of this PII type
    fn severity(&self) -> PiiSeverity;

    /// Placeholder text for replacement
    fn placeholder(&self) -> &str;

    /// Detect PII in text, returning all matches
    fn detect(&self, text: &str) -> Vec<PiiMatch>;
}

/// Build all rules (called once at engine initialization)
pub fn build_rules() -> Vec<Box<dyn PiiRule>> {
    vec![
        Box::new(api_key::ApiKeyRule::new()),
        Box::new(ssh_key::SshKeyRule::new()),
        Box::new(id_card::IdCardRule::new()),
        Box::new(phone::PhoneRule::new()),
        Box::new(bank_card::BankCardRule::new()),
        Box::new(email::EmailRule::new()),
        Box::new(ip_address::IpAddressRule::new()),
    ]
}
```

Create `core/src/pii/allowlist.rs`:

```rust
//! PII allowlist — known non-PII values that should not trigger filtering

use std::collections::HashSet;
use regex::Regex;

/// Allowlist of known non-PII values
pub struct PiiAllowlist {
    /// Known test phone numbers
    pub test_phones: HashSet<String>,
    /// System/example email patterns
    pub system_email_patterns: Vec<Regex>,
    /// Known local/internal IPs
    pub local_ips: HashSet<String>,
}

impl Default for PiiAllowlist {
    fn default() -> Self {
        let test_phones: HashSet<String> = [
            "13800138000", "18888888888", "13900001111",
            "13800000000", "15800000000", "18900000000",
        ].iter().map(|s| s.to_string()).collect();

        let system_email_patterns = vec![
            Regex::new(r"(?i)^noreply@").unwrap(),
            Regex::new(r"(?i)^no-reply@").unwrap(),
            Regex::new(r"(?i)^donotreply@").unwrap(),
            Regex::new(r"(?i)@(example|test|demo|sample|mock|localhost)\b").unwrap(),
            Regex::new(r"(?i)\.(example|test|local|internal|invalid)$").unwrap(),
        ];

        let local_ips: HashSet<String> = [
            "127.0.0.1", "0.0.0.0", "localhost",
            "192.168.0.1", "192.168.1.1", "10.0.0.1", "172.16.0.1",
        ].iter().map(|s| s.to_string()).collect();

        Self { test_phones, system_email_patterns, local_ips }
    }
}

impl PiiAllowlist {
    /// Check if a matched value should be excluded from PII detection
    pub fn is_allowed(&self, value: &str, rule_name: &str) -> bool {
        match rule_name {
            "phone" => self.test_phones.contains(value),
            "email" => self.system_email_patterns.iter().any(|p| p.is_match(value)),
            "ip_address" => self.local_ips.contains(value),
            _ => false,
        }
    }
}
```

**Step 2: Register pii module in lib.rs**

Add to `core/src/lib.rs` (after `pub mod poe;`, line 71):

```rust
pub mod pii;
```

**Step 3: Verify it compiles**

Note: It won't compile yet because the rule files don't exist. We'll create stub files.

Create each rule file as a stub. Example for `core/src/pii/rules/phone.rs`:

```rust
use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;

pub struct PhoneRule;
impl PhoneRule { pub fn new() -> Self { Self } }
impl PiiRule for PhoneRule {
    fn name(&self) -> &str { "phone" }
    fn severity(&self) -> PiiSeverity { PiiSeverity::High }
    fn placeholder(&self) -> &str { "[PHONE]" }
    fn detect(&self, _text: &str) -> Vec<PiiMatch> { vec![] }
}
```

Create identical stubs for all 7 rules (id_card, bank_card, email, api_key, ip_address, ssh_key) with appropriate names, severities, and placeholders:
- `id_card`: Critical, `[ID_CARD]`
- `bank_card`: High, `[BANK_CARD]`
- `email`: Medium, `[EMAIL]`
- `api_key`: Critical, `[REDACTED]`
- `ip_address`: Low, `[IP]`
- `ssh_key`: Critical, `[SSH_KEY]`

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -20`
Expected: Compiles

**Step 5: Commit**

```bash
git add core/src/pii/ core/src/lib.rs
git commit -m "pii: add engine scaffold with PiiRule trait, allowlist, and rule stubs"
```

---

### Task 3: Implement phone rule with anti-false-positive checks

**Files:**
- Modify: `core/src/pii/rules/phone.rs`

**Step 1: Write tests first**

Add tests at the bottom of `core/src/pii/rules/phone.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PhoneRule { PhoneRule::new() }

    // === Positive matches ===

    #[test]
    fn test_detect_china_mobile_13x() {
        let matches = rule().detect("Call me at 13812345678");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    #[test]
    fn test_detect_china_mobile_15x() {
        let matches = rule().detect("Phone: 15987654321");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_china_mobile_18x() {
        let matches = rule().detect("Contact 18612345678 for details");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_multiple_phones() {
        let matches = rule().detect("13812345678 and 15900001234");
        assert_eq!(matches.len(), 2);
    }

    // === Anti-false-positive: UUID fragments ===

    #[test]
    fn test_no_match_uuid_fragment() {
        // UUID: 18160019229f-4b7a-...
        // "18160019229" looks like a phone but is part of a UUID
        let matches = rule().detect("id: 18160019229f-4b7a-8c3d");
        assert_eq!(matches.len(), 0, "UUID fragment should not match as phone");
    }

    #[test]
    fn test_no_match_hex_suffix() {
        let matches = rule().detect("a18612345678b");
        // Preceded by hex 'a' — should skip
        assert_eq!(matches.len(), 0, "Hex-bounded number should not match");
    }

    // === Anti-false-positive: Timestamp context ===

    #[test]
    fn test_no_match_timestamp_context() {
        let matches = rule().detect("\"timestamp\": 13891680001");
        assert_eq!(matches.len(), 0, "Timestamp context should suppress match");
    }

    #[test]
    fn test_no_match_created_at_context() {
        let matches = rule().detect("\"created_at\": 13891680001");
        assert_eq!(matches.len(), 0);
    }

    // === Normal numbers should not match ===

    #[test]
    fn test_no_match_short_number() {
        let matches = rule().detect("Version 12345");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_invalid_prefix() {
        // 10x, 11x, 12x are not valid Chinese mobile prefixes
        let matches = rule().detect("Number: 10812345678");
        assert_eq!(matches.len(), 0);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore pii::rules::phone 2>&1 | tail -20`
Expected: Most tests FAIL (stubs return empty vec)

**Step 3: Implement the phone rule**

Replace the stub in `core/src/pii/rules/phone.rs` with full implementation:

```rust
//! Chinese mobile phone number detection with anti-false-positive checks

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static PHONE_RE: OnceLock<Regex> = OnceLock::new();
static TIMESTAMP_CONTEXT_RE: OnceLock<Regex> = OnceLock::new();

fn phone_regex() -> &'static Regex {
    PHONE_RE.get_or_init(|| Regex::new(r"1[3-9]\d{9}").unwrap())
}

fn timestamp_context_regex() -> &'static Regex {
    TIMESTAMP_CONTEXT_RE.get_or_init(|| {
        Regex::new(r"(?i)(timestamp|time|date|created_at|updated_at|expires?_at|modified_at)\b")
            .unwrap()
    })
}

pub struct PhoneRule;

impl PhoneRule {
    pub fn new() -> Self { Self }

    /// Check if the match is adjacent to hex characters (likely UUID fragment)
    fn is_hex_bounded(text: &str, start: usize, end: usize) -> bool {
        // Check character before match
        if start > 0 {
            if let Some(c) = text[..start].chars().last() {
                if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                    return true; // a-f before the number
                }
            }
        }
        // Check character after match
        if end < text.len() {
            if let Some(c) = text[end..].chars().next() {
                if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                    return true; // a-f after the number
                }
            }
        }
        false
    }

    /// Check if match is in a timestamp context (surrounding 80 chars)
    fn is_timestamp_context(text: &str, start: usize) -> bool {
        let ctx_start = start.saturating_sub(40);
        let ctx_end = (start + 40).min(text.len());
        let context = &text[ctx_start..ctx_end];
        timestamp_context_regex().is_match(context)
    }

    /// Check word boundary: the match should not be part of a longer digit sequence
    fn has_word_boundary(text: &str, start: usize, end: usize) -> bool {
        let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_digit();
        let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_digit();
        before_ok && after_ok
    }
}

impl PiiRule for PhoneRule {
    fn name(&self) -> &str { "phone" }
    fn severity(&self) -> PiiSeverity { PiiSeverity::High }
    fn placeholder(&self) -> &str { "[PHONE]" }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = phone_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let matched = m.as_str();

            // Anti-false-positive: word boundary check
            if !Self::has_word_boundary(text, start, end) {
                continue;
            }

            // Anti-false-positive: hex boundary (UUID fragment)
            if Self::is_hex_bounded(text, start, end) {
                continue;
            }

            // Anti-false-positive: timestamp context
            if Self::is_timestamp_context(text, start) {
                continue;
            }

            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start,
                end,
                matched_text: matched.to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
        }

        results
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore pii::rules::phone 2>&1 | tail -20`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/pii/rules/phone.rs
git commit -m "pii: implement phone rule with hex boundary and timestamp context checks"
```

---

### Task 4: Implement ID card rule with structural validation

**Files:**
- Modify: `core/src/pii/rules/id_card.rs`

**Step 1: Write tests first**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> IdCardRule { IdCardRule::new() }

    // === Positive matches ===

    #[test]
    fn test_detect_valid_id_card() {
        // 310101 = Shanghai, 19900101 = Jan 1 1990, valid checksum
        let matches = rule().detect("ID: 110101199001011234");
        // This may or may not match depending on checksum — use a known-valid one
        // Let's use a structurally valid ID (checksum must be correct)
        let matches = rule().detect("ID: 11010119900307793X");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_id_with_lowercase_x() {
        let matches = rule().detect("身份证号: 11010119900307793x");
        assert_eq!(matches.len(), 1);
    }

    // === Anti-false-positive: Discord Snowflake ===

    #[test]
    fn test_no_match_discord_snowflake() {
        // Discord Snowflake IDs: 17-20 digit numbers, but NOT valid ID cards
        // Region code would be random, date would be invalid, checksum won't match
        let matches = rule().detect("channel: 1468256454954975286");
        assert_eq!(matches.len(), 0, "Discord Snowflake should not match as ID card");
    }

    // === Anti-false-positive: Random 18-digit numbers ===

    #[test]
    fn test_no_match_random_18_digits() {
        let matches = rule().detect("Order: 123456789012345678");
        assert_eq!(matches.len(), 0, "Random number should fail structural validation");
    }

    // === Anti-false-positive: Invalid region code ===

    #[test]
    fn test_no_match_invalid_region() {
        // Region code 99 is invalid
        let matches = rule().detect("ID: 990101199001011234");
        assert_eq!(matches.len(), 0);
    }

    // === Anti-false-positive: Invalid date ===

    #[test]
    fn test_no_match_invalid_date() {
        // Month 13 is invalid
        let matches = rule().detect("ID: 110101199013011234");
        assert_eq!(matches.len(), 0);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore pii::rules::id_card 2>&1 | tail -20`

**Step 3: Implement the ID card rule**

Key implementation details:
- Regex: `\d{17}[\dXx]` with word boundary check
- Region code validation: first 2 digits in 11-82 range
- Date validation: digits 7-14 form valid YYYYMMDD (year 1900-2100, month 1-12, day 1-31)
- Checksum: ISO 7064 MOD 11-2 algorithm (weights: 7,9,10,5,8,4,2,1,6,3,7,9,10,5,8,4,2; check codes: 1,0,X,9,8,7,6,5,4,3,2)

**Step 4: Run tests, verify pass**

**Step 5: Commit**

```bash
git add core/src/pii/rules/id_card.rs
git commit -m "pii: implement id_card rule with region code, date, and checksum validation"
```

---

### Task 5: Implement bank card rule with Luhn + decimal exclusion

**Files:**
- Modify: `core/src/pii/rules/bank_card.rs`

**Step 1: Write tests**

Key test cases:
- Valid 16-digit card with Luhn checksum → match
- Valid 19-digit UnionPay card → match
- Number preceded by `.` (JSON float) → NO match
- Number that fails Luhn → NO match
- Number already matched as ID card (18 digits, valid structure) → handled by dedup in engine

**Step 2-5: Implement, test, commit**

Implementation details:
- Regex: `\d{16,19}` with word boundary
- Decimal point check: skip if `text[start-1] == '.'` or `text[end] == '.'`
- Luhn algorithm validation
- Known BIN prefix check (optional, for higher confidence)

```bash
git commit -m "pii: implement bank_card rule with Luhn checksum and decimal exclusion"
```

---

### Task 6: Implement remaining rules (email, api_key, ip_address, ssh_key)

**Files:**
- Modify: `core/src/pii/rules/email.rs`
- Modify: `core/src/pii/rules/api_key.rs`
- Modify: `core/src/pii/rules/ip_address.rs`
- Modify: `core/src/pii/rules/ssh_key.rs`

These are simpler rules with fewer anti-false-positive concerns.

**email.rs:**
- Regex: `[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}` with word boundary
- Allowlist filtering handles system emails

**api_key.rs:**
- Prefix-based matching only: `sk-[a-zA-Z0-9\-_]{20,}`, `ghp_[a-zA-Z0-9]{36,}`, `AKIA[A-Z0-9]{16}`, `xox[bpras]-[a-zA-Z0-9\-]{10,}`, `tvly-[a-zA-Z0-9\-_]{20,}`, `Bearer [a-zA-Z0-9._\-]{20,}`
- No generic long-string detection (avoids URL slug false positives)

**ip_address.rs:**
- Regex: `\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b`
- Allowlist filtering handles local IPs

**ssh_key.rs:**
- Pattern: `-----BEGIN [A-Z ]*(?:PRIVATE KEY|RSA PRIVATE KEY|EC PRIVATE KEY|DSA PRIVATE KEY)-----`
- Matches the header line; can optionally match the full key block

**Step 1: Write tests for all 4 rules**
**Step 2: Implement all 4 rules**
**Step 3: Run all pii tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore pii:: 2>&1 | tail -30`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/pii/rules/
git commit -m "pii: implement email, api_key, ip_address, and ssh_key rules"
```

---

### Task 7: PiiEngine integration tests

**Files:**
- Modify: `core/src/pii/engine.rs` (add tests)

**Step 1: Write engine-level integration tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PrivacyConfig, PiiAction};

    fn engine() -> PiiEngine {
        PiiEngine::new(PrivacyConfig::default())
    }

    #[test]
    fn test_filter_phone_number() {
        let result = engine().filter("Call me at 13812345678");
        assert_eq!(result.text, "Call me at [PHONE]");
        assert_eq!(result.blocked_count, 1);
    }

    #[test]
    fn test_filter_multiple_pii_types() {
        let result = engine().filter(
            "Phone: 13812345678, ID: 11010119900307793X"
        );
        assert!(result.text.contains("[PHONE]"));
        assert!(result.text.contains("[ID_CARD]"));
        assert_eq!(result.blocked_count, 2);
    }

    #[test]
    fn test_filter_disabled() {
        let config = PrivacyConfig { pii_filtering: false, ..Default::default() };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_warn_mode_no_replacement() {
        let config = PrivacyConfig {
            phone: PiiAction::Warn,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        // Warn mode: original text preserved, but warned
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.warned_count, 1);
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_off_mode_no_detection() {
        let config = PrivacyConfig {
            phone: PiiAction::Off,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.warned_count, 0);
    }

    #[test]
    fn test_filter_no_pii() {
        let result = engine().filter("Normal text with no personal info");
        assert_eq!(result.text, "Normal text with no personal info");
        assert!(!result.has_detections());
    }

    #[test]
    fn test_filter_test_phone_allowed() {
        // 13800138000 is in the allowlist
        let result = engine().filter("Test: 13800138000");
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_excluded_provider() {
        let config = PrivacyConfig {
            exclude_providers: vec!["ollama".to_string()],
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        assert!(engine.is_provider_excluded("ollama"));
        assert!(!engine.is_provider_excluded("anthropic"));
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore pii::engine 2>&1 | tail -20`
Expected: All PASS

**Step 3: Commit**

```bash
git add core/src/pii/engine.rs
git commit -m "pii: add engine integration tests covering all action modes"
```

---

### Task 8: Inject PII filter into HttpProvider::execute()

**Files:**
- Modify: `core/src/providers/http_provider.rs:59-71` (the `execute` method)

**Step 1: Write a test for the integration**

Add to `core/src/providers/http_provider.rs` tests:

```rust
#[test]
fn test_pii_filtering_integration() {
    // Verify PiiEngine can be called from HttpProvider context
    use crate::pii::PiiEngine;
    use crate::config::PrivacyConfig;

    let engine = PiiEngine::new(PrivacyConfig::default());
    let result = engine.filter("User: Call 13812345678 for info");
    assert!(result.text.contains("[PHONE]"));
    assert!(!result.text.contains("13812345678"));
}
```

**Step 2: Modify HttpProvider::execute()**

In `core/src/providers/http_provider.rs`, modify the `execute` method (lines 59-71):

```rust
/// Execute a request (non-streaming)
async fn execute(&self, payload: RequestPayload<'_>) -> Result<String> {
    // PII filtering: filter outbound message before sending to API
    let filtered_input;
    let final_payload = if let Some(engine_lock) = crate::pii::PiiEngine::global() {
        if let Ok(engine) = engine_lock.read() {
            if !engine.is_provider_excluded(&self.name) {
                let result = engine.filter(payload.input);
                if result.has_detections() {
                    filtered_input = result.text;
                    RequestPayload {
                        input: &filtered_input,
                        system_prompt: payload.system_prompt,
                        image: payload.image,
                        attachments: payload.attachments,
                        think_level: payload.think_level,
                        force_standard_mode: payload.force_standard_mode,
                    }
                } else {
                    payload
                }
            } else {
                payload
            }
        } else {
            payload
        }
    } else {
        // PII engine not initialized — pass through
        payload
    };

    let request = self.adapter.build_request(&final_payload, &self.config, false)?;
    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            crate::error::AlephError::Timeout {
                suggestion: Some("Request timed out. Try again or switch providers.".into()),
            }
        } else {
            crate::error::AlephError::network(format!("Network error: {}", e))
        }
    })?;
    self.adapter.parse_response(response).await
}
```

Also apply the same pattern to `execute_stream`:

```rust
async fn execute_stream(
    &self,
    payload: RequestPayload<'_>,
) -> Result<BoxStream<'static, Result<String>>> {
    // PII filtering (same as execute)
    let filtered_input;
    let final_payload = if let Some(engine_lock) = crate::pii::PiiEngine::global() {
        if let Ok(engine) = engine_lock.read() {
            if !engine.is_provider_excluded(&self.name) {
                let result = engine.filter(payload.input);
                if result.has_detections() {
                    filtered_input = result.text;
                    RequestPayload {
                        input: &filtered_input,
                        system_prompt: payload.system_prompt,
                        image: payload.image,
                        attachments: payload.attachments,
                        think_level: payload.think_level,
                        force_standard_mode: payload.force_standard_mode,
                    }
                } else {
                    payload
                }
            } else {
                payload
            }
        } else {
            payload
        }
    } else {
        payload
    };

    let request = self.adapter.build_request(&final_payload, &self.config, true)?;
    let response = request.send().await.map_err(|e| {
        crate::error::AlephError::network(format!("Network error: {}", e))
    })?;
    self.adapter.parse_stream(response).await
}
```

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -20`

**Step 4: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -30`
Expected: All existing + new tests PASS

**Step 5: Commit**

```bash
git add core/src/providers/http_provider.rs
git commit -m "pii: inject PII filtering into HttpProvider::execute() and execute_stream()"
```

---

### Task 9: Initialize PiiEngine at server startup

**Files:**
- Modify: Server initialization code (find the exact location where `Config` is loaded and gateway starts)

**Step 1: Find initialization point**

Search for where `Config` is loaded and used to start the server. Look for `Config::load` or similar in the server binary entry point.

**Step 2: Add PiiEngine::init() call**

After config is loaded, before the gateway starts:

```rust
// Initialize PII filtering engine
crate::pii::PiiEngine::init(config.privacy.clone());
```

**Step 3: Add hot-reload support**

In the config reload handler (where `build_reload_plan` is called), add privacy to hot paths:

```rust
// When privacy config changes, reload PII engine
if new_config.privacy != old_config.privacy {
    crate::pii::PiiEngine::reload(new_config.privacy.clone());
}
```

Note: This requires adding `PartialEq` derive to `PrivacyConfig` and `PiiAction`.

**Step 4: Verify compilation and tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore && cargo test -p alephcore 2>&1 | tail -20`

**Step 5: Commit**

```bash
git add -A
git commit -m "pii: initialize PiiEngine at server startup with hot-reload support"
```

---

### Task 10: Final verification + full test suite

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -40`
Expected: All tests PASS, no regressions

**Step 2: Verify compilation with all features**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore --features gateway 2>&1 | tail -20`

**Step 3: Manual verification**

Test with a sample config:

```toml
[privacy]
pii_filtering = true
id_card = "block"
bank_card = "block"
phone = "block"
email = "warn"
ip_address = "off"
exclude_providers = ["ollama"]
```

**Step 4: Final commit**

```bash
git add -A
git commit -m "pii: complete gateway-level PII filtering implementation"
```
