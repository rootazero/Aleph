//! Mock routes and the per-tab interception loop (spec §2/§3).
//!
//! The registry outlives any `CdpBackend` (backends are rebuilt per call), so
//! it lives on `ProfileManager` next to `TabRegistry` and the backend carries
//! an `Arc` — the same reason `tab_identities` does (mod.rs:85-91).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use aleph_cdp::methods::fetch;
use aleph_cdp::{CdpConnection, EventStream, SessionId};

use crate::browser::error::BrowserError;
use crate::browser::network_policy::BrowserSsrfGuard;

use super::{map_cdp_err, CdpBackend};

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
///
/// `Serialize` because `RouteRuleInfo` (which carries it) is part of
/// `browser_network`'s tool output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteScope {
    Tab,
    Profile,
}

/// Everything `route_add` needs beyond WHERE the rule binds (the backend
/// supplies the profile from its own identity; the call supplies the tab).
/// Grouped because the trait signature would otherwise be seven positional
/// parameters wide.
#[derive(Clone, Debug, PartialEq)]
pub struct NewRouteRule {
    pub url_contains: String,
    pub method: Option<String>,
    pub kind: RouteKind,
    pub scope: RouteScope,
    pub note: Option<String>,
}

/// What `mock_list` shows. A snapshot struct rather than a borrow so the
/// registry lock is never held across a read the caller formats.
///
/// `Serialize` because this struct IS `browser_network`'s wire answer —
/// `kind` travels as the label, never the material (the body can be 256 KiB
/// and the model that listed the rules already knows what it wrote).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
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
        url.contains(&self.url_contains) && self.method.as_deref().is_none_or(|m| m == method)
    }

    fn applies_to(&self, profile: &str, tab_id: &str) -> bool {
        match self.scope {
            RouteScope::Tab => self.profile == profile && self.tab_id.as_deref() == Some(tab_id),
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
    /// `(profile, tab_id)` → the live interception loop's kill switch.
    loops: Mutex<HashMap<(String, String), tokio::task::AbortHandle>>,
    /// Serialises `ensure_intercept_loop`'s check→enable→spawn→register, so
    /// two concurrent arms of one tab cannot both spawn (a doubled loop would
    /// answer every paused request twice and double-count every hit).
    start: tokio::sync::Mutex<()>,
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
            loops: Mutex::new(HashMap::new()),
            start: tokio::sync::Mutex::new(()),
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
        self.refresh_active();
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
            !(r.scope == RouteScope::Tab
                && r.profile == profile
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
        lock(&self.rules)
            .iter()
            .any(|r| r.applies_to(profile, tab_id))
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

    // ---- loop table (the InterceptLoop's book-keeping) ----

    pub(crate) fn loop_running(&self, profile: &str, tab_id: &str) -> bool {
        lock(&self.loops)
            .get(&(profile.to_string(), tab_id.to_string()))
            .is_some_and(|h| !h.is_finished())
    }

    /// The spawn serialiser. Held across `ensure_intercept_loop`'s
    /// check→enable→spawn→register sequence.
    pub(crate) async fn start_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.start.lock().await
    }

    pub(crate) fn register_loop(
        &self,
        profile: &str,
        tab_id: &str,
        abort: tokio::task::AbortHandle,
    ) {
        lock(&self.loops).insert((profile.to_string(), tab_id.to_string()), abort);
        self.refresh_active();
    }

    /// Remove and return the loop's kill switch. Called by the loop itself on
    /// exit AND by `stop_intercept_loop_if_idle`; both removing is fine.
    pub(crate) fn take_loop(
        &self,
        profile: &str,
        tab_id: &str,
    ) -> Option<tokio::task::AbortHandle> {
        let gone = lock(&self.loops).remove(&(profile.to_string(), tab_id.to_string()));
        self.refresh_active();
        gone
    }

    /// A loop whose task ended without anyone noticing (crash, session death)
    /// leaves a stale entry that would make the next `ensure` answer "already
    /// running" while nothing is served. Reap it so the re-arm can proceed.
    pub(crate) fn reap_dead_loop(&self, profile: &str, tab_id: &str) {
        let dead = lock(&self.loops)
            .get(&(profile.to_string(), tab_id.to_string()))
            .is_some_and(|h| h.is_finished());
        if dead {
            self.take_loop(profile, tab_id);
        }
    }

    /// `active` means "a live loop serves this rule right now": a tab rule is
    /// served by its tab's loop; a profile rule by ANY live loop of its
    /// profile. Recomputed wholesale after every state change — the loops set
    /// is tiny and one derivation beats five hand-synced writers (判据 §1).
    fn refresh_active(&self) {
        let live: Vec<(String, String)> = lock(&self.loops)
            .iter()
            .filter(|(_, h)| !h.is_finished())
            .map(|(k, _)| k.clone())
            .collect();
        for rule in lock(&self.rules).iter() {
            let active = match rule.scope {
                RouteScope::Tab => live
                    .iter()
                    .any(|(p, t)| p == &rule.profile && Some(t.as_str()) == rule.tab_id.as_deref()),
                RouteScope::Profile => live.iter().any(|(p, _)| p == &rule.profile),
            };
            rule.active.store(active, Ordering::Relaxed);
        }
    }
}

/// How many consecutive answer failures mean "this session is dead". ONE
/// failure is an orphan (Review Focus #3); a session that rejects every answer
/// is gone, and the loop exits rather than spinning (Review Focus #2).
const ANSWER_FAILURE_LIMIT: u32 = 3;

/// Per-tab consumer of `Fetch.requestPaused`: decide in strict order — SSRF
/// first, rules second (spec §3) — and answer. `events` was subscribed by the
/// caller BEFORE `Fetch.enable` was sent (a connection's broadcast starts at
/// subscription, so subscribing inside the task would open a gap).
///
/// Exit paths, all clean: answers fail [`ANSWER_FAILURE_LIMIT`] times in a
/// row, or the event stream closes. The FIRST is the real detector and covers
/// both deaths named in Review Focus #2 — a dead session rejects every
/// answer, and a dead ENGINE fails every answer too. The second is nominal,
/// never observed in practice: the broadcast's sender lives in the
/// connection's `Shared`, which this loop's own `conn` clone keeps alive, so
/// `EventStream::next() == None` is unreachable here (recording.rs's
/// `engine_gone` watch names the same fact from the other side — the close
/// watch, not the event stream, is the only socket-death signal a subscriber
/// can observe). Either way the
/// registry entry is removed on the way out, so a later `ensure` re-arms
/// instead of mistaking a corpse for a running loop.
async fn run_loop(
    conn: CdpConnection,
    mut events: EventStream,
    session: SessionId,
    profile: String,
    tab_id: String,
    registry: Arc<RouteRegistry>,
    ssrf: Arc<BrowserSsrfGuard>,
) {
    let mut last_lagged = 0u64;
    let mut answer_failures = 0u32;
    while let Some(ev) = events.next().await {
        let lagged = events.lagged();
        if lagged > last_lagged {
            // Deliberate deviation from spec §3's "对积压 requestId 批量 continue
            // 排干": a lagged broadcast has already dropped the event BODIES, so
            // the requestIds are unknowable and there is nothing to drain. The
            // honest behaviour is the audit trail; the dropped requests end on
            // the engine's own timeout.
            tracing::warn!(
                tab_id = %tab_id,
                lagged,
                "intercept loop lagged — paused requests were dropped unanswered; \
                 the engine's own timeout ends them"
            );
            last_lagged = lagged;
        }
        if ev.method != "Fetch.requestPaused" || ev.session.as_ref() != Some(&session) {
            continue;
        }
        let paused = match fetch::request_paused(&ev.params) {
            Ok(p) => p,
            Err(e) => {
                // No requestId, no answer possible; the engine's own timeout
                // ends the request. Not a strike — nothing we sent failed.
                tracing::debug!("undecodable Fetch.requestPaused, left to the engine: {e}");
                continue;
            }
        };
        // ① SSRF — unconditional, before any rule. A veto NEVER falls back to
        // continue (Review Focus #1): continuing would release the request to
        // the internal network. The only endings are fail or hang.
        if ssrf.check_url(&paused.request.url).await.is_err() {
            let mut answered = false;
            for _ in 0..2 {
                if fetch::fail_request(
                    &conn,
                    Some(&session),
                    &paused.request_id,
                    fetch::FailReason::BlockedByClient,
                )
                .await
                .is_ok()
                {
                    answered = true;
                    break;
                }
            }
            if answered {
                answer_failures = 0;
            } else {
                answer_failures += 1;
            }
        } else {
            // ② Rules. Internal faults fail OPEN to continue (spec §3: the page
            // never hangs on Aleph's account) — the SSRF path above is the sole
            // exemption.
            let outcome = match registry.match_lifo(
                &profile,
                &tab_id,
                &paused.request.url,
                &paused.request.method,
            ) {
                None => fetch::continue_request(&conn, Some(&session), &paused.request_id).await,
                Some(hit) => {
                    tracing::debug!(
                        rule = %hit.info.id,
                        url = %paused.request.url,
                        "mock route matched"
                    );
                    match &hit.kind {
                        RouteKind::Mock {
                            status,
                            headers,
                            body,
                        } => {
                            match fetch::fulfill_request(
                                &conn,
                                Some(&session),
                                &paused.request_id,
                                *status,
                                headers,
                                body,
                            )
                            .await
                            {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    tracing::debug!(
                                        "fulfill failed ({e}); failing open to continue"
                                    );
                                    fetch::continue_request(
                                        &conn,
                                        Some(&session),
                                        &paused.request_id,
                                    )
                                    .await
                                }
                            }
                        }
                        RouteKind::Abort => {
                            match fetch::fail_request(
                                &conn,
                                Some(&session),
                                &paused.request_id,
                                fetch::FailReason::Failed,
                            )
                            .await
                            {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    tracing::debug!(
                                        "abort answer failed ({e}); failing open to continue"
                                    );
                                    fetch::continue_request(
                                        &conn,
                                        Some(&session),
                                        &paused.request_id,
                                    )
                                    .await
                                }
                            }
                        }
                    }
                }
            };
            match outcome {
                Ok(()) => answer_failures = 0,
                Err(_) => answer_failures += 1,
            }
        }
        if answer_failures >= ANSWER_FAILURE_LIMIT {
            tracing::warn!(
                tab_id = %tab_id,
                "intercept loop: {ANSWER_FAILURE_LIMIT} consecutive answer failures — \
                 the session is presumed dead; exiting"
            );
            break;
        }
    }
    registry.take_loop(&profile, &tab_id);
}

