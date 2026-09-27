//! Mock routes and the per-tab interception loop (spec §2/§3).
//!
//! The registry outlives any `CdpBackend` (backends are rebuilt per call), so
//! it lives on `ProfileManager` next to `TabRegistry` and the backend carries
//! an `Arc` — the same reason `tab_identities` does (mod.rs:85-91).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// A rule's decision material. `Mock` carries everything `Fetch.fulfillRequest`
/// needs, so the loop never re-consults the registry between verdict and wire.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteKind {
    Mock {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Abort,
}

/// Who a rule applies to. `Tab` binds the one tab it was added against;
/// `Profile` serves every tab of ITS profile — never another profile's
/// (spec §4: "profile 下所有 tab", and a mock that bled across principals
/// would be an isolation breach, not a feature).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteScope {
    Tab,
    Profile,
}

/// What `mock_list` shows. A snapshot struct rather than a borrow so the
/// registry lock is never held across a read the caller formats.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteRuleInfo {
    pub id: String,
    pub url_contains: String,
    pub method: Option<String>,
    pub kind_label: &'static str,
    pub scope: RouteScope,
    pub hits: u64,
    /// Whether a live interception loop is currently serving this rule. A rule
    /// whose loop died (session death, engine restart) stays in the table but
    /// reads `active: false` — present-but-unserved is a fact the model must
    /// be able to see, not a success we invent (判据 §8).
    pub active: bool,
    /// The model's own annotation, carried back verbatim by `mock_list`.
    pub note: Option<String>,
}

/// A match verdict plus the material to carry it out. The hit count is
/// incremented inside `match_lifo` — the verdict and the accounting are one
/// lock hold, so a rule can never serve a request it was not credited for.
#[derive(Clone, Debug)]
pub struct MatchedRoute {
    pub info: RouteRuleInfo,
    pub kind: RouteKind,
}

/// Why [`RouteRegistry::add`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RouteRuleError {
    /// Review Focus #4: an empty needle matches EVERY request — almost always
    /// a model typo, so it is refused here as well as at the tool layer
    /// (depth, not duplication).
    #[error("url_contains must not be empty: an empty needle would intercept every request")]
    EmptyUrlContains,
}

struct RouteRule {
    id: String,
    /// Owning profile — matched even for `Tab` rules, because tab ids are
    /// engine-chosen strings and two profiles' engines are not obliged to
    /// never collide.
    profile: String,
    /// `Some` exactly for `Tab` scope.
    tab_id: Option<String>,
    url_contains: String,
    /// Uppercased at `add`, so the CDP event's always-uppercase method and
    /// the model's `\"post\"` meet in one spelling.
    method: Option<String>,
    kind: RouteKind,
    scope: RouteScope,
    note: Option<String>,
    hits: AtomicU64,
    active: AtomicBool,
}

impl RouteRule {
    fn matches(&self, url: &str, method: &str) -> bool {
        // substring, not regex (R8); the needle may land anywhere in the URL.
        url.contains(&self.url_contains)
            && self.method.as_deref().is_none_or(|m| m == method)
    }

    fn applies_to(&self, profile: &str, tab_id: &str) -> bool {
        match self.scope {
            RouteScope::Tab => {
                self.profile == profile && self.tab_id.as_deref() == Some(tab_id)
            }
            RouteScope::Profile => self.profile == profile,
        }
    }

