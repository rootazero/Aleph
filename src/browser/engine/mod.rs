//! The two browser processes Aleph can drive, and the machinery both need.
//!
//! `Engine` is *what runs the page*; `BrowserDriver` (`super::profile`) is
//! *how Aleph talks to it*. They are orthogonal on purpose (spec §5.4): a
//! profile is an identity — its cookies, its data directory, its policy — and
//! the engine is the means, which the model may swap under a live profile.
//!
//! What lives here is only what BOTH engines need. Anything Chromium-shaped
//! (the `DevToolsActivePort` file, `--use-mock-keychain`) stays in
//! [`chromium`]; anything about a *record* of a launched process (the sidecar
//! registry, the orphan sweep) is in [`process`], because a sweep that had to
//! know which engine wrote a record before it could read it would need a
//! second derivation of that fact (判据 §12).

pub mod capability;
pub mod chromium;
pub mod process;
pub mod readiness;
pub mod registry;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use self::process::{EngineProcess, Launched};
use super::error::BrowserError;

pub use capability::{capabilities, supported_by, Cap, EngineCapabilities, CAP_FIELDS};

/// Declares the [`Engine`] variants **once** and emits both the enum and its
/// [`Engine::ALL`].
///
/// Without this, `ALL` was a hand-written array beside a hand-written enum —
/// two facts claiming to enumerate one set, with the array's own doc asserting
/// an invariant nothing checked (判据 §1, and the comment was the lying half).
/// It was measured rather than argued: adding a third variant produced **13
/// compile errors** and `ALL` was not one of them, so the natural repair (add
/// the variant, fix the 13 matches, green build) left five consumers iterating
/// two engines out of three — `locate_all` among them, returning 2 rows under
/// a doc claiming the count *is* the variant count, with no dead-code warning
/// because the compiler cannot know `ALL` excludes it.
///
/// **A test cannot be the fix.** Stable Rust cannot enumerate variants, so any
/// completeness guard needs a second hand-written list — the same defect again
/// — and a guard that iterates `ALL` cannot see `ALL` being short; it just
/// tests fewer things (判据 §3, §2's 恒绿 face). The macro removes the need for
/// a guard instead of adding one.
///
/// Each variant name is on its own line on purpose: a macro that hid `Obscura`
/// from `grep -n 'Obscura' src/browser/engine/mod.rs` would trade one silent
/// failure for another. The idiom is already in this subsystem — `obscura_tag!`
/// in `runtimes/specs.rs` exists for exactly this reason — so it adds no
/// dependency and no new pattern.
macro_rules! declare_engines {
    (
        $(
            $(#[$vmeta:meta])*
            $variant:ident
        ),+ $(,)?
    ) => {
        /// Which browser process backs a profile.
        ///
        /// The wire spelling is frozen the day it ships: it is a config value
        /// (`[general.browser.profiles.<name>] engine = "obscura"`), a
        /// `runtime_manage{capability}` value, and a field in every sidecar
        /// record on disk. `snake_case` matches `BrowserDriver`'s existing
        /// serde (`super::profile::BrowserDriver`, `profile.rs:22-30`).
        ///
        /// **Deliberately not a variant of `BrowserType`** (`profile.rs:13-19`):
        /// that enum answers "which member of the Chromium family", and obscura
        /// is not one (spec §6.3). Folding them would make `browser = "obscura"`
        /// parse into a value `discovery::find_chromium_preferred` would then
        /// hunt for on disk.
        #[derive(
            Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum Engine {
            $(
                $(#[$vmeta])*
                $variant,
            )+
        }

        impl Engine {
            /// Every engine, in the order tables and doctor rows render them.
            ///
            /// **Emitted by `declare_engines!` from the same variant list as
            /// the enum**, so a third engine reaches every enumerator by being
            /// declared, not by being remembered here as well (判据 §5). That
            /// sentence used to be a claim; the construct above it is now what
            /// makes it true, and deleting the macro is how you falsify it.
            pub const ALL: [Self; { [$(stringify!($variant)),+].len() }] =
                [$(Self::$variant),+];
        }
    };
}

declare_engines! {
    /// The default engine (spec §0): a small, Aleph-shaped browser installed
    /// from the runtime ledger.
    #[default]
    Obscura,
    /// The escape hatch: a Chromium-family browser, supplied at runtime.
    Chromium,
}

impl Engine {
    /// The wire spelling — the SAME string serde writes, derived from one
    /// place so a `#[serde(rename_all)]` change cannot leave the two
    /// disagreeing.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Obscura => "obscura",
            Self::Chromium => "chromium",
        }
    }

    /// The inverse of [`Self::as_str`], exact-match only.
    ///
    /// No case folding and no trimming: the callers are a config file serde
    /// already validated and a `runtime_manage{capability}` value, and a
    /// lenient parse here would accept a spelling serde rejects, so the two
    /// front doors would disagree about the same string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// This engine's leaf under `~/.aleph/data/browser/`.
    #[must_use]
    pub const fn data_subdir(self) -> &'static str {
        self.as_str()
    }

    /// The argv switch that names this engine's per-profile data directory.
    ///
    /// **This is the token the orphan sweep matches on before it kills**
    /// ([`process::reap_orphans`]), so it is not cosmetic: Chromium's
    /// `--user-data-dir` and obscura's `--storage-dir` (spec §6.2) are the
    /// only evidence that a pid recorded hours ago is still the process the
    /// record meant. A shared switch would let either engine's record
    /// authorise a SIGKILL against the other's process.
    #[must_use]
    pub const fn data_dir_flag(self) -> &'static str {
        match self {
            Self::Chromium => "--user-data-dir",
            Self::Obscura => "--storage-dir",
        }
    }

    /// The engine a sidecar record with no `engine` key must be read as.
    ///
    /// A separate, *named* function rather than `Default`: the product
    /// default is obscura and the compat default is Chromium, and those are
    /// different questions with different answers. A bare `#[serde(default)]`
    /// on the sidecar field would silently pick up the product default the
    /// day someone changes it, and every pre-upgrade Chromium record would
    /// then be swept with obscura's argv switch — i.e. never matched, never
    /// reaped, forever.
    #[must_use]
    pub const fn chromium_default() -> Self {
        Self::Chromium
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How long a kill waits for the child to actually die before saying it did
/// not. Short on purpose: this runs on the daemon's wedged-shutdown path.
pub const ENGINE_KILL_GRACE: Duration = Duration::from_millis(500);

/// The **wedged-exit path's** budget for the engine half of a shutdown, and
/// the default this crate's own tests spend.
///
/// Not "the" shutdown budget: `EngineRegistry::shutdown_all` takes the budget
/// as a parameter, because its two callers have opposite economics, and the
/// orderly caller derives its own from the watchdog it runs inside
/// (`start/helpers.rs`'s `ORDERLY_BROWSER_STOP_BUDGET`). Production in the
/// registry names no shutdown constant at all.
///
/// For the wedged path this is a hard ceiling, not a hope: that watchdog has
/// already spent `SHUTDOWN_FAILSAFE` before it reaches us and the
/// `std::process::exit(0)` after it waits for nobody.
///
/// "Hard ceiling" covers **taking the registry lock as well as the stops**, and
/// that is the whole content of the claim: `EngineRegistry::handle` holds that
/// lock across a launch (`chromium::DEVTOOLS_PORT_DEADLINE`, 30 s, plus an
/// unbounded binary resolve, plus `readiness::READY_GATE_BUDGET`), so a
/// deadline that started after the acquisition bounded only the kills and this
/// sentence was false by an order of magnitude while reading as true.
/// `shutdown_all` starts the clock before the lock for exactly that reason.
///
/// **Derived from [`ENGINE_KILL_GRACE`], not chosen.** It was a flat 1 s over a
/// loop that waited up to `ENGINE_KILL_GRACE` per engine *in sequence*, so
/// three stuck engines could not fit — an arithmetic impossibility nothing
/// stated (判据 §13: a limit's position and its arithmetic decide what it
/// actually limits). The loop is concurrent now, so the wall cost is ONE grace
/// window whatever N is, and this is that window plus the same again for the
/// socket closes and the sidecar unlinks. The two numbers cannot drift apart
/// because there is only one.
/// `the_shutdown_budget_is_derived_from_one_kill_grace` pins it.
pub const ENGINE_SHUTDOWN_BUDGET: Duration = ENGINE_KILL_GRACE.saturating_mul(2);

/// May this call start a browser?
///
/// A payload-free gate, deliberately NOT `super::playwright_launch::LaunchPolicy`.
/// That one borrows a `SessionLaunch`, and the engine path has already spent
/// those parameters in `super::manager::ProfileManager::launch_request_for` —
/// so the registry would be handed a value it reads half of, and every holder
/// of an `Arc<EngineRegistry>` (a CDP backend, which lives behind
/// `Arc<dyn BrowserBackend>`) would have to carry its lifetime. Two gates for
/// two drivers is not duplication: they gate different things, and the
/// playwright one keeps its payload because its caller has not derived a
/// request yet.
///
/// The reason either exists is the same, and it is `LaunchPolicy`'s own doc:
/// a sensor must not create what it measures.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineLaunch {
    /// Observing. A missing engine is an answer, not something to fix.
    Refuse,
    /// Asking for a browser. Launch one if the profile has none.
    Allow,
}

