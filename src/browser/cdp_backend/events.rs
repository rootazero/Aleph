//! The one event pump per engine connection, and the two verbs that read what
//! it wrote.
//!
//! Three consumers read the pump's output — `console_messages`, `network_log`
//! and `dialog::handle_dialog` — and all three read it out of [`TabEntry`], so
//! the mapping from a CDP event to a recorded fact exists exactly once, in
//! [`apply_event`]. The click path calls that same function directly when it
//! races a dialog (`actions::press_and_release`), so the pump and the racer
//! cannot disagree about what a dialog event means.
//!
//! # Why an empty ring is not an answer on its own
//!
//! `TabEntry::console` and `TabEntry::network` start empty and stay empty when
//! nothing was logged — and also when nothing was *listening*. Those are
//! different facts with the same bytes, and reporting the second as the first
//! is a fail-closed answer consumed as an observation (判据 §8). The pump is
//! what makes the difference, so [`render_ring`] takes the pump's flag and
//! picks a different sentence for each. `EngineHandle::pump_started` exists for
//! that reader; the double-spawn gate is the smaller half of its job.
//!
//! **The same collapse has a third level, and `EngineHandle::pump_lagged`
//! closes it.** `aleph_cdp::EventStream::next` steps over a dropped event and
//! counts it, so a ring can be *incomplete* while the pump was running the
//! whole time — and "here are the lines" would then be a confident wrong
//! answer. [`render_ring`] therefore has four answers, not two, and the fourth
//! borrows `navigate::lag_note`: the count and its framing have one home, and
//! only the consequence clause differs between a navigation barrier and a log.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use aleph_cdp::{CdpEvent, SessionId};

use crate::browser::engine::{EngineHandle, TabEntry, TabTable};
use crate::browser::error::BrowserError;

use super::CdpBackend;

/// How many console lines and network lines a tab keeps.
///
/// Bounded because a page in a `console.log` loop would otherwise grow this
/// without limit for the lifetime of the browser; the tail is what the model's
/// last action produced, so the OLDEST entries are the ones that go.
pub(super) const RING_CAP: usize = 500;

fn push_bounded(ring: &mut VecDeque<String>, line: String) {
    ring.push_back(line);
    while ring.len() > RING_CAP {
        ring.pop_front();
    }
}

fn tab_for_session<'a>(
    tabs: &'a mut TabTable,
    session: Option<&SessionId>,
) -> Option<&'a mut TabEntry> {
    let s = session?;
    tabs.entries.values_mut().find(|e| &e.session == s)
}