    fn info(&self) -> RouteRuleInfo {
        RouteRuleInfo {
            id: self.id.clone(),
            url_contains: self.url_contains.clone(),
            method: self.method.clone(),
            kind_label: match &self.kind {
                RouteKind::Mock { .. } => "mock",
                RouteKind::Abort => "abort",
            },
            scope: self.scope,
            hits: self.hits.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
            note: self.note.clone(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The mock-rule table. One per `ProfileManager`; tab ids are engine-chosen,
/// so every key that could collide across profiles carries the profile too.
pub struct RouteRegistry {
    /// Registration order IS Vec order; LIFO = scan from the tail.
    rules: Mutex<Vec<RouteRule>>,
    /// `r1, r2…` monotonic, never reused (spec §4: the model quotes ids back).
    counter: AtomicU64,
}

impl Default for RouteRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            rules: Mutex::new(Vec::new()),
            counter: AtomicU64::new(0),
        }
    }

    pub fn add(
        &self,
        profile: &str,
        tab_id: &str,
        url_contains: &str,
        method: Option<&str>,
        kind: RouteKind,
        scope: RouteScope,
        note: Option<String>,
    ) -> Result<RouteRuleInfo, RouteRuleError> {
        if url_contains.is_empty() {
            return Err(RouteRuleError::EmptyUrlContains);
        }
        let id = format!("r{}", self.counter.fetch_add(1, Ordering::Relaxed) + 1);
        let rule = RouteRule {
            id,
            profile: profile.to_string(),
            tab_id: match scope {
                RouteScope::Tab => Some(tab_id.to_string()),
                RouteScope::Profile => None,
            },
            url_contains: url_contains.to_string(),
            method: method.map(|m| m.to_ascii_uppercase()),
            kind,
            scope,
            note,
            hits: AtomicU64::new(0),
            active: AtomicBool::new(false),
        };
        let info = rule.info();
        lock(&self.rules).push(rule);
        Ok(info)
    }

    /// Remove one rule by id. The returned info carries the FINAL hit count —
    /// the confirmation `mock_remove` reports.
    pub fn remove(&self, id: &str) -> Option<RouteRuleInfo> {
        let mut rules = lock(&self.rules);
        let at = rules.iter().position(|r| r.id == id)?;
        Some(rules.remove(at).info())
    }

    /// Drop every `Tab` rule bound to `(profile, tab_id)`. Returns how many.
    pub fn clear_tab(&self, profile: &str, tab_id: &str) -> usize {
        let mut rules = lock(&self.rules);
        let before = rules.len();
        rules.retain(|r| {
            !(r.scope == RouteScope::Tab && r.profile == profile
                && r.tab_id.as_deref() == Some(tab_id))
        });
        before - rules.len()
    }

    /// Drop every `Profile` rule of `profile` — never another profile's, and
    /// never its tab rules. Returns how many.
    pub fn clear_profile(&self, profile: &str) -> usize {
        let mut rules = lock(&self.rules);
        let before = rules.len();
        rules.retain(|r| !(r.scope == RouteScope::Profile && r.profile == profile));
        before - rules.len()
    }

    /// Every rule of one profile, in registration order.
    pub fn list(&self, profile: &str) -> Vec<RouteRuleInfo> {
        lock(&self.rules)
            .iter()
            .filter(|r| r.profile == profile)
            .map(RouteRule::info)
            .collect()
    }

    /// Whether anything would intercept on this tab: one of its own tab rules,
    /// or any of its profile's rules. The zero-overhead clause's test (spec §2:
    /// no rules → no `Fetch.enable`, no task).
    pub(crate) fn has_rules_for(&self, profile: &str, tab_id: &str) -> bool {
        lock(&self.rules).iter().any(|r| r.applies_to(profile, tab_id))
    }

    /// Tab rules first (tail-first), then profile rules (tail-first). A hit is
    /// credited to the rule THAT matched, inside this same lock hold.
    pub(crate) fn match_lifo(
        &self,
        profile: &str,
        tab_id: &str,
        url: &str,
        method: &str,
    ) -> Option<MatchedRoute> {
        let rules = lock(&self.rules);
        let hit = rules
            .iter()
            .rev()
            .find(|r| {
                r.scope == RouteScope::Tab
                    && r.profile == profile
                    && r.tab_id.as_deref() == Some(tab_id)
                    && r.matches(url, method)
            })
            .or_else(|| {
                rules.iter().rev().find(|r| {
                    r.scope == RouteScope::Profile && r.profile == profile && r.matches(url, method)
                })
            })?;
        hit.hits.fetch_add(1, Ordering::Relaxed);
        Some(MatchedRoute {
            info: hit.info(),
            kind: hit.kind.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_json(status: u16, tag: &str) -> RouteKind {
        RouteKind::Mock {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: format!("\"{tag}\"").into_bytes(),
        }
    }

    #[test]
    fn matching_is_lifo_and_tab_rules_beat_profile_rules() {
        let reg = RouteRegistry::new();
        let r1 = reg
            .add("p", "t1", "api", None, mock_json(200, "profile-old"), RouteScope::Profile, None)
            .expect("add r1");
        let r2 = reg
            .add("p", "t1", "api", None, mock_json(200, "tab-new"), RouteScope::Tab, None)
            .expect("add r2");
        let r3 = reg
            .add("p", "t1", "api", None, mock_json(200, "tab-newer"), RouteScope::Tab, None)
            .expect("add r3");
        assert_eq!((r1.id.as_str(), r2.id.as_str(), r3.id.as_str()), ("r1", "r2", "r3"));

        // LIFO: the LAST registered matching rule wins…
        let hit = reg
            .match_lifo("p", "t1", "https://a.test/api/x", "GET")
            .expect("a hit");
        assert_eq!(hit.info.id, "r3");
        // …and the decision material travels WITH the verdict, so the loop
        // never re-derives (and cannot re-derive differently).
        match &hit.kind {
            RouteKind::Mock { body, .. } => assert_eq!(body, b"\"tab-newer\""),
            RouteKind::Abort => panic!("r3 is a mock"),
        }

        // r3 gone → r2; both tab rules gone → the profile rule is reached.
        assert!(reg.remove("r3").is_some());
        assert_eq!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "GET")
                .expect("hit")
                .info
                .id,
            "r2"
        );
        assert!(reg.remove("r2").is_some());
        assert_eq!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "GET")
                .expect("hit")
                .info
                .id,
            "r1",
            "profile rules are only reached once no tab rule matches"
        );
    }