/// One live engine process plus the one CDP connection Aleph drives it with.
///
/// One per profile, held by [`registry::EngineRegistry`]. Backends stay
/// per-call (hot-swapping the SSRF policy depends on it); the handle is what
/// must not be rebuilt, because rebuilding it means launching a second
/// browser.
pub struct EngineHandle {
    pub engine: Engine,
    pub profile: String,
    pub launched: Launched,
    pub conn: aleph_cdp::CdpConnection,
    pub tabs: tokio::sync::Mutex<TabTable>,
    /// Whether this connection's event pump has been claimed.
    ///
    /// **Not bookkeeping for a double-spawn guard** — that is the smaller half
    /// of what it does. The three fields below it on [`TabEntry`] (`console`,
    /// `network`, `pending_dialog`) are filled by that pump and by nothing
    /// else, and an empty `VecDeque` answers *"the page logged nothing"* and
    /// *"nobody was listening"* with the same bytes. Only the second of those
    /// is a fail-closed answer, and consuming it as an observation is 判据 §8.
    /// This flag is the only thing that tells them apart, so
    /// `cdp_backend::events::render_ring` reads it and says which of the two
    /// the caller is looking at.
    ///
    /// Claimed by `cdp_backend::events::ensure_pump` through a
    /// compare-exchange, so two verbs resolving the same handle concurrently
    /// start one pump rather than two.
    pub pump_started: std::sync::atomic::AtomicBool,
    /// How many engine events the pump never saw, cumulative for this
    /// connection.
    ///
    /// The same 判据 §8 collapse as [`Self::pump_started`], one level down.
    /// That flag separates *"nobody was listening"* from *"nothing was
    /// logged"*; without this counter there is still no third answer for
    /// *"the pump ran and missed n"*, and a console or network ring with a
    /// silent hole in it reads as a complete observation.
    ///
    /// `aleph_cdp::EventStream::next` steps over a lag and counts it — `None`
    /// means only that the connection closed — so the count is the only
    /// evidence a gap exists. The pump publishes it here because the readers
    /// (`console_messages`, `network_log`) hold the handle and not the stream.
    ///
    /// Connection-wide, not per tab: the broadcast drops events for the
    /// subscriber, not for a tab, so which tab's lines went missing is not
    /// knowable. Reporting the gap on every tab of the engine over-reports,
    /// which is the safe direction for an "I may be incomplete" note.
    ///
    /// **Cumulative and monotonic, so it is not what a reader reports.**
    /// Reported raw it would be a 判据 §2 恒真 on the presentation layer — the
    /// most expensive place for one, because the model reads it: after a single
    /// busy page dropped events, every log read on every tab for the life of
    /// the connection would carry the gap note, including logs that really are
    /// complete. [`TabEntry::lag_at_attach`] is the subtrahend that turns this
    /// into a per-tab delta, so a tab attached after the drop reads clean.
    pub pump_lagged: std::sync::atomic::AtomicU64,
    process: Arc<dyn EngineProcess>,
}