impl CdpBackend {
    /// The shared mock-route table (owned by `ProfileManager`; this backend is
    /// rebuilt per call, so the table arrives as an `Arc`).
    pub(crate) fn routes(&self) -> &Arc<RouteRegistry> {
        &self.routes
    }

    /// Arm interception on `tab_id` iff a rule applies to it — idempotent, and
    /// free when no rule applies (spec §2's zero-overhead clause: no
    /// `Fetch.enable`, no task).
    ///
    /// The event subscription is taken BEFORE `Fetch.enable` is sent: an event
    /// caused by the enable is only visible to a subscriber that already
    /// exists. The spawn is serialised per registry so two concurrent arms
    /// cannot both spawn (a doubled loop would answer every paused request
    /// twice and double-count every hit).
    ///
    /// Fails CLOSED: a rule that cannot be armed errors the caller — a
    /// navigation that proceeded anyway would serve the page unmocked,
    /// silently ignoring the rule the model registered.
    pub(crate) async fn ensure_intercept_loop(
        &self,
        tab_id: &str,
        session: &SessionId,
    ) -> Result<(), BrowserError> {
        let profile = self.profile_name().to_string();
        let _serial = self.routes.start_lock().await;
        self.routes.reap_dead_loop(&profile, tab_id);
        if self.routes.loop_running(&profile, tab_id) {
            return Ok(());
        }
        if !self.routes.has_rules_for(&profile, tab_id) {
            return Ok(());
        }
        let handle = self.handle().await?;
        let events = handle.conn.events();
        fetch::enable(&handle.conn, Some(session))
            .await
            .map_err(|e| map_cdp_err(self.engine(), "Fetch.enable", e))?;
        let task = tokio::spawn(run_loop(
            handle.conn.clone(),
            events,
            session.clone(),
            profile.clone(),
            tab_id.to_string(),
            self.routes.clone(),
            self.ssrf_guard.clone(),
        ));
        self.routes
            .register_loop(&profile, tab_id, task.abort_handle());
        tracing::info!(tab_id, profile = %profile, "mock-route interception armed");
        Ok(())
    }