/// Render one `Runtime.consoleAPICalled` argument.
///
/// CDP gives a `value` for serialisable arguments and a `description` for
/// objects; an argument with neither is rendered as its type rather than
/// dropped, because a blank inside a console line reads like the page printed
/// a blank.
fn render_arg(arg: &serde_json::Value) -> String {
    if let Some(v) = arg.get("value") {
        return match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    if let Some(d) = arg.get("description").and_then(|d| d.as_str()) {
        return d.to_string();
    }
    if let Some(u) = arg.get("unserializableValue").and_then(|d| d.as_str()) {
        return u.to_string();
    }
    format!(
        "<{}>",
        arg.get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("unknown")
    )
}

/// Fold one CDP event into the tab table.
///
/// Returns whether it changed anything — which is what the tests assert on, and
/// what makes "the event was for a session we do not know" observable rather
/// than silent.
pub(super) fn apply_event(tabs: &mut TabTable, ev: &CdpEvent) -> bool {
    let Some(entry) = tab_for_session(tabs, ev.session.as_ref()) else {
        return false;
    };
    match ev.method.as_str() {
        "Runtime.consoleAPICalled" => {
            let level = ev.params["type"].as_str().unwrap_or("log");
            let args = ev.params["args"].as_array().cloned().unwrap_or_default();
            let text = args.iter().map(render_arg).collect::<Vec<_>>().join(" ");
            push_bounded(&mut entry.console, format!("[{level}] {text}"));
            true
        }
        "Runtime.exceptionThrown" => {
            let d = &ev.params["exceptionDetails"];
            let text = d["exception"]["description"]
                .as_str()
                .or_else(|| d["text"].as_str())
                .unwrap_or("uncaught exception");
            push_bounded(&mut entry.console, format!("[error] {text}"));
            true
        }
        "Network.requestWillBeSent" => {
            let method = ev.params["request"]["method"].as_str().unwrap_or("GET");
            let url = ev.params["request"]["url"].as_str().unwrap_or("");
            push_bounded(&mut entry.network, format!("-> {method} {url}"));
            true
        }
        "Network.responseReceived" => {
            let status = ev.params["response"]["status"].as_i64().unwrap_or(0);
            let url = ev.params["response"]["url"].as_str().unwrap_or("");
            push_bounded(&mut entry.network, format!("<- {status} {url}"));
            true
        }
        "Page.javascriptDialogOpening" => {
            let kind = ev.params["type"].as_str().unwrap_or("dialog");
            let message = ev.params["message"].as_str().unwrap_or("");
            entry.pending_dialog = Some(format!("{kind}: {message}"));
            true
        }
        "Page.javascriptDialogClosed" => {
            entry.pending_dialog = None;
            true
        }
        _ => false,
    }
}

/// Claim this handle's pump slot. `true` for exactly one caller, ever.
///
/// Extracted from [`ensure_pump`] so the gate is testable **without racing a
/// spawned task**: the whole of "only one pump per handle" is this
/// compare-exchange, and a test that drove it through `tokio::spawn` would be
/// measuring the scheduler instead. A `swap(true)` — the obvious-looking
/// simplification — returns the OLD value and reads `false` on the first call
/// too, which is why this is a named function with its own falsifier rather
/// than an inline expression.
#[must_use]
fn claim_pump(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Start this handle's pump if it is not already running.
///
/// Idempotent by [`claim_pump`] rather than by a "did we?" read: two verbs
/// resolving the same handle concurrently is the normal case, and two pumps
/// would record every console line twice.
pub(super) fn ensure_pump(handle: &Arc<EngineHandle>) {
    if !claim_pump(&handle.pump_started) {
        return;
    }
    // Subscribed HERE, synchronously, before the spawn — not inside the task.
    // `pump_started` has just become true, and a verb that reads it a
    // microsecond later is entitled to read it as "every event from now on is
    // being recorded". Taking the subscription inside the spawned task would
    // make that claim false for however long the scheduler took to poll it,
    // and the events lost in that window are exactly the ones a verb's own
    // action produced.
    let mut events = handle.conn.events();
    // A `Weak`, deliberately: an `Arc` here would keep the handle — and so the
    // websocket — alive forever, and the pump would outlive the browser it was
    // reading. The upgrade failing IS the exit condition.
    let weak: Weak<EngineHandle> = Arc::downgrade(handle);
    tokio::spawn(async move {
        // Ask for target lifecycle events. This is the ONLY reason a tab we did
        // not create ourselves can enter the table, and it is an event rather
        // than a poll because `Target.getTargets` is not a usable source — real
        // obscura answers it with an empty list on a fresh connection.
        if let Some(handle) = weak.upgrade() {
            if let Err(e) =
                aleph_cdp::methods::target::set_discover_targets(&handle.conn, true).await
            {
                // Not fatal: without discovery, popups are simply not adopted,
                // and the tabs we created ourselves are unaffected. Logged
                // because a silently missing subscription and a browser that
                // opens no popups look identical.
                tracing::warn!(
                    error = %e,
                    "browser: target discovery not enabled; popups this page \
                     opens will not be addressable"
                );
            }
        }
        while let Some(ev) = events.next().await {
            let Some(handle) = weak.upgrade() else { break };
            // Published every iteration, before the event is folded in.
            // `EventStream::next` STEPS OVER a lag and counts it — `None` means
            // only that the connection closed — so without this read the rings
            // would carry a silent hole and `render_ring` would report them as
            // a complete observation. That is `pump_started`'s own 判据 §8
            // collapse one level down, and the corrections file named this
            // stream's second consumer as the place it would appear.
            handle.pump_lagged.store(events.lagged(), Ordering::Relaxed);
            // Popup adoption needs an attach, which is async, so it cannot live
            // inside `apply_event`'s `&mut TabTable`.
            if ev.method == "Target.targetCreated" {
                adopt_popup(&handle, &ev).await;
                continue;
            }
            let mut tabs = handle.tabs.lock().await;
            apply_event(&mut tabs, &ev);
        }
    });
}

/// Adopt a page the page itself opened (`target=_blank`, `window.open`).
///
/// Gated on `openerId` naming a tab we already own: a browser-wide discovery
/// stream also carries targets belonging to other profiles and to the browser's
/// own machinery, and adopting those would put tabs in this profile's table
/// that this profile never opened — the enumeration problem in a different
/// costume.
async fn adopt_popup(handle: &Arc<EngineHandle>, ev: &CdpEvent) {
    let info = &ev.params["targetInfo"];
    if info["type"].as_str() != Some("page") {
        return;
    }
    let (Some(target_id), Some(opener)) = (info["targetId"].as_str(), info["openerId"].as_str())
    else {
        return;
    };
    {
        let tabs = handle.tabs.lock().await;
        if !tabs.entries.contains_key(opener) || tabs.entries.contains_key(target_id) {
            return;
        }
    }
    if let Err(e) = handle
        .attach_tab(&aleph_cdp::TargetId(target_id.to_string()))
        .await
    {
        tracing::warn!(
            target = %target_id, error = %e,
            "browser: a popup opened by a tracked tab could not be attached; it \
             is not addressable"
        );
        return;
    }
    if let Some(url) = info["url"].as_str() {
        let mut tabs = handle.tabs.lock().await;
        if let Some(entry) = tabs.entries.get_mut(target_id) {
            entry.url = url.to_string();
        }
    }
}

/// Which ring a rendering is about. An enum rather than a `&str` at each call
/// site so the two sentences below are written once each.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Ring {
    Console,
    Network,
}

impl Ring {
    const fn what(self) -> &'static str {
        match self {
            Self::Console => "console messages",
            Self::Network => "network activity",
        }
    }
}