/// The tabs of one engine. The key is the CDP `targetId` string.
#[derive(Default)]
pub struct TabTable {
    pub entries: HashMap<String, TabEntry>,
    pub active: Option<String>,
}

/// Per-tab state that outlives a single call.
pub struct TabEntry {
    pub session: aleph_cdp::SessionId,
    /// This tab's ref table. Beside the session because a ref's identity is
    /// `(frame, loader, backend node)` within THIS tab's document.
    pub refs: crate::browser::page_state::RefTable,
    /// Bumped once per page-state capture; refs carry the generation they were
    /// minted in (spec §4.3).
    pub generation: u64,
    /// The tab's current URL, as last observed. The post-navigation SSRF audit
    /// reads it, and it starts at `"about:blank"` because that is where
    /// [`EngineHandle::attach_tab`] finds a freshly created target — not
    /// `String::new()`, which would be an empty string standing in for "I do
    /// not know" and then getting vetted as a URL (判据 §8).
    pub url: String,
    pub console: VecDeque<String>,
    pub network: VecDeque<String>,
    pub pending_dialog: Option<String>,
    /// [`EngineHandle::pump_lagged`] at the moment this tab joined the table.
    ///
    /// The reader subtracts it, so the gap note describes **this tab's own
    /// window** rather than the connection's whole life. Without it the note is
    /// monotonic: one busy page load makes every later read on every tab say
    /// "there is a gap", including reads of rings that are genuinely complete,
    /// and a hint that is always on has stopped distinguishing anything.
    ///
    /// It does NOT decay for a tab that lived through a drop, and that is
    /// deliberate: that ring may really have a hole and nothing heals it. What
    /// the subtraction restores is the ability of a *fresh* tab to read clean.
    pub lag_at_attach: u64,
}