    /// The disarm half of the zero-overhead clause. "Idle" means the loop can
    /// never serve again: no rule applies to `tab_id` any more, OR the tab is
    /// gone from the table (a closed tab's session is dead — its loop could
    /// only ever error). `Fetch.disable` is sent when the tab is still there
    /// to hear it; either way the task is ended. Best-effort — a dead engine
    /// makes the disable moot, and the loop's own session-death exit covers
    /// the rest.
    pub(crate) async fn stop_intercept_loop_if_idle(&self, tab_id: &str) {
        let profile = self.profile_name();
        let tab_alive = match self.handle().await {
            Ok(handle) => handle.ensure_tab(tab_id).await.ok(),
            Err(_) => None,
        };
        if tab_alive.is_some() && self.routes.has_rules_for(profile, tab_id) {
            return;
        }
        let Some(abort) = self.routes.take_loop(profile, tab_id) else {
            return;
        };
        if let Some(session) = tab_alive {
            if let Ok(handle) = self.handle().await {
                let _ = fetch::disable(&handle.conn, Some(&session)).await;
            }
        }
        abort.abort();
    }

    /// `browser_network{action:"mock_add"}`: register the rule, then ARM
    /// whoever it applies to — registration without arming was T2's wiring
    /// debt (its hook covers `navigate()` only, and `history()` never passes
    /// through it), and a rule that sits in the table unserved reads exactly
    /// like a served one everywhere but the `active` flag.
    ///
    /// Rollback, not best-effort: if the arm fails (an engine that refuses
    /// `Fetch.enable`), the rule is removed again and the caller gets the
    /// error — the model should not have to audit a flag to learn its
    /// registration never took effect (判据 §8).
    pub(crate) async fn route_add(
        &self,
        tab_id: &str,
        rule: NewRouteRule,
    ) -> Result<RouteRuleInfo, BrowserError> {
        // The capability gate runs BEFORE the registry: an engine whose Fetch
        // path is unprobed (obscura today) refuses before any state changes.
        // Only `route_add` is gated — list/remove/clear are pure table
        // operations that must stay reachable, or rules registered under one
        // engine could never be cleaned up after a `switch_engine`.
        super::require(
            crate::browser::engine::capabilities(self.engine()),
            self.engine(),
            |c| c.network_interception,
            "route_add",
        )?;
        let profile = self.profile_name().to_string();
        let scope = rule.scope;
        let info = self
            .routes
            .add(
                &profile,
                tab_id,
                &rule.url_contains,
                rule.method.as_deref(),
                rule.kind,
                scope,
                rule.note,
            )
            .map_err(|e| BrowserError::ActionFailed(e.to_string()))?;
        if let Err(e) = self.arm_applicable_tabs(scope, tab_id).await {
            let _ = self.routes.remove(&info.id);
            self.disarm_idle_loops().await;
            return Err(e);
        }
        Ok(info)
    }

    /// `browser_network{action:"mock_list"}` — the profile's table, as-is.
    pub(crate) async fn route_list(&self) -> Result<Vec<RouteRuleInfo>, BrowserError> {
        Ok(self.routes.list(self.profile_name()))
    }

    /// `browser_network{action:"mock_remove"}` — remove by id. An unknown id
    /// is a model mistake, so it is an error naming the id, not a quiet Ok.
    pub(crate) async fn route_remove(&self, rule_id: &str) -> Result<RouteRuleInfo, BrowserError> {
        let info = self.routes.remove(rule_id).ok_or_else(|| {
            BrowserError::ActionFailed(format!(
                "no mock route {rule_id:?} — browser_network{{action:\"mock_list\"}} \
                 lists the live rule ids"
            ))
        })?;
        self.disarm_idle_loops().await;
        Ok(info)
    }

    /// `browser_network{action:"mock_clear"}` — the scope picks WHICH half of
    /// the table goes (the tool layer makes the scope mandatory, so a clear
    /// is never ambiguous).
    pub(crate) async fn route_clear(
        &self,
        tab_id: &str,
        scope: RouteScope,
    ) -> Result<usize, BrowserError> {
        let profile = self.profile_name();
        let cleared = match scope {
            RouteScope::Tab => self.routes.clear_tab(profile, tab_id),
            RouteScope::Profile => self.routes.clear_profile(profile),
        };
        self.disarm_idle_loops().await;
        Ok(cleared)
    }

    /// Arm the tabs `scope` + `tab_id` make a fresh rule applicable to: the
    /// one tab for `Tab`; EVERY live tab of the profile for `Profile` (T2's
    /// wiring debt, obligation 2 — the navigate hook arms only FUTURE
    /// navigations, so without this replay an already-open tab would wait
    /// for its next `browser_navigate` before the rule served it).
    async fn arm_applicable_tabs(
        &self,
        scope: RouteScope,
        tab_id: &str,
    ) -> Result<(), BrowserError> {
        let handle = self.handle().await?;
        match scope {
            RouteScope::Tab => {
                let session = handle.ensure_tab(tab_id).await?;
                self.ensure_intercept_loop(tab_id, &session).await
            }
            RouteScope::Profile => {
                let tab_ids: Vec<String> =
                    handle.tabs.lock().await.entries.keys().cloned().collect();
                for id in tab_ids {
                    let session = handle.ensure_tab(&id).await?;
                    self.ensure_intercept_loop(&id, &session).await?;
                }
                Ok(())
            }
        }
    }