/// Turn one tab's ring into the text the model reads.
///
/// **`watching` is an ARGUMENT, not a field read in here**, for the reason
/// `lag_note` takes a `u64` rather than the `&EventStream` it came from
/// (`navigate.rs:41-45`): a branch whose input is a parameter can be driven to
/// both outcomes by a test, and one that reads the flag inside cannot.
///
/// **Four** answers, not two, and `lagged` is the fourth:
/// * **not watching** — there is no pump, so an empty ring is not evidence of
///   anything. Saying "no console messages" here would be a fail-closed answer
///   consumed as a fact (判据 §8), and a model that read it would conclude the
///   page is quiet and stop looking.
/// * **watching, nothing recorded, nothing dropped** — a real observation, said
///   in words because an empty string reads to the model as a call that failed
///   to produce output.
/// * **watching, lines recorded** — the lines.
/// * **anything dropped** — the same answer as above with [`lag_note`]
///   appended, because a ring with a hole in it is not a complete record and
///   "empty" / "these lines" would both be a confident wrong answer. The
///   surrounding design already taught the model that elision announces itself:
///   `redact_and_wrap_log` appends its own note when IT drops lines.
pub(super) fn render_ring(
    watching: bool,
    lagged: u64,
    ring: &VecDeque<String>,
    kind: Ring,
) -> String {
    let what = kind.what();
    if !watching {
        return format!(
            "no event pump is running on this engine, so Aleph has recorded no \
             {what} for this tab — that is NOT evidence the page produced none. \
             Reopen the profile with `browser_open` and repeat the action you \
             wanted the log for."
        );
    }
    // ONE derivation of how a dropped-event count is phrased, shared with the
    // navigation barrier (`navigate::lag_note`); only the consequence clause
    // differs. Connecting to the existing reader rather than writing a second
    // one is the whole point — the count and its framing have one home.
    let note = super::navigate::lag_note(lagged, super::navigate::LOG_HAS_A_HOLE);
    if ring.is_empty() {
        return format!("no {what} recorded for this tab{note}");
    }
    format!(
        "{}{note}",
        ring.iter().cloned().collect::<Vec<_>>().join("\n")
    )
}