impl TabEntry {
    #[must_use]
    pub fn new(session: aleph_cdp::SessionId) -> Self {
        Self {
            session,
            refs: crate::browser::page_state::RefTable::new(),
            generation: 0,
            url: "about:blank".to_string(),
            console: VecDeque::new(),
            network: VecDeque::new(),
            pending_dialog: None,
            // Stamped by [`EngineHandle::attach_tab`], which is where the
            // connection's count is in scope. `0` here is right for the first
            // tab, which exists before any pump does.
            lag_at_attach: 0,
        }
    }
}

impl EngineHandle {
    /// Build a handle around an already-launched process and an already-ready
    /// connection.
    ///
    /// `pub`, not `pub(crate)`, and NOT split into two cfg'd bodies: a seam
    /// that parks a handle ([`registry::EngineRegistry::insert_for_test`]) is
    /// useless to a caller that cannot build one, and the
    /// `--features test-helpers --test '*'` integration tests are a different
    /// crate. Two bodies would be two answers to "how is a handle assembled",
    /// so the "only the registry launches" rule is held by a census instead of
    /// by visibility: `engine_handle_is_built_in_exactly_one_production_place`
    /// in [`registry`].
    #[must_use]
    pub fn new(
        engine: Engine,
        profile: String,
        launched: Launched,
        conn: aleph_cdp::CdpConnection,
        process: Arc<dyn EngineProcess>,
        first_tab: (String, aleph_cdp::SessionId),
    ) -> Self {
        let (tab_id, session) = first_tab;
        let mut entries = HashMap::new();
        entries.insert(tab_id.clone(), TabEntry::new(session));
        Self {
            engine,
            profile,
            launched,
            conn,
            tabs: tokio::sync::Mutex::new(TabTable {
                entries,
                active: Some(tab_id),
            }),
            pump_started: std::sync::atomic::AtomicBool::new(false),
            pump_lagged: std::sync::atomic::AtomicU64::new(0),
            process,
        }
    }