    /// After a removal, every live tab whose applicable rule set went to zero
    /// gets disarmed — the zero-overhead clause's tool-layer half (T2 wired
    /// the same check into `close_tab`; removal paths must not wait for a
    /// tab to die). Best-effort like its sibling: a dead engine makes the
    /// disable moot, and the loop's own session-death exit covers the rest.
    async fn disarm_idle_loops(&self) {
        let Ok(handle) = self.handle().await else {
            return;
        };
        let tab_ids: Vec<String> = handle.tabs.lock().await.entries.keys().cloned().collect();
        for tab_id in tab_ids {
            self.stop_intercept_loop_if_idle(&tab_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_cdp::{CdpConnection, ConnectOptions, SessionId};
    use serde_json::{json, Value};

    use super::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;

    fn mock_json(status: u16, tag: &str) -> RouteKind {
        RouteKind::Mock {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: format!("\"{tag}\"").into_bytes(),
        }
    }

    /// A `Fetch.requestPaused` frame as the engine would push it.
    fn paused(request_id: &str, url: &str, method: &str, session: &str) -> Value {
        json!({
            "method": "Fetch.requestPaused",
            "sessionId": session,
            "params": {
                "requestId": request_id,
                "request": {"url": url, "method": method}
            }
        })
    }

    async fn connect(server: &FakeCdpServer) -> CdpConnection {
        CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect("connect to the fake server")
    }

    /// Poll `cond` for up to 2s. Assertions about "the loop answered" race the
    /// spawned task; polling turns the race into a bounded wait.
    async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// A live loop driving a real connection to `server`. The test owns the
    /// JoinHandle, so "the loop exited" is an awaitable fact, not a guess.
    fn spawn_loop(
        conn: &CdpConnection,
        registry: &Arc<RouteRegistry>,
        guard: Arc<crate::browser::network_policy::BrowserSsrfGuard>,
    ) -> tokio::task::JoinHandle<()> {
        let events = conn.events();
        let handle = tokio::spawn(run_loop(
            conn.clone(),
            events,
            SessionId("S1".to_string()),
            "p".to_string(),
            "t1".to_string(),
            registry.clone(),
            guard,
        ));
        registry.register_loop("p", "t1", handle.abort_handle());
        handle
    }

    #[test]
    fn matching_is_lifo_and_tab_rules_beat_profile_rules() {
        let reg = RouteRegistry::new();
        let r1 = reg
            .add(
                "p",
                "t1",
                "api",
                None,
                mock_json(200, "profile-old"),
                RouteScope::Profile,
                None,
            )
            .expect("add r1");
        let r2 = reg
            .add(
                "p",
                "t1",
                "api",
                None,
                mock_json(200, "tab-new"),
                RouteScope::Tab,
                None,
            )
            .expect("add r2");
        let r3 = reg
            .add(
                "p",
                "t1",
                "api",
                None,
                mock_json(200, "tab-newer"),
                RouteScope::Tab,
                None,
            )
            .expect("add r3");
        assert_eq!(
            (r1.id.as_str(), r2.id.as_str(), r3.id.as_str()),
            ("r1", "r2", "r3")
        );

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
        reg.add(
            "p",
            "t1",
            "api",
            Some("post"),
            mock_json(200, "x"),
            RouteScope::Tab,
            None,
        )
        .expect("add");
        assert!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "GET")
                .is_none(),
            "a POST-only rule must not catch GET"
        );
        assert!(
            reg.match_lifo("p", "t1", "https://a.test/api/x", "POST")
                .is_some(),
            "method input is normalised, lowercase `post` still narrows to POST"
        );
        // substring, not anchored (R8): the needle may land anywhere.
        assert!(reg
            .match_lifo("p", "t1", "https://deep.example/prefix/api/x?q=1", "POST")
            .is_some());
        assert!(reg
            .match_lifo("p", "t1", "https://a.test/other", "POST")
            .is_none());
    }

    #[test]
    fn hits_are_counted_on_the_rule_that_matched() {
        let reg = RouteRegistry::new();
        let profile_rule = reg
            .add(
                "p",
                "t1",
                "api",
                None,
                mock_json(200, "p"),
                RouteScope::Profile,
                None,
            )
            .expect("add");
        let tab_rule = reg
            .add(
                "p",
                "t1",
                "api",
                None,
                mock_json(200, "t"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        reg.match_lifo("p", "t1", "https://a.test/api/1", "GET");
        reg.match_lifo("p", "t1", "https://a.test/api/2", "GET");
        let listed = reg.list("p");
        let tab_info = listed.iter().find(|i| i.id == tab_rule.id).expect("listed");
        let profile_info = listed
            .iter()
            .find(|i| i.id == profile_rule.id)
            .expect("listed");
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
            .add(
                "p",
                "t1",
                "",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect_err("empty url_contains is refused");
        assert_eq!(err, RouteRuleError::EmptyUrlContains);
        assert!(
            reg.list("p").is_empty(),
            "a refused rule never entered the table"
        );
    }

    #[test]
    fn profile_rules_do_not_leak_across_profiles() {
        // spec §4: scope=profile means "this profile's tabs", not "every tab in
        // the daemon". A mock that bled into another principal's browser would
        // be an isolation breach, not a feature.
        let reg = RouteRegistry::new();
        reg.add(
            "alice",
            "t1",
            "api",
            None,
            mock_json(200, "a"),
            RouteScope::Profile,
            None,
        )
        .expect("add");
        assert!(
            reg.match_lifo("bob", "t9", "https://a.test/api", "GET")
                .is_none(),
            "alice's profile rule must not serve bob's tab"
        );
        assert!(
            reg.match_lifo("alice", "t9", "https://a.test/api", "GET")
                .is_some(),
            "a profile rule serves ANY tab of its own profile"
        );
        reg.add(
            "alice",
            "t1",
            "secret",
            None,
            mock_json(200, "s"),
            RouteScope::Tab,
            None,
        )
        .expect("add");
        assert!(
            reg.match_lifo("alice", "t2", "https://a.test/secret", "GET")
                .is_none(),
            "a tab rule binds one tab, not the profile"
        );
    }

    #[test]
    fn rule_ids_are_monotonic_and_never_reused() {
        // spec §4: the model quotes rule ids back; a reused id would let
        // `mock_remove r1` delete a rule it never meant.
        let reg = RouteRegistry::new();
        let first = reg
            .add(
                "p",
                "t1",
                "a",
                None,
                mock_json(200, "1"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        assert!(reg.remove(&first.id).is_some());
        let second = reg
            .add(
                "p",
                "t1",
                "a",
                None,
                mock_json(200, "2"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        assert_ne!(first.id, second.id, "ids are not recycled");
        assert_eq!(second.id, "r2");
    }

    #[test]
    fn clear_tab_and_clear_profile_remove_only_their_own_scope() {
        let reg = RouteRegistry::new();
        reg.add(
            "p",
            "t1",
            "a",
            None,
            mock_json(200, "t1"),
            RouteScope::Tab,
            None,
        )
        .expect("add");
        reg.add(
            "p",
            "t2",
            "a",
            None,
            mock_json(200, "t2"),
            RouteScope::Tab,
            None,
        )
        .expect("add");
        reg.add(
            "p",
            "t1",
            "b",
            None,
            mock_json(200, "p"),
            RouteScope::Profile,
            None,
        )
        .expect("add");
        assert_eq!(reg.clear_tab("p", "t1"), 1, "one tab rule for t1");
        assert_eq!(
            reg.list("p").len(),
            2,
            "t2's tab rule and the profile rule survive"
        );
        assert_eq!(reg.clear_profile("p"), 1, "one profile rule");
        assert_eq!(reg.list("p").len(), 1, "tab rules survive a profile clear");
        // A profile clear never touches another profile.
        reg.add(
            "q",
            "t9",
            "b",
            None,
            mock_json(200, "q"),
            RouteScope::Profile,
            None,
        )
        .expect("add");
        assert_eq!(reg.clear_profile("p"), 0);
        assert_eq!(reg.list("q").len(), 1);
    }

    #[test]
    fn list_reports_kind_label_scope_hits_and_active() {
        let reg = RouteRegistry::new();
        let mock = reg
            .add(
                "p",
                "t1",
                "a",
                Some("GET"),
                mock_json(201, "m"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let abort = reg
            .add(
                "p",
                "t1",
                "b",
                None,
                RouteKind::Abort,
                RouteScope::Profile,
                None,
            )
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
        reg.add(
            "p",
            "t2",
            "a",
            None,
            mock_json(200, "x"),
            RouteScope::Tab,
            None,
        )
        .expect("add");
        assert!(
            !reg.has_rules_for("p", "t1"),
            "another tab's rule does not count"
        );
        assert!(reg.has_rules_for("p", "t2"));
        reg.add(
            "p",
            "t1",
            "a",
            None,
            mock_json(200, "x"),
            RouteScope::Profile,
            None,
        )
        .expect("add");
        assert!(
            reg.has_rules_for("p", "t1"),
            "a profile rule counts for every tab"
        );
    }

    // ===================== InterceptLoop (FakeCdpServer-driven) =====================

    #[tokio::test]
    async fn a_paused_request_matching_no_rule_is_continued_verbatim() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        let task = spawn_loop(&conn, &registry, open_guard());

        server.push_event(paused("r1", "http://93.184.216.34/api/x", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.continueRequest").len() == 1).await,
            "a non-matching request is released verbatim"
        );
        assert_eq!(
            server.last_params("Fetch.continueRequest").expect("frame")["requestId"],
            "r1"
        );
        task.abort();
    }

    #[tokio::test]
    async fn a_paused_request_matching_a_mock_rule_is_fulfilled() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        registry
            .add(
                "p",
                "t1",
                "/api/",
                None,
                mock_json(200, "mocked"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let task = spawn_loop(&conn, &registry, open_guard());

        server.push_event(paused("r7", "http://93.184.216.34/api/users", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.fulfillRequest").len() == 1).await,
            "a matching request is answered with the mock"
        );
        let params = server.last_params("Fetch.fulfillRequest").expect("frame");
        assert_eq!(params["requestId"], "r7");
        assert_eq!(params["responseCode"], 200);
        assert_eq!(
            params["body"], "Im1vY2tlZCI=",
            "base64 of the mock body `\"mocked\"` — encoded on the wire, raw in the rule"
        );
        assert!(server.received_for("Fetch.continueRequest").is_empty());
        assert_eq!(
            registry.list("p")[0].hits,
            1,
            "the serving rule is credited with the hit"
        );
        task.abort();
    }

    #[tokio::test]
    async fn a_paused_request_matching_an_abort_rule_is_failed() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        registry
            .add(
                "p",
                "t1",
                "ads.",
                None,
                RouteKind::Abort,
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let task = spawn_loop(&conn, &registry, open_guard());

        server.push_event(paused("r8", "http://93.184.216.34/ads.js", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.failRequest").len() == 1).await,
            "an abort rule fails the request"
        );
        assert_eq!(
            server.last_params("Fetch.failRequest").expect("frame")["errorReason"],
            "Failed"
        );
        task.abort();
    }

    /// Review Focus #1, first half: a matching mock rule must NEVER outrank
    /// the SSRF veto.
    #[tokio::test]
    async fn ssrf_veto_beats_a_matching_mock_rule() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        registry
            .add(
                "p",
                "t1",
                "169.254.169.254",
                None,
                mock_json(200, "creds"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let task = spawn_loop(&conn, &registry, default_guard());

        server.push_event(paused(
            "r9",
            "http://169.254.169.254/latest/meta-data",
            "GET",
            "S1",
        ));
        assert!(
            wait_until(|| server.received_for("Fetch.failRequest").len() == 1).await,
            "the vetoed request is failed"
        );
        assert_eq!(
            server.last_params("Fetch.failRequest").expect("frame")["errorReason"],
            "BlockedByClient"
        );
        assert!(
            server.received_for("Fetch.fulfillRequest").is_empty(),
            "the mock rule matched but was NOT served — SSRF outranks every rule"
        );
        assert!(server.received_for("Fetch.continueRequest").is_empty());
        assert_eq!(
            registry.list("p")[0].hits,
            0,
            "the veto ran BEFORE matching: no hit is credited"
        );
        task.abort();
    }

    /// Review Focus #1, second half: when answering the veto ITSELF fails, the
    /// only acceptable endings are "retry the fail" and "leave it hanging" —
    /// continuing would release the request to the internal network.
    #[tokio::test]
    async fn an_ssrf_veto_never_falls_back_to_continue() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![(
            "Fetch.failRequest",
            Responder::Error {
                code: -32000,
                message: "injected answer failure".to_string(),
            },
        )]))
        .await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        let task = spawn_loop(&conn, &registry, default_guard());

        server.push_event(paused("r1", "http://169.254.169.254/latest", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.failRequest").len() == 2).await,
            "the failed veto is retried once, then given up on"
        );
        // Settle, then prove the one thing that must never happen did not.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            server.received_for("Fetch.continueRequest").is_empty(),
            "a vetoed request must NEVER be continued — that is release to the internal network"
        );
        // The loop survived the double failure and still serves the next event.
        server.push_event(paused("r2", "http://93.184.216.34/ok", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.continueRequest").len() == 1).await,
            "one stubborn veto does not wedge the loop"
        );
        task.abort();
    }

    /// Review Focus #2: the tab dies mid-decision; every answer errors. The
    /// loop must recognise the dead session and EXIT — an error-spinning task
    /// would burn the runtime for the rest of the profile's life.
    #[tokio::test]
    async fn the_loop_exits_when_its_session_dies_mid_decision() {
        let dead = Responder::Error {
            code: -32001,
            message: "Session with given id not found".to_string(),
        };
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![
            ("Fetch.fulfillRequest", dead.clone()),
            ("Fetch.continueRequest", dead),
        ]))
        .await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        registry
            .add(
                "p",
                "t1",
                "/api/",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let task = spawn_loop(&conn, &registry, open_guard());
        assert!(
            registry.list("p")[0].active,
            "a live loop means the rule reads active"
        );

        for i in 1..=3 {
            server.push_event(paused(
                &format!("r{i}"),
                "http://93.184.216.34/api/x",
                "GET",
                "S1",
            ));
        }
        let joined = tokio::time::timeout(Duration::from_secs(2), task).await;
        assert!(
            joined.is_ok(),
            "the loop must exit within the strike budget, not spin on a dead session"
        );
        assert!(
            !registry.loop_running("p", "t1"),
            "the dead loop cleans its own registry entry"
        );
        assert!(
            !registry.list("p")[0].active,
            "the rule stays in the table but reads inactive — present-but-unserved is a fact"
        );
    }

    /// Review Focus #3: a `requestPaused` the engine queued BEFORE
    /// `Fetch.disable` lands arrives afterwards; answering it errors. That
    /// error is not a fault — the loop must tolerate it and serve the next one.
    #[tokio::test]
    async fn an_orphan_paused_event_after_disable_is_tolerated() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        server.on(
            "Fetch.continueRequest",
            Responder::Error {
                code: -32000,
                message: "Invalid InterceptionId.".to_string(),
            },
        );
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        let task = spawn_loop(&conn, &registry, open_guard());

        server.push_event(paused("orphan", "http://93.184.216.34/a", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.continueRequest").len() == 1).await,
            "the orphan was answered (and the answer errored engine-side)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !task.is_finished(),
            "one orphan answer error must not kill the loop"
        );

        // The engine answers normally again: the loop still serves.
        server.on("Fetch.continueRequest", Responder::Reply(json!({})));
        server.push_event(paused("r2", "http://93.184.216.34/b", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.continueRequest").len() == 2).await,
            "the next request is served after the orphan"
        );
        assert!(!task.is_finished());
        task.abort();
    }

    /// Internal faults fail OPEN: a fulfill that errors on the wire releases
    /// the request to the real upstream rather than hanging the page (spec §3).
    /// Only the SSRF path is exempt, and it is pinned shut by the veto tests.
    #[tokio::test]
    async fn a_fulfill_that_errors_fails_open_to_continue() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![(
            "Fetch.fulfillRequest",
            Responder::Error {
                code: -32000,
                message: "injected fulfill failure".to_string(),
            },
        )]))
        .await;
        let conn = connect(&server).await;
        let registry = Arc::new(RouteRegistry::new());
        registry
            .add(
                "p",
                "t1",
                "/api/",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        let task = spawn_loop(&conn, &registry, open_guard());

        server.push_event(paused("r5", "http://93.184.216.34/api/x", "GET", "S1"));
        assert!(
            wait_until(|| server.received_for("Fetch.continueRequest").len() == 1).await,
            "a failed fulfill falls back to continue — the page never hangs on Aleph's account"
        );
        task.abort();
    }

    // ===================== the navigate hook (Review Focus #5) =====================

    /// Drive a navigation to completion: the fake answers `Page.navigate`, and
    /// the load barrier is ended by a pushed `Page.loadEventFired` once the
    /// navigate frame is on the wire.
    async fn navigate_to_completion(
        server: &FakeCdpServer,
        backend: Arc<crate::browser::cdp_backend::CdpBackend>,
        tab_id: &str,
        url: &str,
    ) -> Result<(), crate::browser::error::BrowserError> {
        let tab = tab_id.to_string();
        let url = url.to_string();
        let nav = tokio::spawn(async move { backend.navigate(&tab, &url).await });
        assert!(
            wait_until(|| server.received_for("Page.navigate").len() == 1).await,
            "the navigation was dispatched"
        );
        server.push_event(json!({
            "method": "Page.loadEventFired",
            "sessionId": "S1",
            "params": {}
        }));
        nav.await.expect("navigate task joined")
    }

    async fn backend_with_tab(
        server: &FakeCdpServer,
    ) -> (
        Arc<crate::browser::cdp_backend::CdpBackend>,
        Arc<RouteRegistry>,
    ) {
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L1" })),
        );
        let (_reg, backend) = backend_with(server, Engine::Chromium, open_guard()).await;
        let backend = Arc::new(backend);
        let handle = backend.handle().await.expect("pre-seeded handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach T1");
        let routes = backend.routes().clone();
        (backend, routes)
    }

    /// Review Focus #5: with a profile rule registered, a tab's FIRST
    /// navigation must already be intercepted — `Fetch.enable` reaches the
    /// engine before `Page.navigate`, or the first request slips past.
    #[tokio::test]
    async fn profile_rules_are_armed_before_the_tabs_first_navigation() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        routes
            .add(
                "default",
                "T1",
                "api",
                None,
                mock_json(200, "armed"),
                RouteScope::Profile,
                None,
            )
            .expect("add");

        navigate_to_completion(&server, backend, "T1", "https://ok.example/")
            .await
            .expect("navigation completes");

        let order = methods(&server);
        let enable_at = order
            .iter()
            .position(|m| m == "Fetch.enable")
            .expect("Fetch.enable was sent");
        let nav_at = order
            .iter()
            .position(|m| m == "Page.navigate")
            .expect("Page.navigate was sent");
        assert!(
            enable_at < nav_at,
            "Fetch.enable must reach the engine before the first Page.navigate: {order:?}"
        );
        assert!(routes.loop_running("default", "T1"));
        assert!(
            routes.list("default").iter().all(|i| i.active),
            "a served rule reads active"
        );
    }

    /// spec §2's zero-overhead clause: no applicable rules → no `Fetch.enable`,
    /// no task. An engine without Fetch must never notice the feature exists.
    #[tokio::test]
    async fn zero_rules_means_no_fetch_enable_and_no_loop() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;

        navigate_to_completion(&server, backend, "T1", "https://ok.example/")
            .await
            .expect("navigation completes");

        assert!(
            server.received_for("Fetch.enable").is_empty(),
            "zero rules → the engine is never asked for Fetch"
        );
        assert!(!routes.loop_running("default", "T1"));
    }

    /// Scope precision at the hook: a rule bound to ANOTHER tab must not arm
    /// interception on this one.
    #[tokio::test]
    async fn a_tab_rule_for_another_tab_does_not_arm_this_tab() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        routes
            .add(
                "default",
                "OTHER",
                "api",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect("add");

        navigate_to_completion(&server, backend, "T1", "https://ok.example/")
            .await
            .expect("navigation completes");

        assert!(
            server.received_for("Fetch.enable").is_empty(),
            "T1 has no applicable rule — OTHER's tab rule does not count"
        );
        assert!(!routes.loop_running("default", "T1"));
    }

    /// Fail-closed at the hook: a rule that cannot be armed (the engine
    /// refuses `Fetch.enable`) must FAIL the navigation — navigating anyway
    /// would serve the page unmocked, silently ignoring the model's rule.
    #[tokio::test]
    async fn a_rule_that_cannot_be_armed_fails_the_navigation_loudly() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Fetch.enable",
            Responder::Error {
                code: -32601,
                message: "'Fetch.enable' wasn't found".to_string(),
            },
        );
        let (backend, routes) = backend_with_tab(&server).await;
        routes
            .add(
                "default",
                "T1",
                "api",
                None,
                mock_json(200, "x"),
                RouteScope::Profile,
                None,
            )
            .expect("add");

        let err = backend
            .navigate("T1", "https://ok.example/")
            .await
            .expect_err("an unarmable rule must not navigate");
        assert!(
            format!("{err}").contains("Fetch.enable"),
            "the refusal names what could not be armed: {err}"
        );
        assert!(
            server.received_for("Page.navigate").is_empty(),
            "no navigation happened without interception"
        );
        assert!(!routes.loop_running("default", "T1"), "no half-armed loop");
    }

    /// The disarm half of the zero-overhead clause: removing a tab's last
    /// applicable rule sends `Fetch.disable` and ends the loop.
    #[tokio::test]
    async fn removing_the_last_rule_disarms_the_tab() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        let rule = routes
            .add(
                "default",
                "T1",
                "api",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect("add");

        navigate_to_completion(&server, backend.clone(), "T1", "https://ok.example/")
            .await
            .expect("navigation completes");
        assert!(
            routes.loop_running("default", "T1"),
            "armed by the navigate"
        );

        assert!(routes.remove(&rule.id).is_some());
        backend.stop_intercept_loop_if_idle("T1").await;

        assert!(
            wait_until(|| server.received_for("Fetch.disable").len() == 1).await,
            "Fetch.disable reached the engine"
        );
        assert!(!routes.loop_running("default", "T1"), "the loop is gone");
    }

    /// spec §2/§4 lifecycle: closing a tab drops its tab-scope rules and ends
    /// its interception loop — the task dies with the session.
    #[tokio::test]
    async fn closing_a_tab_drops_its_rules_and_its_loop() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Target.closeTarget",
            Responder::Reply(json!({ "success": true })),
        );
        let (backend, routes) = backend_with_tab(&server).await;
        routes
            .add(
                "default",
                "T1",
                "api",
                None,
                mock_json(200, "x"),
                RouteScope::Tab,
                None,
            )
            .expect("add");
        navigate_to_completion(&server, backend.clone(), "T1", "https://ok.example/")
            .await
            .expect("navigation completes");
        assert!(
            routes.loop_running("default", "T1"),
            "armed by the navigate"
        );
        assert_eq!(routes.list("default").len(), 1);

        backend.close_tab("T1").await.expect("close");

        assert!(
            routes.list("default").is_empty(),
            "tab-scope rules die with the tab"
        );
        assert!(
            !routes.loop_running("default", "T1"),
            "the loop is disarmed with the tab"
        );
    }

    // ===================== the tool-facing route_* verbs (Task 3) =====================

    fn new_rule(url_contains: &str, scope: RouteScope) -> NewRouteRule {
        NewRouteRule {
            url_contains: url_contains.to_string(),
            method: None,
            kind: mock_json(200, "x"),
            scope,
            note: None,
        }
    }

    /// T2's wiring debt, obligation 1: `navigate()` is not the only way a live
    /// tab reaches a matching URL (`history()` never passes through it, and an
    /// already-open page needs no navigation at all), so `mock_add` must arm
    /// the tab itself — a rule registered against an open page must not sit
    /// in the table serving nothing until the next `browser_navigate`.
    #[tokio::test]
    async fn route_add_on_a_live_tab_arms_it_without_a_navigate() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;

        let info = backend
            .route_add("T1", new_rule("api", RouteScope::Tab))
            .await
            .expect("add");
        assert_eq!(info.id, "r1");

        // The enable is awaited inside route_add, so it is already on the wire.
        assert_eq!(
            server.received_for("Fetch.enable").len(),
            1,
            "the live tab is armed at registration time, with no navigate"
        );
        assert!(routes.loop_running("default", "T1"));
        assert!(
            routes.list("default")[0].active,
            "a served rule reads active"
        );
    }

    /// T2's wiring debt, obligation 2 (the eager ruling): a profile-scope rule
    /// applies to every tab of the profile INCLUDING the ones already open, so
    /// registration replays the arm to each live tab — not just the tab the
    /// call happened to name.
    #[tokio::test]
    async fn route_add_with_profile_scope_arms_every_live_tab_of_the_profile() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T2".into()))
            .await
            .expect("attach T2");

        backend
            .route_add("T1", new_rule("api", RouteScope::Profile))
            .await
            .expect("add");

        assert_eq!(
            server.received_for("Fetch.enable").len(),
            2,
            "both live tabs are armed, not only the one the call named"
        );
        assert!(routes.loop_running("default", "T1"));
        assert!(routes.loop_running("default", "T2"));
    }

    /// Fail-closed, the same rule the navigate hook lives by: a rule that
    /// cannot be armed must not stay in the table pretending to be served —
    /// the registration is rolled back and the caller gets the error.
    #[tokio::test]
    async fn route_add_rolls_the_rule_back_when_the_engine_refuses_fetch() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Fetch.enable",
            Responder::Error {
                code: -32601,
                message: "'Fetch.enable' wasn't found".to_string(),
            },
        );
        let (backend, routes) = backend_with_tab(&server).await;