    #[test]
    fn method_filter_narrows_and_substring_matches_anywhere() {
        let reg = RouteRegistry::new();
        reg.add("p", "t1", "api", Some("post"), mock_json(200, "x"), RouteScope::Tab, None)
            .expect("add");
        assert!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "GET").is_none(),
            "a POST-only rule must not catch GET"
        );
        assert!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "POST").is_some(),
            "method input is normalised, lowercase `post` still narrows to POST"
        );
        // substring, not anchored (R8): the needle may land anywhere.
        assert!(
            reg.match_lifo("p", "t1", "https://deep.example/prefix/api/x?q=1", "POST")
                .is_some()
        );
        assert!(reg.match_lifo("p", "t1", "https://a.test/other", "POST").is_none());
    }

    #[test]
    fn hits_are_counted_on_the_rule_that_matched() {
        let reg = RouteRegistry::new();
        let profile_rule = reg
            .add("p", "t1", "api", None, mock_json(200, "p"), RouteScope::Profile, None)
            .expect("add");
        let tab_rule = reg
            .add("p", "t1", "api", None, mock_json(200, "t"), RouteScope::Tab, None)
            .expect("add");
        reg.match_lifo("p", "t1", "https://a.test/api/1", "GET");
        reg.match_lifo("p", "t1", "https://a.test/api/2", "GET");
        let listed = reg.list("p");
        let tab_info = listed.iter().find(|i| i.id == tab_rule.id).expect("listed");
        let profile_info = listed.iter().find(|i| i.id == profile_rule.id).expect("listed");
        assert_eq!(tab_info.hits, 2, "both hits landed on the tab rule");
        assert_eq!(profile_info.hits, 0, "the shadowed rule is not credited");
    }

    #[test]
    fn add_rejects_an_empty_url_contains() {
        // Review Focus #4: an empty needle matches EVERYTHING — almost always a
        // model typo, so the registry refuses it too (the tool layer refuses it
        // first; depth, not duplication).
        let reg = RouteRegistry::new();
        let err = reg
            .add("p", "t1", "", None, mock_json(200, "x"), RouteScope::Tab, None)
            .expect_err("empty url_contains is refused");
        assert_eq!(err, RouteRuleError::EmptyUrlContains);
        assert!(reg.list("p").is_empty(), "a refused rule never entered the table");
    }

    #[test]
    fn profile_rules_do_not_leak_across_profiles() {
        // spec §4: scope=profile means "this profile's tabs", not "every tab in
        // the daemon". A mock that bled into another principal's browser would
        // be an isolation breach, not a feature.
        let reg = RouteRegistry::new();
        reg.add("alice", "t1", "api", None, mock_json(200, "a"), RouteScope::Profile, None)
            .expect("add");
        assert!(
            reg.match_lifo("bob", "t9", "https://a.test/api", "GET").is_none(),
            "alice's profile rule must not serve bob's tab"
        );
        assert!(
            reg.match_lifo("alice", "t9", "https://a.test/api", "GET").is_some(),
            "a profile rule serves ANY tab of its own profile"
        );
        reg.add("alice", "t1", "secret", None, mock_json(200, "s"), RouteScope::Tab, None)
            .expect("add");
        assert!(
            reg.match_lifo("alice", "t2", "https://a.test/secret", "GET").is_none(),
            "a tab rule binds one tab, not the profile"
        );
    }

    #[test]
    fn rule_ids_are_monotonic_and_never_reused() {
        // spec §4: the model quotes rule ids back; a reused id would let
        // `mock_remove r1` delete a rule it never meant.
        let reg = RouteRegistry::new();
        let first = reg
            .add("p", "t1", "a", None, mock_json(200, "1"), RouteScope::Tab, None)
            .expect("add");
        assert!(reg.remove(&first.id).is_some());
        let second = reg
            .add("p", "t1", "a", None, mock_json(200, "2"), RouteScope::Tab, None)
            .expect("add");
        assert_ne!(first.id, second.id, "ids are not recycled");
        assert_eq!(second.id, "r2");
    }

    #[test]
    fn clear_tab_and_clear_profile_remove_only_their_own_scope() {
        let reg = RouteRegistry::new();
        reg.add("p", "t1", "a", None, mock_json(200, "t1"), RouteScope::Tab, None).expect("add");
        reg.add("p", "t2", "a", None, mock_json(200, "t2"), RouteScope::Tab, None).expect("add");
        reg.add("p", "t1", "b", None, mock_json(200, "p"), RouteScope::Profile, None).expect("add");
        assert_eq!(reg.clear_tab("p", "t1"), 1, "one tab rule for t1");
        assert_eq!(reg.list("p").len(), 2, "t2's tab rule and the profile rule survive");
        assert_eq!(reg.clear_profile("p"), 1, "one profile rule");
        assert_eq!(reg.list("p").len(), 1, "tab rules survive a profile clear");
        // A profile clear never touches another profile.
        reg.add("q", "t9", "b", None, mock_json(200, "q"), RouteScope::Profile, None).expect("add");
        assert_eq!(reg.clear_profile("p"), 0);
        assert_eq!(reg.list("q").len(), 1);
    }

    #[test]
    fn list_reports_kind_label_scope_hits_and_active() {
        let reg = RouteRegistry::new();
        let mock = reg
            .add("p", "t1", "a", Some("GET"), mock_json(201, "m"), RouteScope::Tab, None)
            .expect("add");
        let abort = reg
            .add("p", "t1", "b", None, RouteKind::Abort, RouteScope::Profile, None)
            .expect("add");
        let listed = reg.list("p");
        assert_eq!(listed.len(), 2);
        let mock = listed.iter().find(|i| i.id == mock.id).expect("listed");
        assert_eq!(mock.kind_label, "mock");
        assert_eq!(mock.scope, RouteScope::Tab);
        assert_eq!(mock.method.as_deref(), Some("GET"));
        let abort = listed.iter().find(|i| i.id == abort.id).expect("listed");
        assert_eq!(abort.kind_label, "abort");
        assert_eq!(abort.scope, RouteScope::Profile);
        // No loop is armed in this test, so nothing is being served.
        assert!(listed.iter().all(|i| !i.active));
    }

    #[test]
    fn has_rules_for_sees_tab_and_profile_rules() {
        let reg = RouteRegistry::new();
        assert!(!reg.has_rules_for("p", "t1"));
        reg.add("p", "t2", "a", None, mock_json(200, "x"), RouteScope::Tab, None).expect("add");
        assert!(!reg.has_rules_for("p", "t1"), "another tab's rule does not count");
        assert!(reg.has_rules_for("p", "t2"));
        reg.add("p", "t1", "a", None, mock_json(200, "x"), RouteScope::Profile, None)
            .expect("add");
        assert!(reg.has_rules_for("p", "t1"), "a profile rule counts for every tab");
    }
}