    /// A handle wired to an already-connected `CdpConnection`, with no process
    /// behind it. Test-only: the `cdp_backend` tests are about what goes on the
    /// wire, and a real launch would make every one of them need a browser.
    ///
    /// `#[cfg(test)]`, NOT `#[cfg(any(test, feature = "test-helpers"))]`: the
    /// body names [`crate::browser::testkit`], and that module is declared
    /// `#[cfg(test)]` at `src/browser/mod.rs` (R84). Under
    /// `cargo test --features test-helpers --test '*'` the library is built
    /// WITHOUT `cfg(test)` and WITH the feature, so the wider gate would
    /// compile this function against a module that does not exist (`E0433`) —
    /// and that command is in this task's own verification set.
    #[cfg(test)]
    #[must_use]
    pub fn for_test(engine: Engine, profile: &str, conn: aleph_cdp::CdpConnection) -> Self {
        Self {
            engine,
            profile: profile.to_string(),
            launched: Launched {
                pid: 0,
                endpoint: self::process::CdpEndpoint {
                    http_url: String::new(),
                    ws_url: String::new(),
                    pid: 0,
                },
                sidecar_path: std::path::PathBuf::new(),
                // Ruling R7: the launcher keeps the `Child` so it can reap it
                // after a kill. There is no process behind a test handle, so
                // the slot is empty — and empty here means "no child", which is
                // the truth, not "we lost it".
                child: std::sync::Arc::new(crate::sync_primitives::Mutex::new(None)),
            },
            conn,
            tabs: tokio::sync::Mutex::new(TabTable::default()),
            pump_started: std::sync::atomic::AtomicBool::new(false),
            pump_lagged: std::sync::atomic::AtomicU64::new(0),
            // `detached`, not `new`: `FakeEngineProcess::new(engine,
            // &FakeCdpServer, &Path)` exists to hand a launcher a server to
            // point a `Launched` at and a directory to write a sidecar into.
            // A handle with no process behind it has neither, and inventing a
            // temp dir here would make every `cdp_backend` test create one it
            // never uses.
            process: Arc::new(crate::browser::testkit::FakeEngineProcess::detached(engine)),
        }
    }

    /// Whether the engine is still reachable.
    ///
    /// Derived from the connection's disconnect watch — the owner of "the
    /// engine died" per spec §5.2 — and not from a second `try_wait` on the
    /// pid, which would be a second answer free to disagree with the first.
    #[must_use]
    pub fn alive(&self) -> bool {
        self.conn.closed().borrow().is_none()
    }