/// One tab's ring, rendered. Both verbs are the same three steps, so they share
/// them rather than each spelling out the lock, the lookup and the flag read.
async fn read_ring(
    be: &CdpBackend,
    tab_id: &str,
    kind: Ring,
    pick: fn(&TabEntry) -> &VecDeque<String>,
) -> Result<String, BrowserError> {
    let handle = be.handle().await?;
    handle.ensure_tab(tab_id).await?;
    let watching = handle.pump_started.load(Ordering::SeqCst);
    let lagged = handle.pump_lagged.load(Ordering::Relaxed);
    let tabs = handle.tabs.lock().await;
    let entry = tabs
        .entries
        .get(tab_id)
        .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))?;
    Ok(render_ring(watching, lagged, pick(entry), kind))
}

pub(super) async fn console_messages(
    be: &CdpBackend,
    tab_id: &str,
) -> Result<String, BrowserError> {
    read_ring(be, tab_id, Ring::Console, |e| &e.console).await
}

pub(super) async fn network_log(be: &CdpBackend, tab_id: &str) -> Result<String, BrowserError> {
    read_ring(be, tab_id, Ring::Network, |e| &e.network).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_cdp::testkit::FakeCdpServer;
    use aleph_cdp::SessionId;
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;

    fn table_with_one_tab() -> (TabTable, SessionId) {
        let session = SessionId("S1".into());
        let mut tabs = TabTable::default();
        tabs.entries
            .insert("T1".into(), TabEntry::new(session.clone()));
        (tabs, session)
    }

    fn ev(session: &SessionId, method: &str, params: serde_json::Value) -> CdpEvent {
        CdpEvent {
            session: Some(session.clone()),
            method: method.into(),
            params,
        }
    }

    /// The ring is bounded, and the bound drops the OLDEST lines: the newest are
    /// the ones the model's last action produced.
    #[test]
    fn the_console_ring_keeps_the_newest_entries_not_the_first_ones() {
        let (mut tabs, session) = table_with_one_tab();
        for i in 0..(RING_CAP + 100) {
            apply_event(
                &mut tabs,
                &ev(
                    &session,
                    "Runtime.consoleAPICalled",
                    json!({"type": "log",
                           "args": [{"type": "string", "value": format!("line{i}")}]}),
                ),
            );
        }
        let ring = &tabs.entries["T1"].console;
        assert_eq!(ring.len(), RING_CAP, "the ring must be bounded");
        assert!(
            ring.back()
                .expect("a bounded ring of 500 is not empty")
                .contains(&format!("line{}", RING_CAP + 99)),
            "the newest line must survive: {:?}",
            ring.back()
        );
        // Exact, not a `contains`: `"line0"` is not a substring of `"line100"`,
        // so a `!contains("line0")` assertion would pass for a ring that had
        // dropped the wrong hundred — or none at all, if the cap were raised.
        // The surviving window is 100..=RING_CAP+99, so its first line is
        // named outright.
        assert_eq!(
            ring.front().map(String::as_str),
            Some("[log] line100"),
            "the oldest 100 lines must be the ones that went"
        );
    }

    /// An event for a session this table does not know must change nothing — not
    /// be attributed to whichever tab happens to be first.
    ///
    /// The table is built with TWO tabs on purpose. With one, `values_mut()`
    /// has a single element and "find the matching session" and "take the
    /// first" are the same answer, so the mutation this test exists to catch
    /// would stay green — a fixture built to be legible is built to be tidy,
    /// and tidy inputs cannot collide. The precondition below is asserted so
    /// that premise cannot rot.
    #[test]
    fn an_event_for_an_unknown_session_is_dropped_not_misattributed() {
        let (mut tabs, session) = table_with_one_tab();
        tabs.entries
            .insert("T2".into(), TabEntry::new(SessionId("S2".into())));
        assert!(
            tabs.entries.len() > 1,
            "precondition: with one tab, 'the matching session' and 'the first \
             entry' are the same element and this test cannot fail"
        );

        let other = SessionId("S-OTHER".into());
        let changed = apply_event(
            &mut tabs,
            &ev(
                &other,
                "Runtime.consoleAPICalled",
                json!({"type": "log", "args": [{"type": "string", "value": "not ours"}]}),
            ),
        );
        assert!(!changed, "an unknown session changes nothing");
        for (id, entry) in &tabs.entries {
            assert!(
                entry.console.is_empty(),
                "tab {id} was given another session's line: {:?}",
                entry.console
            );
        }

        // …and the known session still lands, so the drop above is a lookup
        // failing rather than the whole function being inert (判据 §2 恒红).
        assert!(apply_event(
            &mut tabs,
            &ev(
                &session,
                "Runtime.consoleAPICalled",
                json!({"type": "log", "args": [{"type": "string", "value": "ours"}]}),
            ),
        ));
        assert_eq!(tabs.entries["T1"].console.len(), 1);
        assert!(tabs.entries["T2"].console.is_empty());
    }

    /// The dialog latch has both edges. Only the opening one is exercised by the
    /// click path, so the closing one is the half that would rot silently.
    #[test]
    fn the_dialog_latch_sets_and_clears() {
        let (mut tabs, session) = table_with_one_tab();
        apply_event(
            &mut tabs,
            &ev(
                &session,
                "Page.javascriptDialogOpening",
                json!({"type": "alert", "message": "are you sure"}),
            ),
        );
        assert_eq!(
            tabs.entries["T1"].pending_dialog.as_deref(),
            Some("alert: are you sure")
        );
        apply_event(
            &mut tabs,
            &ev(&session, "Page.javascriptDialogClosed", json!({})),
        );
        assert!(tabs.entries["T1"].pending_dialog.is_none());
    }

    /// The gate that makes the pump singular, tested WITHOUT a spawned task:
    /// the whole of "only one pump per handle" is this compare-exchange, and
    /// driving it through `tokio::spawn` would measure the scheduler instead.
    #[test]
    fn the_pump_slot_is_claimed_exactly_once() {
        let flag = AtomicBool::new(false);
        assert!(claim_pump(&flag), "the first caller owns the pump");
        assert!(flag.load(Ordering::SeqCst), "and the claim is recorded");
        assert!(
            !claim_pump(&flag),
            "a second caller must not spawn a second pump — two pumps record \
             every console line twice"
        );
        assert!(!claim_pump(&flag), "and it stays claimed");
    }

    /// **An empty ring with no pump is not an observation.** This is the reader
    /// `EngineHandle::pump_started` exists for: `TabEntry::console` is empty
    /// both when the page logged nothing and when nobody was listening, and
    /// only the first of those is a fact (判据 §8).
    ///
    /// Driven by the ARGUMENT, so both branches are reachable regardless of
    /// what the production wiring does — the same seam `require(caps, …)` uses
    /// one file over (R42).
    #[test]
    fn an_empty_ring_says_whether_anyone_was_listening() {
        let empty = VecDeque::new();

        let unwatched = render_ring(false, 0, &empty, Ring::Console);
        assert!(
            unwatched.contains("NOT evidence"),
            "with no pump the answer must refuse to be read as an observation: \
             {unwatched}"
        );
        assert!(
            unwatched.contains("browser_open"),
            "a fail-closed answer names the door that opens it (判据 §14): \
             {unwatched}"
        );

        let watched = render_ring(true, 0, &empty, Ring::Console);
        assert!(
            watched.contains("no console messages recorded"),
            "with a pump, an empty ring IS an observation: {watched}"
        );
        assert_ne!(
            watched, unwatched,
            "the two must not share a spelling, which is the whole defect"
        );

        // The network ring says network things, so a reader cannot be told the
        // console was quiet when it was the network that was not watched.
        assert!(render_ring(false, 0, &empty, Ring::Network).contains("network activity"));
        assert!(
            render_ring(true, 0, &empty, Ring::Network).contains("no network activity recorded")
        );

        // And a non-empty ring is the lines themselves — the recorded lines are
        // facts whoever was listening.
        let mut two = VecDeque::new();
        two.push_back("[log] a".to_string());
        two.push_back("[log] b".to_string());
        assert_eq!(
            render_ring(true, 0, &two, Ring::Console),
            "[log] a\n[log] b"
        );
    }

    /// The fourth answer: a ring the pump dropped events into is not a complete
    /// record, and saying "here are the lines" (or "nothing was logged") is the
    /// same 判据 §8 collapse `pump_started` closed, one level down.
    ///
    /// Driven by the ARGUMENT, like `watching` and for the same reason: a lag
    /// needs more than 64 queued events and a consumer holding the table lock
    /// to produce, which is a runtime race, not something a unit test should be
    /// asked to win.
    #[test]
    fn a_ring_the_pump_dropped_events_into_says_so() {
        let mut two = VecDeque::new();
        two.push_back("[log] a".to_string());
        two.push_back("[log] b".to_string());

        let clean = render_ring(true, 0, &two, Ring::Console);
        let holed = render_ring(true, 9, &two, Ring::Console);
        assert_eq!(clean, "[log] a\n[log] b", "no lag, no words");
        assert!(
            holed.starts_with(&clean),
            "the lines themselves must survive: {holed}"
        );
        assert!(
            holed.contains('9') && holed.contains("gap"),
            "the count and what it means are the fact: {holed}"
        );

        // An EMPTY ring with a lag is the worst of the four: it reads as "the
        // page did nothing" while the pump was losing events.
        let empty = VecDeque::new();
        let empty_holed = render_ring(true, 4, &empty, Ring::Network);
        assert!(
            empty_holed.contains("no network activity recorded"),
            "{empty_holed}"
        );
        assert!(
            empty_holed.contains('4') && empty_holed.contains("gap"),
            "an empty ring with dropped events must not read as an observation: \
             {empty_holed}"
        );

        // No pump at all still wins: "nobody was listening" is the stronger
        // statement, and appending a drop count to it would suggest someone was.
        let none = render_ring(false, 9, &empty, Ring::Console);
        assert!(none.contains("NOT evidence"), "{none}");
        assert!(
            !none.contains("gap"),
            "with no pump there is no partial record to qualify: {none}"
        );
    }

    /// The production half of the claim above: going through the public verb,
    /// the answer is the OBSERVATION wording — because `CdpBackend::handle`
    /// starts the pump before any verb can read a ring.
    ///
    /// This is what makes `pump_started` a live wire rather than a field with a
    /// clever doc: delete `events::ensure_pump(&handle)` from `handle()` and
    /// this test goes red with the "no event pump is running" sentence.
    #[tokio::test]
    async fn a_ring_read_through_the_verb_reports_an_observation() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("pre-seeded handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach ok");

        assert!(
            handle.pump_started.load(Ordering::SeqCst),
            "resolving a handle must start the pump, or every ring read below \
             is answering about a tab nothing was watching"
        );
        for out in [
            backend.console_messages("T1").await.expect("console ok"),
            backend.network_log("T1").await.expect("network ok"),
        ] {
            assert!(
                !out.contains("no event pump is running"),
                "the verb reported that nothing was listening: {out}"
            );
            assert!(
                out.contains("recorded for this tab"),
                "an empty-but-watched ring is an observation: {out}"
            );
        }
    }

    /// The discovery stream is browser-wide. Adopting a target whose opener is
    /// not one of ours would put another profile's page — or a piece of the
    /// browser's own machinery — into this profile's table, which is the
    /// enumeration problem in a different costume.
    #[tokio::test]
    async fn a_popup_is_adopted_only_when_its_opener_is_a_tab_we_own() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("OWNED".into()))
            .await
            .expect("attach");

        let created = |target: &str, opener: &str| CdpEvent {
            session: None,
            method: "Target.targetCreated".into(),
            params: json!({ "targetInfo": {
                "targetId": target, "type": "page", "openerId": opener,
                "url": "https://popup.example/", "attached": false
            }}),
        };

        adopt_popup(&handle, &created("STRANGER", "NOT-OURS")).await;
        assert!(
            !handle.tabs.lock().await.entries.contains_key("STRANGER"),
            "a target opened by someone else's tab must not join this profile"
        );

        adopt_popup(&handle, &created("POPUP", "OWNED")).await;
        let tabs = handle.tabs.lock().await;
        assert!(
            tabs.entries.contains_key("POPUP"),
            "a popup our own tab opened must become addressable"
        );
        assert_eq!(tabs.entries["POPUP"].url, "https://popup.example/");
    }
}