        let err = backend
            .route_add("T1", new_rule("api", RouteScope::Tab))
            .await
            .expect_err("an unarmable rule is refused");
        assert!(
            format!("{err}").contains("Fetch.enable"),
            "the refusal names what could not be armed: {err}"
        );
        assert!(
            routes.list("default").is_empty(),
            "the refused rule never stays in the table"
        );
        assert!(!routes.loop_running("default", "T1"), "no half-armed loop");
    }

    /// The capability row is the gate: obscura's `network_interception` is
    /// `Unsupported` (NOT_PROBED — the plan's Task 4 probe is the expiry
    /// check), so the refusal arrives BEFORE any Fetch handshake and names
    /// the engine that does serve the verb (判据 §14).
    #[tokio::test]
    async fn route_add_is_refused_on_an_engine_the_table_says_cannot_serve_it() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Obscura, open_guard()).await;

        let err = backend
            .route_add("T1", new_rule("api", RouteScope::Tab))
            .await
            .expect_err("obscura refuses before the wire");
        match err {
            BrowserError::UnsupportedByEngine {
                verb, supported_by, ..
            } => {
                assert_eq!(verb, "route_add");
                assert_eq!(supported_by, Some(Engine::Chromium));
            }
            other => panic!("expected UnsupportedByEngine, got {other:?}"),
        }
        assert!(
            server.received_for("Fetch.enable").is_empty(),
            "the refusal happens before any Fetch handshake"
        );
    }

    /// The zero-overhead clause at the TOOL layer: once nothing applies to a
    /// tab any more, `Fetch.disable` goes out and the loop ends — without
    /// waiting for a `close_tab`.
    #[tokio::test]
    async fn route_remove_of_the_last_applicable_rule_disarms_the_tab() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        let info = backend
            .route_add("T1", new_rule("api", RouteScope::Tab))
            .await
            .expect("add");
        assert_eq!(server.received_for("Fetch.enable").len(), 1);

        let removed = backend.route_remove(&info.id).await.expect("remove");
        assert_eq!(removed.id, info.id, "the confirmation names the rule");
        assert_eq!(removed.hits, 0);

        assert_eq!(
            server.received_for("Fetch.disable").len(),
            1,
            "Fetch.disable reached the engine"
        );
        assert!(!routes.loop_running("default", "T1"), "the loop is gone");
    }

    #[tokio::test]
    async fn route_clear_scopes_and_reports_counts() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, routes) = backend_with_tab(&server).await;
        backend
            .route_add("T1", new_rule("api", RouteScope::Tab))
            .await
            .expect("add tab rule");
        backend
            .route_add("T1", new_rule("api", RouteScope::Profile))
            .await
            .expect("add profile rule");
        assert_eq!(routes.list("default").len(), 2);

        let n = backend
            .route_clear("T1", RouteScope::Tab)
            .await
            .expect("clear tab scope");
        assert_eq!(n, 1, "only T1's tab rules were cleared");
        let remaining = routes.list("default");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].scope, RouteScope::Profile);
        assert!(
            routes.loop_running("default", "T1"),
            "the profile rule still applies — the loop stays armed"
        );
        assert!(server.received_for("Fetch.disable").is_empty());

        let n = backend
            .route_clear("T1", RouteScope::Profile)
            .await
            .expect("clear profile scope");
        assert_eq!(n, 1);
        assert!(routes.list("default").is_empty());
        assert_eq!(
            server.received_for("Fetch.disable").len(),
            1,
            "nothing applies any more — the tab is disarmed"
        );
        assert!(!routes.loop_running("default", "T1"));
    }

    /// Removing an id nobody registered is a model mistake, not a no-op: the
    /// error names the id so the model can re-list (判据 §8).
    #[tokio::test]
    async fn route_remove_of_an_unknown_id_fails_loudly() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (backend, _routes) = backend_with_tab(&server).await;

        let err = backend
            .route_remove("r99")
            .await
            .expect_err("an unknown id is not a silent success");
        assert!(format!("{err}").contains("r99"), "{err}");
    }
}