    /// The session attached to `tab_id`. **A lookup, never an attach.**
    ///
    /// An unknown tab is `TabNotFound`: acting on a tab that does not exist is
    /// the fail-closed direction, and inventing one here would make a typo look
    /// like a working page. Every verb that operates on a tab which must
    /// already exist calls this — `navigate`, `snapshot`, `evaluate`,
    /// `screenshot`, the dialog and cookie paths — and only the verbs that
    /// legitimately bring a tab into existence call [`Self::attach_tab`].
    pub async fn ensure_tab(&self, tab_id: &str) -> Result<aleph_cdp::SessionId, BrowserError> {
        let tabs = self.tabs.lock().await;
        tabs.entries
            .get(tab_id)
            .map(|e| e.session.clone())
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))
    }

    /// Attach to `target`, enable the three domains this driver listens on, and
    /// put the tab in the table. Returns its session.
    ///
    /// The other half of [`Self::ensure_tab`], split because the two are
    /// opposite fail directions and one name cannot be both: this one CREATES
    /// the entry, so only the verbs that bring a tab into existence may use it
    /// — `open_tab` (after `Target.createTarget`), `switch_tab` (adopting a
    /// target the browser has but Aleph has not seen) and popup adoption.
    ///
    /// **Idempotent**, and that is not a convenience: attaching twice gives one
    /// page two sessions and two `Page.enable` subscriptions, and the second
    /// session's events arrive somewhere nothing reads. An already-known target
    /// gets its existing session back, unchanged.
    ///
    /// The table guard is held across the attach so two concurrent adoptions of
    /// one popup cannot both miss the check and both attach.
    ///
    /// The three `enable`s are here rather than at the call sites because a tab
    /// whose domains were never enabled looks identical to a quiet one: no
    /// error, no events, and a console log that is simply always empty
    /// (判据 §2).
    /// **Bounded by ONE deadline across all four round trips**, the same shape
    /// `registry::bring_up` uses and for the same reason. Each of `attach`,
    /// `Page.enable`, `Runtime.enable` and `Network.enable` is separately
    /// bounded by the connection's `command_timeout` (30 s by default), so end
    /// to end the worst case was ~120 s — and every one of those seconds is
    /// spent holding the table guard, blocking every `ensure_tab` and every
    /// other `attach_tab` on this engine. Four limits that add up to a total
    /// nobody chose is the same defect as no limit at all (判据 §13), so the
    /// budget is the connection's own per-command patience spent ONCE for the
    /// whole sequence.
    pub async fn attach_tab(
        &self,
        target: &aleph_cdp::TargetId,
    ) -> Result<aleph_cdp::SessionId, BrowserError> {
        let mut tabs = self.tabs.lock().await;
        if let Some(existing) = tabs.entries.get(&target.0) {
            return Ok(existing.session.clone());
        }
        let deadline = tokio::time::Instant::now() + self.conn.command_timeout();
        let stalled = |step: &str| {
            BrowserError::AttachFailed(format!(
                "attaching to tab {} on {} stalled at {step} and did not finish within \
                 {}s. The tab is refused rather than half-wired; nothing was added to \
                 the tab table.",
                target.0,
                self.engine.as_str(),
                self.conn.command_timeout().as_secs_f64()
            ))
        };

        // No outer `timeout_at` on the attach: `conn.attach` already runs under
        // the connection's own `command_timeout`, which IS this deadline, so
        // wrapping it would only take the decision away from the arm that
        // cleans up. `CdpConnection::call_with_timeout` removes its `pending`
        // entry in its own `Err(_elapsed)` arm — an invariant that crate states
        // and tests — and an outer timeout DROPS that future, so the cleanup
        // never runs and the entry leaks into a connection that outlives the
        // call.
        let session = self.conn.attach(target).await.map_err(|e| {
            BrowserError::AttachFailed(format!(
                "could not attach to tab {} on {}: {e}",
                target.0,
                self.engine.as_str()
            ))
        })?;
        // A LOOP, not an array literal, and each call gets what is LEFT of the
        // budget rather than an outer wrapper.
        //
        // The array literal built all three futures before the loop body ran,
        // so once the deadline passed `Runtime.enable` and `Network.enable`
        // were still dispatched — constructed, inserted into `pending`, written
        // to the socket — and then instantly timed out. Measured:
        // `pending before=0 after=3`. One stalled attach leaked three entries
        // AND sent two more requests to a browser the caller had already given
        // up on, while holding this guard. Lazy evaluation is what stops the
        // dispatch; the per-call budget is what lets the crate's own cleanup
        // arm fire instead of ours.
        //
        // The method names are spelled here rather than reached through
        // `aleph_cdp::methods::{page,runtime,network}::enable`, because those
        // wrappers hardcode the connection's full `command_timeout` and cannot
        // be given a remainder. What keeps that second spelling honest is
        // `a_stalled_enable_and_a_refused_enable_are_told_apart`, which obtains
        // the names by CALLING those wrappers and compares. It is deliberately
        // not `attach_tab_creates_the_entry_that_ensure_tab_only_finds`, which
        // this comment used to name: that one asserts against a third copy, so
        // measured, it stays green when the owner drifts.
        for domain in ["Page", "Runtime", "Network"] {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(stalled(&format!("{domain}.enable")));
            }
            self.conn
                .call_with_timeout(
                    Some(&session),
                    &format!("{domain}.enable"),
                    serde_json::json!({}),
                    remaining,
                )
                .await
                // Two different facts, two different messages. A per-call
                // budget expiring says "this engine is not answering"; a CDP
                // error says "this engine refused". They collapsed into one
                // sentence when the per-call budget replaced the outer
                // wrapper, and the reader's next move differs: wait and retry
                // versus look at what the browser objected to (判据 §17).
                .map_err(|e| match e {
                    aleph_cdp::CdpError::Timeout { .. } => stalled(&format!("{domain}.enable")),
                    other => BrowserError::AttachFailed(format!(
                        "attached to tab {} but {domain} would not enable: {other}. Its \
                         events would be silently absent, so the tab is refused \
                         rather than half-wired.",
                        target.0
                    )),
                })?;
        }
        let mut entry = TabEntry::new(session.clone());
        // The connection's dropped-event count as this tab starts. Everything
        // before it happened to other tabs' rings, not this one's, so the
        // reader subtracts it rather than reporting a gap this tab never had.
        entry.lag_at_attach = self.pump_lagged.load(std::sync::atomic::Ordering::Relaxed);
        tabs.entries.insert(target.0.clone(), entry);
        if tabs.active.is_none() {
            tabs.active = Some(target.0.clone());
        }
        Ok(session)
    }

    /// Stop the engine and clear its record. `true` when it actually died.
    ///
    /// SIGKILL plus a bounded reap, and **never a graceful handshake** — the
    /// same rule `super::manager::ProfileManager::shutdown_browsers` states,
    /// and for the same reason: one caller is the wedged-shutdown watchdog, and
    /// a CDP `Browser.close` is a round trip to the process that may be why the
    /// shutdown wedged. `conn.close()` is local; it closes our socket and fails
    /// every pending call, it does not ask the peer for permission.
    pub async fn shutdown(&self) -> bool {
        self.conn.close().await;
        stop_launched(&*self.process, &self.launched, self.engine).await
    }
}

/// Kill a launched engine and clear its record **only if it is known dead**.
///
/// One derivation of "how an engine is stopped", shared by
/// [`EngineHandle::shutdown`] and by the launch path's own cleanup in
/// [`registry`]. Two copies is how one of them would go on deleting the record
/// of a process that refused to die (判据 §1) — which is the defect this
/// function was extracted to fix.
///
/// **The sidecar is removed only on `true`.** That record is the only thing
/// [`process::reap_orphans_now`] reads, and it is keyed on the session key — so
/// deleting it for a process that is still running does not tidy anything, it
/// makes that process unreapable for good and frees the slot for the next
/// launch of the same profile to overwrite. The `Ok(false)` arm's own message
/// says "leaving it for the next boot sweep"; the sweep has nothing to find
/// unless this holds. "I could not kill it" is not "it is gone" (判据 §8).
pub(crate) async fn stop_launched(
    process: &dyn EngineProcess,
    launched: &Launched,
    engine: Engine,
) -> bool {
    let pid = launched.pid;
    // The whole `Launched`, not the pid: it carries the `std::process::Child`
    // the launcher has to `wait()` on after signalling. A signal without a
    // reap leaves a zombie, and a pid on its own cannot be reaped by anyone.
    let died = match process.kill(launched, ENGINE_KILL_GRACE).await {
        Ok(true) => true,
        Ok(false) => {
            tracing::warn!(
                pid,
                engine = engine.as_str(),
                sidecar = %launched.sidecar_path.display(),
                "the engine did not exit within the kill grace window; leaving it \
                 AND its sidecar for the next boot sweep"
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                pid,
                engine = engine.as_str(),
                error = %e,
                sidecar = %launched.sidecar_path.display(),
                "could not kill the engine; leaving it AND its sidecar for the next \
                 boot sweep"
            );
            false
        }
    };
    if !died {
        return false;
    }
    match tokio::fs::remove_file(&launched.sidecar_path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            path = %launched.sidecar_path.display(),
            error = %e,
            "could not remove the engine sidecar; the boot sweep will see a \
             record for a process that is gone"
        ),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::Engine;

    /// The two engines are told apart by the argv switch that names their
    /// per-profile data directory, and the sweep kills on that answer. A
    /// single shared switch would let an obscura record authorise a SIGKILL
    /// against a Chromium (and the reverse) purely because both processes
    /// happen to sit under the same profile root.
    #[test]
    fn each_engine_names_its_data_directory_with_its_own_switch() {
        assert_eq!(Engine::Chromium.data_dir_flag(), "--user-data-dir");
        assert_eq!(Engine::Obscura.data_dir_flag(), "--storage-dir");
        assert_ne!(
            Engine::Chromium.data_dir_flag(),
            Engine::Obscura.data_dir_flag()
        );
    }

    /// `as_str` / `parse` are inverses, and `parse` refuses everything else.
    /// The wire spelling is a config value (`engine = "obscura"`) and a
    /// `runtime_manage{capability}` value, so a silent acceptance of a
    /// near-miss would pick an engine the operator did not name.
    #[test]
    fn engine_parses_exactly_its_own_wire_spelling() {
        for e in Engine::ALL {
            assert_eq!(
                Engine::parse(e.as_str()),
                Some(e),
                "{} did not round-trip",
                e.as_str()
            );
            assert_eq!(e.data_subdir(), e.as_str());
        }
        for bad in [
            "",
            "Chromium",
            "OBSCURA",
            "chrome",
            "obscura ",
            "chromium\n",
        ] {
            assert_eq!(Engine::parse(bad), None, "accepted {bad:?}");
        }
    }

    /// The human spelling and the wire spelling are one string, not two.
    ///
    /// `Display` is what error texts interpolate (Task 9's `EngineMismatch`
    /// names two engines; Task 8's `UnsupportedByEngine` names two more), and
    /// `as_str` is what serde, the config file and `runtime_manage`'s
    /// `capability` value use. A hand-written `Display` that said "Obscura" or
    /// "the obscura engine" would put a spelling in front of the operator that
    /// their config file rejects (判据 §1).
    #[test]
    fn display_matches_as_str_for_every_engine() {
        for e in Engine::ALL {
            assert_eq!(
                e.to_string(),
                e.as_str(),
                "{e:?} formats differently than it parses"
            );
            // And the formatted text round-trips back through `parse`, which
            // is the property an error message the reader retypes depends on.
            assert_eq!(Engine::parse(&e.to_string()), Some(e));
        }
    }

    /// The product default is obscura (spec §6.3), and it must NOT be what
    /// an old sidecar record falls back to — those were all Chromium. Two
    /// different defaults, two different names, so neither can be reached
    /// by accident.
    #[test]
    fn the_product_default_is_obscura_and_the_sidecar_compat_default_is_chromium() {
        assert_eq!(Engine::default(), Engine::Obscura);
        assert_eq!(Engine::chromium_default(), Engine::Chromium);
    }
}
