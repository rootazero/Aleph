//! What each engine can actually do, measured rather than assumed.
//!
//! This is a **name list**, and a name list only covers the world as it was on
//! the day it was written (判据 §5). Two things keep it honest: every row
//! carries the version it was measured on, and Task 16's real-machine prober
//! checks the claims against the running binary — so a claim that stops being
//! true goes red on a real machine rather than living on as prose.
//!
//! Every obscura value below cites the file:line in obscura `72c84ad` that
//! decided it; the survey is
//! `docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/obscura-source-survey.md` §3.

use super::Engine;
use crate::runtimes::OBSCURA_TAG;

/// Task 0's probe results, verbatim, as the only copy in the repository.
///
/// `include_str!`, not a runtime read: a missing matrix is a COMPILE error
/// naming the path, rather than a test that quietly finds nothing and certifies
/// its subject by looking nowhere (判据 §3).
///
/// `pub(crate)` and hoisted out of the test module because it now has a SECOND
/// reader — `builtin_tools::browser_tools::emulate`'s DESCRIPTION guard, which
/// asks the same fixture a different question (which emulation methods this
/// engine refuses). One path literal, two readers: the alternative is two
/// `include_str!` paths to one file, which is 判据 §1 with a compile error as
/// its only saving grace.
#[cfg(test)]
pub(crate) const T0_SUPPORT_MATRIX: &str = include_str!("fixtures/t0-support-matrix.json");

/// How Task 0 keyed each engine's half. Its Chromium capture is under
/// `"chrome"`, not `"chromium"` — mapped in the readers rather than by
/// rewriting the fixture, because rewriting somebody else's measurement to
/// match our vocabulary is how a fixture stops being evidence.
/// The one arm that diverges is spelled out; every other engine keeps
/// `as_str`, so a third engine does not need a second name here.
#[cfg(test)]
pub(crate) const fn t0_key(engine: Engine) -> &'static str {
    match engine {
        Engine::Chromium => "chrome",
        other => other.as_str(),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cap {
    Supported,
    /// Routed and real, but with a behavioural caveat the caller has to plan
    /// around. Reserved; no row uses it today, and a row that becomes
    /// `Partial` must say why in a comment beside it.
    Partial,
    Unsupported,
}

impl Cap {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
        }
    }
}

/// **Every row names the `browser_*` verb that dispatches it** (R39, 判据 §9).
/// This table is published to the model through
/// `browser_session{action:"capabilities"}`, so a row with no verb behind it
/// tells the model Aleph can do something no tool can reach — a capability
/// with no client is not delivered. Two rows were cut on exactly that test:
/// `touch` (no `browser_tap`; nothing dispatches `Input.dispatchTouchEvent`)
/// and `multi_connection` (an architectural property, not a verb — it
/// belongs in the live-view design notes, not in a table the model reads as a
/// menu). Two more were cut under the same rule and later RESTORED when a
/// verb arrived to dispatch them: `network_interception` (restored with
/// `browser_network`'s mock actions,
/// docs/superpowers/plans/2026-09-24-browser-network-mock.md) and
/// `screencast` (restored with `browser_record`,
/// docs/superpowers/plans/2026-09-27-browser-recording.md).
pub struct EngineCapabilities {
    /// `browser_dialog` — `Page.handleJavaScriptDialog`, plus the
    /// `Page.javascriptDialogOpening` event the backend tracks to know a
    /// dialog is open at all.
    pub js_dialogs: Cap,
    /// `browser_drag`.
    ///
    /// **The one row whose two engines disagree, so it states its rule here
    /// rather than twice (判据 §9 — one verb, several faces, ONE derivation).**
    /// The rule: this row is `Supported` only where an **effect** reading
    /// exists for the method `browser_drag` actually sends
    /// (`Input.dispatchMouseEvent`) on that engine. A protocol `ok` does not
    /// qualify — obscura answers `ok` to `DOM.setAttributeValue` and
    /// `DOM.removeNode` with `effect: false` (T0), so on this engine "ok" is a
    /// measured report-success-no-op candidate, not evidence.
    ///
    /// Applied: Chromium has such a reading and obscura does not. Both arms
    /// carry the citation beside their value, and
    /// `the_disagreeing_row_cites_the_effect_reading_that_separates_the_engines`
    /// `every_matrix_exempt_disagreement_cites_its_reading` keeps the Chromium
    /// citation from becoming a dead reference.
    pub drag: Cap,
    /// `browser_upload` — `DOM.setFileInputFiles`. (The other upload route,
    /// the file-chooser interception, is a NOOP on obscura and no verb uses
    /// it.)
    ///
    /// obscura gates this method behind `--allow-file-access`, which Aleph
    /// never passes; see the `OBSCURA` row below.
    pub file_upload: Cap,
    /// `browser_pdf` — `Page.printToPDF`.
    pub pdf: Cap,
    /// `browser_type` and `browser_fill` — `Input.insertText`.
    pub insert_text: Cap,
    /// `browser_click`, `browser_type` and `browser_fill_form` — the
    /// effect-arrival probe: a one-shot capture listener installed on the
    /// resolved node via `Runtime.callFunctionOn` BEFORE dispatch, read back
    /// via `Runtime.evaluate` after. It needs the engine's page-global JS
    /// state to persist across those two calls, and its synthetic
    /// `Input.dispatch*` events to reach page-installed listeners.
    pub effect_probe: Cap,
    /// `browser_click`, `browser_type`, `browser_fill_form`, `browser_select`
    /// and `browser_hover` — the pre-dispatch ref staleness check
    /// (`browser_tools::precheck_ref`): a pure lookup against the cdp
    /// backend's per-tab `RefTable` plus the tab registry's URL-drift signal.
    ///
    /// Engine-agnostic by construction: the check reads Aleph's own tab state
    /// and sends NO round-trip to the engine, so there is nothing per-engine
    /// to measure and both engines are `Supported`. The honest asymmetry is
    /// one level up, at the DRIVER: the two legacy drivers own no `RefTable`,
    /// so the check skips them (a skip, not a pass) — and this engine-keyed
    /// table cannot say so, which is why the doc does.
    pub ref_precheck: Cap,
    /// `browser_network`'s four mock actions — `mock_add`, `mock_list`,
    /// `mock_remove`, `mock_clear` — served by the Fetch-domain handshake
    /// (`Fetch.enable` plus the per-tab `requestPaused` loop) that only the
    /// cdp backend drives.
    pub network_interception: Cap,
    /// `browser_record`'s three actions — `start`, `stop`, `status` — served
    /// by the `Page.startScreencast` frame stream (plus `screencastFrameAck`
    /// pacing and `Page.stopScreencast`) that only the cdp backend drives;
    /// the frames feed an ffmpeg encoder, so the row covers the engine's
    /// half of the pipe, not the encoder's.
    pub screencast: Cap,
    /// `browser_qa`'s `check_errors` dimension — the engine's
    /// `Runtime.exceptionThrown` events, which the cdp backend's event pump
    /// folds into the console ring as `[error]` lines
    /// (`cdp_backend::events.rs`). The same pump already serves
    /// `Runtime.consoleAPICalled`, so the row is about whether THIS event
    /// arrives, not whether events do.
    pub error_events: Cap,
    /// The build each row above was measured against. A capability claim with
    /// no measurement date is a list that expires without telling anyone.
    pub measured_on: &'static str,
}

/// The field list every consumer walks: the prose, the JSON, and the QA
/// prober. A field added to the struct and forgotten here would silently stop
/// being described, serialised or falsified, so Task 16's
/// `cap_fields_covers_every_cap_typed_field_of_the_struct` derives the expected
/// set from this file's own source (判据 §3).
pub const CAP_FIELDS: [(&str, fn(&EngineCapabilities) -> Cap); 10] = [
    ("js_dialogs", |c| c.js_dialogs),
    ("drag", |c| c.drag),
    ("file_upload", |c| c.file_upload),
    ("pdf", |c| c.pdf),
    ("insert_text", |c| c.insert_text),
    ("effect_probe", |c| c.effect_probe),
    ("ref_precheck", |c| c.ref_precheck),
    ("network_interception", |c| c.network_interception),
    ("screencast", |c| c.screencast),
    ("error_events", |c| c.error_events),
];

static OBSCURA: EngineCapabilities = EngineCapabilities {
    // Two witnesses, and only one of them witnessed — stated because the row
    // is published to the model under the words "Measured on <tag>" (判据 §18).
    //
    //  1. The source survey (§3): neither `Page.javascriptDialogOpening` nor
    //     `Page.handleJavaScriptDialog` exists anywhere in `obscura-cdp`, so a
    //     page that opens an alert simply never reports one. **This is what
    //     the value rests on.**
    //  2. T0's probe: `{"protocol": "no event within 4s (wait expired, not a
    //     measured absence)", "effect": null}` — the fixture goes out of its
    //     way to say it is NOT a measured absence, and an unknown may only say
    //     "I don't know" (判据 §8). It corroborates nothing either way; it is
    //     not a second reading of the first fact.
    //
    // `Unsupported` is the fail-closed answer, so the VALUE is right under
    // both witnesses. What is being recorded here is that it is right on one
    // of them, not two.
    js_dialogs: Cap::Unsupported,
    // **Unsupported as the fail-closed answer to an unknown, not as a
    // measurement.** `browser_drag` does NOT send `Input.dispatchDragEvent`:
    // `cdp_backend::actions::drag` sends six `Input.dispatchMouseEvent`s
    // (move, press, three interpolated moves, release), which obscura answers
    // `ok`. Task 0 probed `Input.dispatchDragEvent` — absent on obscura, and a
    // report-success-no-op on Chrome — but that is a method no Aleph verb
    // dispatches, so it certifies nothing either way about this row.
    //
    // What is actually unknown is whether OBSCURA delivers a synthetic mouse
    // sequence to a page's own drag listeners. Nobody has measured that on
    // this engine: T0 read no `effect` for `Input.dispatchMouseEvent`, and
    // `qa/browser_managed`'s drag stage — which DOES read the effect, and is
    // what Chromium's row rests on — has never been run against obscura. (The
    // earlier draft of this comment said "nobody has measured it" without the
    // engine, which read as though the two rows answered the same evidence
    // state; they do not, and the field doc above now states the rule both
    // arms are answering.) An unknown may not be spent as a permission
    // (判据 §8), so the row stays
    // `Unsupported` and the refusal names chromium — and
    // `capability_table_agrees_with_the_t0_support_matrix`'s `NOT_PROBED`
    // carries the reason, so this exemption is argued rather than forgotten.
    drag: Cap::Unsupported,
    // **Corrected in task 16 against Task 0's probe, which ran the binary.**
    // The source survey read `domains/dom.rs:257`, saw a real implementation,
    // and recorded YES. The probe asked the running v0.2.2 server and got:
    //   "DOM.setFileInputFiles is disabled. Restart with
    //    `obscura serve --allow-file-access` to enable local file uploads."
    // The arm exists and is gated, and Aleph never passes that flag — spec
    // §7.2 forbids it, and `obscura`'s
    // `allow_file_access_appears_nowhere_in_the_browser_subsystem` is what
    // keeps that true. So under every argv Aleph will ever produce, obscura
    // cannot upload a file, and a table that said otherwise would send the
    // model at a verb the default engine always refuses.
    //
    // A static read of an arm is not a measurement of a gate in front of it
    // (判据 §18): the survey and the probe were not two readings of one fact,
    // they were readings of two different facts, and only one of them is the
    // one `browser_upload` meets. (The OTHER upload route,
    // `Page.setInterceptFileChooserDialog`, is a NOOP at
    // `domains/page.rs:1382` and no verb uses it.)
    file_upload: Cap::Unsupported,
    // `Page.printToPDF` → `domains/pdf.rs` (`page.rs:1534`), render feature —
    // which the ledger's chosen archive carries. `browser_pdf`'s route.
    pdf: Cap::Supported,
    // Landed in obscura ce9714f, 2026-09-04 (`domains/input.rs:329`).
    insert_text: Cap::Supported,
    // **Unsupported as a MEASURED verdict, 2026-09-26, obscura v0.2.2
    // (x86_64-linux) — `qa/browser_dual`'s `caps` stage, three premises.**
    // Written `Unsupported` at implementation time as the fail-closed answer
    // to an unknown (no binary on that machine); the caps probe then measured
    // the two premises the row stated and added the third the first two
    // could not see:
    //   1. window-global persistence across `Runtime.evaluate` calls — TRUE;
    //   2. node-level listeners observe synthetic `Input.dispatchMouseEvent`
    //      — TRUE (window-level capture listeners observe NOTHING; the
    //      production probe's `this.addEventListener` is node-level, which is
    //      the only reason the probe shape works at all);
    //   3. the read-back after a SELF-NAVIGATING click — FALSE: the click's
    //      default action fires (the page navigates), `window.__alephProbeHit`
    //      dies with the document, and the read-back evaluates cleanly on the
    //      NEW document answering null — where chromium's read-back either
    //      wins the race into the old context or errors out
    //      (`ProbeReadback::Unknown`, action stands). A clean null is
    //      indistinguishable from "never landed", so arming the probe here
    //      turns every navigating click into a false `EffectNotDelivered` —
    //      measured end-to-end by `qa/browser_dual`'s `click` stage, which
    //      went red on exactly that message the one run the row was flipped
    //      to `Supported`.
    // The gate stays closed; what would open it is a read-back that survives
    // (or detects) the navigation the dispatch caused, not a new obscura build
    // alone.
    effect_probe: Cap::Unsupported,
    // A pure lookup against Aleph's own per-tab `RefTable` — no engine
    // round-trip, so nothing to measure per engine (see the field doc).
    ref_precheck: Cap::Supported,
    // **Unsupported as a MEASURED verdict, 2026-09-27, obscura v0.2.2
    // (x86_64-linux) — the probe the mock plan's Task 4 promised
    // (docs/superpowers/specs/2026-09-24-browser-network-mock-design/
    // probes/fetch-probe.mjs), which measures the handshake link by link
    // rather than end to end:**
    //   1. `Fetch.enable{catch-all}` answers ok — the arm EXISTS;
    //   2. a navigation's `requestPaused` ARRIVES — but the request is NOT
    //      HELD: left unanswered for 1.5 s the page still reaches `complete`
    //      with the real body (3/3 runs). The pause is advisory;
    //   3. `fulfillRequest` on that pause answers ok and changes nothing —
    //      the page shows the REAL body (3/3): a fulfilled-nothing;
    //   4. a subresource `fetch()` pause arrives only SOMETIMES (2 of 3 runs;
    //      when it does not, the fetch wedges pending forever — a request
    //      held with no event to answer it is worse than no interception);
    //   5. when the subresource event does arrive, fulfill serves the mock
    //      correctly — the one working link cannot carry the chain.
    // A mock route on this engine would report `active` while every page
    // load slips past it — the exact lie this row exists to prevent. Written
    // `Unsupported` at implementation time as the fail-closed answer to an
    // unknown; the probe turned the unknown into a measured NO (判据 §8
    // spent the other way). What would flip it: a build whose navigation
    // pauses HOLD (link 2) and whose subresource events are reliable
    // (link 4) — re-run the probe.
    network_interception: Cap::Unsupported,
    // **Supported on a MEASUREMENT (2026-09-30, pinned v0.2.2, x86_64-linux),
    // upgraded from the fail-closed NOT_PROBED answer the row shipped with.**
    // The recording plan's Task 4 probe (docs/superpowers/specs/
    // 2026-09-27-browser-recording-design/probes/screencast-probe.mjs) pointed
    // `Page.startScreencast{jpeg}` at this build and read the C1-tightened
    // criterion — arrival alone is not usability: 12 frames, EVERY one
    // base64-decodable AND JPEG-magic'd (FFD8), every ack accepted, and
    // `stopScreencast` actually stopped the stream (0 frames after). The
    // same build whose Fetch pauses are advisory serves a sound screencast —
    // capability is per-domain, not per-binary, which is why these are two
    // rows and not one.
    screencast: Cap::Supported,
    // **Unsupported on a MEASUREMENT (2026-10-01, pinned v0.2.2, x86_64-linux)
    // — the fail-closed NOT_PROBED answer the row shipped with is spent.**
    // The same-pump argument (`Runtime.consoleAPICalled`, measured working on
    // this build) argued FOR arrival — and lost. Two independent readings
    // fired an async throw whose callback was PROVEN to run (a window flag
    // set before the throw, 判据 §2): `qa/browser_dual`'s caps line and the
    // spec-side probe (docs/superpowers/specs/2026-09-30-browser-qa-design/
    // probes/qa-events-probe.mjs). `Runtime.exceptionThrown` never arrived
    // within 10 s on either. The `Page.javascriptDialogOpening` silent miss
    // (see the `js_dialogs` row) is now a pattern, not an anecdote: this
    // build does not broadcast its Runtime-domain page faults. What flips
    // the row: a build that emits the event — re-run the caps line.
    error_events: Cap::Unsupported,
    // **Derived, never spelled.** `runtimes::specs` owns the pinned tag
    // (`obscura_tag!` / `OBSCURA_TAG`) and its doc says "bump this — and only
    // this — … everything else follows". A literal here made this row a second
    // author for that tag, outside the one-file scope of
    // `the_obscura_tag_has_one_author`, so a bump would have left the model
    // being told its capability facts were measured on a build it is not
    // running, with nothing red (判据 §1, §3). `the_obscura_row_is_stamped_with
    // _the_pinned_tag` below is the falsifier for both halves.
    measured_on: OBSCURA_TAG,
};

static CHROMIUM: EngineCapabilities = EngineCapabilities {
    js_dialogs: Cap::Supported,
    // **Not a T0 reading — T0 read no `effect` for `Input.dispatchMouseEvent`
    // on either engine.** This row rests on `qa/browser_managed`'s drag stage,
    // which drives `browser_drag` against a real Chromium and then asserts the
    // page saw it: `qa/browser_managed/pages/tools.html` records `dropped:html5`
    // or `dropped:mouseup` and the driver checks `#dropped != "dropped:no"`.
    // That is an effect reading for the method this verb sends, which is
    // exactly what the field doc's rule asks for and exactly what obscura does
    // not have.
    drag: Cap::Supported,
    file_upload: Cap::Supported,
    pdf: Cap::Supported,
    insert_text: Cap::Supported,
    // Session-scoped JS-global persistence and trusted-input delivery of
    // `Input.dispatch*` to page listeners are the two premises the probe
    // needs — and they are the same premises the DevTools console, this
    // repo's own `evaluate`/`wait_probe` verbs, and the whole click path
    // already run on (the liveness/occlusion probes measured on Chrome
    // 152/153 rely on the first; every `browser_click` relies on the
    // second). `qa/browser_dual`'s `caps` stage is the expiry check.
    effect_probe: Cap::Supported,
    // Engine-agnostic: a local table read, no wire call (see the field doc).
    ref_precheck: Cap::Supported,
    // The Fetch chain runs over the same connection and event pump that the
    // console/evaluate verbs already run on (both measured), and the five
    // method wrappers' wire shapes are pinned by aleph-cdp's FakeCdpServer
    // tests — but no REAL chromium has served a mocked request end to end
    // yet: qa/browser_dual's caps stage (Task 4 of
    // docs/superpowers/plans/2026-09-24-browser-network-mock.md) is the
    // expiry check, same role it plays for `effect_probe`.
    network_interception: Cap::Supported,
    // **Measured end to end on a REAL chromium — the project's first
    // real-chromium recording reading.** Same probe, `--engine chrome
    // --encode` (docs/superpowers/specs/2026-09-27-browser-recording-design/
    // probes/screencast-probe.mjs), 2026-09-30 on Chrome 151.0.7922.169
    // (headless=new; the row's `measured_on` below keeps naming Chrome 152
    // because the OTHER rows' readings were taken there — a re-confirmation
    // is not a re-measurement, 判据 §18): 38 frames, every one decodable
    // JPEG, acks accepted, clean stop — then the full production ffmpeg leg
    // (image2pipe + `-c:v mjpeg` hint + `-c:v libvpx`): 217 frames in, exit
    // 0, and ffprobe reads a vp8 stream back out of the webm. Until that
    // date this row rested on the same-connection argument alone; the probe
    // replaced the argument with a reading.
    screencast: Cap::Supported,
    // **Measured 2026-10-01 on Chrome 151.0.7922.169 (headless=new) — the
    // same-pump inference the row shipped with is replaced by a reading.**
    // The spec-side probe (docs/superpowers/specs/2026-09-30-browser-qa-
    // design/probes/qa-events-probe.mjs, `--engine chrome`) fired an async
    // throw and `Runtime.exceptionThrown` arrived naming the probe marker
    // (exceptionDetails, uncaught). The pump and the `Runtime.enable`
    // subscription are shared with `Runtime.consoleAPICalled`, as the old
    // inference said; what changed is that THIS event's arrival is now a
    // fact rather than an argument from the sibling. `qa/browser_dual`'s
    // caps line keeps the obscura half honest.
    error_events: Cap::Supported,
    // Provenance, not a freshness stamp: this row records the build the
    // capabilities were measured on. Re-confirmed unchanged on Chrome
    // 153.0.8010.36 (2026-09-13) — recorded here rather than by editing the
    // value above, because a re-confirmation is not a re-measurement and the
    // field must keep naming the build the reading was taken under (判据 §18).
    measured_on: "Chrome 152.0.7977.76",
};

#[must_use]
pub const fn capabilities(engine: Engine) -> &'static EngineCapabilities {
    match engine {
        Engine::Obscura => &OBSCURA,
        Engine::Chromium => &CHROMIUM,
    }
}

/// The engine that supports the capability `pick` selects, or `None` when
/// neither does.
///
/// This is the `supported_by` half of every `BrowserError::UnsupportedByEngine`
/// — derived here rather than typed out at each refusal site, so the refusal
/// and the table cannot disagree about which engine to send the caller to
/// (判据 §1). A hint naming the engine the caller is already on would be worse
/// than no hint: a closed gate has to name a door that opens (判据 §14).
#[must_use]
pub fn supported_by(pick: fn(&EngineCapabilities) -> Cap) -> Option<Engine> {
    [Engine::Chromium, Engine::Obscura]
        .into_iter()
        .find(|e| pick(capabilities(*e)) == Cap::Supported)
}

/// The machine-readable table: `{ "<engine>": { "<cap>": "<state>", …,
/// "measured_on": "…" } }`.
///
/// One author with [`describe_for_tool`] — both walk [`CAP_FIELDS`] — so the
/// sentence the model reads and the object the QA fixture diffs cannot say
/// different things.
#[must_use]
pub fn capabilities_json() -> serde_json::Value {
    let mut root = serde_json::Map::new();
    for engine in Engine::ALL {
        let caps = capabilities(engine);
        let mut row = serde_json::Map::new();
        for (name, get) in CAP_FIELDS {
            row.insert(
                (*name).to_string(),
                serde_json::Value::String(get(caps).as_str().to_string()),
            );
        }
        row.insert(
            "measured_on".to_string(),
            serde_json::Value::String(caps.measured_on.to_string()),
        );
        root.insert(engine.as_str().to_string(), serde_json::Value::Object(row));
    }
    serde_json::Value::Object(root)
}

/// The prose form, for the `browser_session{action:"capabilities"}` result.
///
/// **Deliberately NOT in any tool's `DESCRIPTION`** (plan ruling R2'). That is
/// a `const &'static str` and the builtin catalog is a `const` array fed from
/// it, so a runtime `String` cannot get there — and a second, hand-written
/// copy of this table living in a description would be the exact defect
/// `builtin_registry::definitions`'s module doc records, plus four lines of
/// engine prose on every turn's tool listing, which is what R9's *prune the
/// prompt* prunes. The description names the ACTION; the action serves the
/// table.
///
/// The model that never asks still learns the gap at the only moment it
/// matters: `BrowserError::UnsupportedByEngine` names the verb and the engine
/// that has it, and that engine comes from [`supported_by`] — the same table.
#[must_use]
pub fn describe_for_tool() -> String {
    let mut out = String::from(
        // What moves and what does not is `cdp_backend::migration`'s
        // `MigrationState` — three fields, three carried clauses. This sentence
        // said scroll position does NOT move; the switch reads
        // `window.scrollX/Y` and writes `window.scrollTo`, so it did, and the
        // sentence was the one lying (判据 §1).
        //
        // The EXHAUSTIVE list lives here rather than in
        // `BrowserSessionTool::DESCRIPTION` and that is the R2' split, not an
        // accident: this string is the RESULT of `action='capabilities'`, so it
        // costs nothing on a turn that never asks, while `DESCRIPTION` ships
        // with the tool list on every turn. `DESCRIPTION` therefore keeps only
        // the three things a model needs BEFORE it decides — the verb, what it
        // carries, and that page state and every ref are lost — and points
        // here for the rest.
        "Browser engines. obscura is the default; chromium is the escape hatch, reached with \
         browser_session{action:\"switch_engine\", engine:\"chromium\"}. A switch moves every \
         cookie, each open tab's URL and scroll position, and the localStorage of each open \
         tab's origin (best effort). It does not move the JS heap or in-flight form input, \
         the history stack, sessionStorage, IndexedDB, service workers, or the localStorage \
         of origins with no open tab; every snapshot ref dies with the old process.\n",
    );
    for engine in Engine::ALL {
        let caps = capabilities(engine);
        let gaps: Vec<&str> = CAP_FIELDS
            .iter()
            .filter(|(_, get)| get(caps) != Cap::Supported)
            .map(|(name, _)| *name)
            .collect();
        if gaps.is_empty() {
            out.push_str(&format!(
                "{}: every verb in this table. Measured on {}.\n",
                engine.as_str(),
                caps.measured_on
            ));
        } else {
            out.push_str(&format!(
                "{}: cannot {}. Measured on {}.\n",
                engine.as_str(),
                gaps.join(", "),
                caps.measured_on
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows Task 0's probe does not settle, each with the reason.
    ///
    /// **`drag` is here because the label and the verb are different
    /// methods.** Task 0 probed `Input.dispatchDragEvent`;
    /// `cdp_backend::actions::drag` sends `Input.dispatchMouseEvent` six times
    /// and never sends `dispatchDragEvent` at all. Judging the row on that
    /// label would be 判据 §12 — deriving a decision in one space and applying
    /// it in another — and it would have flipped Chromium's row to
    /// `Unsupported` on a measurement of a method Aleph does not use. The
    /// method the verb DOES send is `ok` on both engines with no effect
    /// reading, i.e. unmeasured by T0, so the row is decided by the rule
    /// stated on the `drag` FIELD and the citations beside each value.
    ///
    /// Module-scope rather than local to
    /// `capability_table_agrees_with_the_t0_support_matrix`, because it is
    /// exactly "the rows whose evidence is not in the matrix" and that is the
    /// set `every_matrix_exempt_disagreement_cites_its_reading` has to walk. A
    /// second copy over there would be the two-authors shape this file spends
    /// most of its guards on (判据 §1).
    /// The rows in [`NOT_PROBED`] are the ones whose evidence lives
    /// outside the T0 matrix. `effect_probe`'s entry changed KIND on
    /// 2026-09-26: still not settled by T0's wire probe (it has no label
    /// there), but no longer UNMEASURED — `qa/browser_dual`'s `caps` stage
    /// ran the three premises against the real pinned-tag binary and the
    /// row is now a measured `Unsupported` (see the `OBSCURA` row for the
    /// evidence and what would open the gate).
    const NOT_PROBED: [(&str, &str); 6] = [
        (
            "drag",
            "T0's Input.dispatchDragEvent label measures a method no browser_* verb \
             dispatches; browser_drag sends Input.dispatchMouseEvent, whose effect T0 \
             did not read on either engine",
        ),
        (
            "effect_probe",
            "T0's wire matrix has no label for the probe; settled instead by \
             qa/browser_dual's caps stage against the real pinned-tag binary on \
             2026-09-26: premises 1-2 (window-global persistence, node-level \
             delivery) hold, premise 3 (read-back after a self-navigating click) \
             fails — a clean null on the new document is indistinguishable from \
             'never landed', so the probe would lie on every link click",
        ),
        (
            "ref_precheck",
            "no engine round-trip to probe: the check is a pure lookup against the cdp \
             backend's per-tab RefTable plus the tab registry's URL-drift signal, so the \
             wire matrix has no label that could decide it",
        ),
        (
            "network_interception",
            "T0's wire matrix has no Fetch label; settled instead by the mock plan's \
             Task 4 probe (docs/superpowers/specs/2026-09-24-browser-network-mock-design/\
             probes/fetch-probe.mjs) against the pinned build on 2026-09-27: \
             Fetch.enable answers and events arrive, but navigation pauses are \
             advisory (the request completes unanswered) and subresource pauses \
             are unreliable — a MEASURED Unsupported, not an unknown",
        ),
        (
            "screencast",
            "T0's wire matrix has no screencast label; settled instead by the \
             recording plan's Task 4 probe (docs/superpowers/specs/\
             2026-09-27-browser-recording-design/probes/screencast-probe.mjs) \
             on 2026-09-30: the pinned obscura build serves a sound stream \
             (every frame decodable JPEG, acks accepted, clean stop), and \
             Chrome 151.0.7922.169 passed the full encode leg (ffmpeg exit 0, \
             ffprobe-readable vp8 webm) — a MEASURED Supported on BOTH engines",
        ),
        (
            "error_events",
            "T0's wire matrix has no Runtime.exceptionThrown label; settled by C3 \
             Task 4 on 2026-10-01 (qa/browser_dual's caps line + docs/superpowers/\
             specs/2026-09-30-browser-qa-design/probes/qa-events-probe.mjs): the \
             pinned obscura build NEVER emits the event (the throwing callback was \
             proven to run — a MEASURED Unsupported, the second silent-miss of its \
             kind), while Chrome 151.0.7922.169 delivers it naming the marker — a \
             MEASURED Supported",
        ),
    ];

    /// A row with no provenance cannot be re-measured, and an empty string
    /// reads exactly like a row someone forgot to fill in.
    #[test]
    fn every_row_names_the_build_it_was_measured_on() {
        for engine in Engine::ALL {
            assert!(
                !capabilities(engine).measured_on.is_empty(),
                "{engine:?} has no measured_on"
            );
        }
    }

    /// The `supported_by` hint must name an engine that really supports the
    /// verb. Written over the three rows Tasks 12–13 actually refuse on, so it
    /// goes red if one of them is flipped without its refusal being revisited.
    #[test]
    fn supported_by_names_an_engine_that_actually_supports_it() {
        assert_eq!(supported_by(|c| c.drag), Some(Engine::Chromium));
        assert_eq!(supported_by(|c| c.pdf), Some(Engine::Chromium));
        assert_eq!(supported_by(|c| c.js_dialogs), Some(Engine::Chromium));
        // Both engines upload, so this one's hint is the engine you are already
        // on — which is why `upload`'s refusal is unreachable on both today.
        assert_eq!(supported_by(|c| c.file_upload), Some(Engine::Chromium));
        // The probe row: obscura is Unsupported on a MEASUREMENT (caps stage,
        // 2026-09-26 — the read-back cannot survive a self-navigating click),
        // so the door the hint names must be chromium.
        assert_eq!(supported_by(|c| c.effect_probe), Some(Engine::Chromium));
    }

    /// `supported_by` must be able to answer `None`, or its `Option` return is
    /// a shape no input reaches and every refusal's "use this engine instead"
    /// is unconditional (判据 §2). No production row is `Unsupported` on both
    /// engines today, so the only way to reach the arm is a `pick` that is
    /// unsatisfiable — which is exactly what the day a row goes double-
    /// `Unsupported` would look like.
    #[test]
    fn supported_by_answers_none_when_no_engine_supports_the_pick() {
        assert_eq!(supported_by(|_| Cap::Unsupported), None);
        // `Partial` is not `Supported`: a row with a behavioural caveat must
        // not be advertised as the door that opens.
        assert_eq!(supported_by(|_| Cap::Partial), None);
    }

    /// `as_str` covers every variant with a distinct word. A `match` that fans
    /// three classes into one string is 判据 §2's shape, and this table reaches
    /// the model as prose.
    #[test]
    fn every_cap_renders_a_distinct_word() {
        let words = [Cap::Supported, Cap::Partial, Cap::Unsupported].map(Cap::as_str);
        assert_eq!(words, ["supported", "partial", "unsupported"]);
        assert_eq!(
            words.iter().collect::<std::collections::HashSet<_>>().len(),
            words.len(),
            "two caps render the same word: {words:?}"
        );
    }

    /// The row the source survey and the running binary disagreed about, and
    /// which way that disagreement was resolved.
    ///
    /// Kept as its own named test rather than folded into the matrix check
    /// because the matrix check reads a file and this reads the RULE: obscura
    /// gates `DOM.setFileInputFiles` behind a flag Aleph never passes, so the
    /// row is `Unsupported` no matter what the arm at `domains/dom.rs:257`
    /// does. If someone ever makes Aleph pass `--allow-file-access`, the
    /// census in `obscura.rs` goes red first and this one second.
    #[test]
    fn obscura_cannot_upload_because_aleph_never_passes_the_flag_that_would_let_it() {
        assert_eq!(capabilities(Engine::Obscura).file_upload, Cap::Unsupported);
        assert_eq!(capabilities(Engine::Chromium).file_upload, Cap::Supported);
        // And the refusal has a door: `browser_upload` on obscura must name an
        // engine that really can (判据 §14).
        assert_eq!(supported_by(|c| c.file_upload), Some(Engine::Chromium));
    }

    /// The two rows R39 cut, asserted as ABSENT.
    ///
    /// A cut that is only recorded in a comment comes back: the next reader
    /// measures obscura's touch handling, finds it works, and adds the row
    /// without noticing that no verb reaches it. The table is a menu the
    /// model reads (`browser_session{action:"capabilities"}`), so a row it
    /// cannot order is worse than a missing one — 判据 §9, and 判据 §17's
    /// "point at the line that renders it" applied to a capability instead
    /// of a UI string. (`screencast` USED to be on this list; it left it the
    /// day `browser_record`'s three actions became the verbs that dispatch
    /// it — the restoration the struct doc records.)
    #[test]
    fn no_capability_row_exists_without_a_verb_that_dispatches_it() {
        let src = include_str!("capability.rs");
        for (cut, why) in [
            (
                "touch",
                "no browser_tap verb; nothing dispatches Input.dispatchTouchEvent",
            ),
            ("multi_connection", "an architectural property, not a verb"),
        ] {
            assert!(
                !CAP_FIELDS.iter().any(|(n, _)| *n == cut),
                "`{cut}` is back in CAP_FIELDS — {why}. If a verb now dispatches \
                 it, add the row AND name that verb in its doc comment (R39)."
            );
            assert!(
                !src.contains(&format!("pub {cut}: Cap,")),
                "`{cut}` is back on EngineCapabilities — {why}"
            );
        }
    }

    /// Every surviving row names its verb. The table is published to the
    /// model, so "which tool do I call to use this" must be answerable from
    /// the row itself, not from a reader's memory of the backend.
    #[test]
    fn every_capability_row_names_the_verb_that_dispatches_it() {
        let src = include_str!("capability.rs");
        for (name, verb) in [
            ("js_dialogs", "browser_dialog"),
            ("drag", "browser_drag"),
            ("file_upload", "browser_upload"),
            ("pdf", "browser_pdf"),
            ("insert_text", "browser_type"),
            // The probe is shared by three verbs; the doc must name all of
            // them, or the model cannot tell which tools the row governs.
            ("effect_probe", "browser_click"),
            ("effect_probe", "browser_type"),
            ("effect_probe", "browser_fill_form"),
            // The precheck guards the five ref-taking verbs; the doc must
            // name all five for the same reason.
            ("ref_precheck", "browser_click"),
            ("ref_precheck", "browser_type"),
            ("ref_precheck", "browser_fill_form"),
            ("ref_precheck", "browser_select"),
            ("ref_precheck", "browser_hover"),
            // The mock routes are one row behind four actions of ONE verb;
            // the doc must name the verb and every action, or the model
            // cannot tell which calls the row governs.
            ("network_interception", "browser_network"),
            ("network_interception", "mock_add"),
            ("network_interception", "mock_list"),
            ("network_interception", "mock_remove"),
            ("network_interception", "mock_clear"),
            // Session recording is one row behind three actions of ONE verb;
            // same naming rule as the mock routes.
            ("screencast", "browser_record"),
            ("screencast", "start"),
            ("screencast", "stop"),
            ("screencast", "status"),
            // The exception fold is one row behind ONE dimension of ONE verb.
            ("error_events", "browser_qa"),
        ] {
            assert!(
                CAP_FIELDS.iter().any(|(n, _)| *n == name),
                "capability `{name}` is missing from CAP_FIELDS"
            );
            // The doc comment sits directly above the field; find the field and
            // walk back over the `///` lines.
            //
            // ⚠️ `skip_while(empty)` is load-bearing, not tidiness: `src[..at]`
            // ends at the `p` of `pub`, so the last `lines()` item is the
            // field's own indentation — a non-`///` fragment that stops
            // `take_while` dead and leaves `doc` EMPTY for every field. Without
            // it this guard is 恒红 rather than green-when-wrong, which is the
            // fourth face of 判据 §2 and the one that gets a guard weakened
            // instead of fixed. (It was written that way and caught on the
            // first run.)
            let field = format!("pub {name}: Cap,");
            let at = src
                .find(&field)
                .unwrap_or_else(|| panic!("no field `{name}` on EngineCapabilities"));
            let doc: String = src[..at]
                .lines()
                .rev()
                .skip_while(|l| l.trim().is_empty())
                .take_while(|l| l.trim_start().starts_with("///"))
                .collect();
            assert!(
                !doc.is_empty(),
                "the doc walk found no `///` lines above `{name}` — the walk is \
                 broken, not the row"
            );
            assert!(
                doc.contains(verb),
                "capability `{name}`'s doc comment does not name `{verb}`, the verb \
                 that dispatches it (R39). A row the model cannot order is worse \
                 than a missing one."
            );
        }
    }

    /// Chromium's row exists so the table can answer "who DOES support this",
    /// which is the half of `UnsupportedByEngine` that makes it actionable. A
    /// table with one engine in it cannot answer that at all.
    #[test]
    fn the_chromium_row_supports_every_verb_and_says_what_it_was_measured_on() {
        let c = capabilities(Engine::Chromium);
        for (name, get) in CAP_FIELDS {
            assert_eq!(get(c), Cap::Supported, "chromium.{name}");
        }
        assert!(c.measured_on.starts_with("Chrome "), "{}", c.measured_on);
    }

    /// F5 — the obscura row's provenance stamp has ONE author, and it is
    /// `runtimes::specs`.
    ///
    /// Both halves are load-bearing and neither implies the other. The
    /// equality alone would stay green against a re-introduced literal that
    /// happens to match today and stops matching on the next bump; the
    /// spelling census alone would stay green against a stamp derived from
    /// some other constant. Before this, a mutation of the stamp to
    /// `"v0.9.9-MUTANT"` reddened nothing in the whole crate — the table was
    /// free to tell the model its capability facts were measured on a build
    /// nobody is running.
    #[test]
    fn the_obscura_row_is_stamped_with_the_pinned_tag() {
        assert_eq!(
            capabilities(Engine::Obscura).measured_on,
            OBSCURA_TAG,
            "the obscura row must be stamped with the tag the installer pins, \
             not with a build of its own"
        );
        let spellings = include_str!("capability.rs")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains(OBSCURA_TAG))
            .count();
        assert_eq!(
            spellings, 0,
            "the pinned tag is spelled as a literal in capability.rs; refer to \
             OBSCURA_TAG instead, or `the_obscura_tag_has_one_author` (which \
             reads only specs.rs) will certify two authors as one"
        );
    }

    /// F7 — a disagreement the T0 matrix does not settle must cite the
    /// reading that DOES settle it, and that citation must still exist.
    ///
    /// The rows in `ROWS` need nothing here: their two values are each checked
    /// against the matrix, so a disagreement there is derived rather than
    /// asserted. The rows in [`NOT_PROBED`] are the ones whose evidence lives
    /// outside this file — and a disagreement among THOSE is a route published
    /// to the model (`supported_by` answers "chromium supports it" and the
    /// model pays an engine switch for it) resting on reasoning no test can
    /// see.
    ///
    /// The subject is derived from `NOT_PROBED`, so a second exempt row that
    /// starts disagreeing fails here instead of inheriting `drag`'s argument
    /// (判据 §3). The citation itself is read rather than trusted: a reference,
    /// once written, turns the referenced thing into something that may not
    /// change silently (判据 §1, fourth form), and this is the wire that makes
    /// that true rather than hoped.
    #[test]
    fn every_matrix_exempt_disagreement_cites_its_reading() {
        let o = capabilities(Engine::Obscura);
        let c = capabilities(Engine::Chromium);
        let pick = |name: &str| {
            CAP_FIELDS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, get)| *get)
                .unwrap_or_else(|| panic!("NOT_PROBED names `{name}`, which is not a Cap field"))
        };

        let exempt_disagreements: Vec<&str> = NOT_PROBED
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| pick(name)(o) != pick(name)(c))
            .collect();
        assert_eq!(
            exempt_disagreements,
            vec![
                "drag",
                "effect_probe",
                "network_interception",
                "error_events"
            ],
            "a matrix-exempt row other than \
             `drag`/`effect_probe`/`network_interception`/`error_events` \
             now answers differently per engine. That difference is published to the model \
             as a route, and nothing in the matrix decides it — state the \
             reading that does, beside BOTH values, and check it here"
        );

        // effect_probe's disagreement rests on a MEASURED ruling since
        // 2026-09-26 (caps stage: premise 3 fails — the read-back answers a
        // clean null on the new document after a self-navigating click), and
        // BOTH values must say what the row needs to flip — checked in the
        // source rather than trusted, because a published route resting on an
        // argument nobody can read is the shape this test exists for.
        let src = include_str!("capability.rs");
        assert!(
            src.contains("qa/browser_dual") && src.contains("clean null"),
            "obscura's effect_probe row no longer states the measurement it rests on"
        );
        // network_interception's disagreement rests on a MEASUREMENT on the
        // obscura side since 2026-09-27 (the mock plan's Task 4 probe:
        // navigation pauses advisory, subresource pauses unreliable), while
        // chromium's Supported still rests on the Fetch chain running over
        // the same connection and event pump the probed verbs already use.
        // Both rows must keep naming that probe, or the route published to
        // the model rests on an argument nobody can read.
        // Scoped to the PRODUCTION prefix: the plan name also appears in
        // NOT_PROBED, which is test text — a citation that lives only beside
        // the guard certifies nothing about the rows it guards.
        let production = crate::utils::source_scan::production_prefix(src);
        assert!(
            production.contains("2026-09-24-browser-network-mock"),
            "the network_interception rows no longer name the Task 4 probe that \
             decides obscura's value"
        );
        // error_events' disagreement rests on MEASUREMENTS on both sides
        // since 2026-10-01 (C3 Task 4): obscura never emits
        // Runtime.exceptionThrown (the throwing callback proven to run — the
        // second silent-miss of its kind, after javascriptDialogOpening),
        // chromium delivers it (Chrome 151, qa-events-probe). Both rows must
        // keep naming the probe and the precedent, or the route published to
        // the model rests on an argument nobody can read.
        assert!(
            production.contains("javascriptDialogOpening")
                && production.contains("qa-events-probe"),
            "the error_events rows no longer state the precedent (obscura) and the \
             probe (both engines) they rest on"
        );
        // screencast's disagreement CLOSED on 2026-09-30: the Task 4 probe
        // measured obscura's stream sound (it is now `Supported` on both
        // engines, so the row no longer appears in the exempt list above).
        // The citation assert stays: both values rest on that probe, and a
        // published fact whose reading nobody can read is the shape this
        // test exists for — measured or not.
        assert!(
            production.contains("2026-09-27-browser-recording"),
            "the screencast rows no longer name the Task 4 probe that \
             decided both engines' values"
        );

        // The citation, checked rather than trusted. `driver` drives the verb
        // and reads the effect back; `page` is the fixture that records which
        // mechanism fired. Either one losing the drag stage would leave
        // Chromium's `Supported` resting on a measurement that no longer runs.
        let driver = include_str!("../../../qa/browser_managed/drive_tools.py");
        let page = include_str!("../../../qa/browser_managed/pages/tools.html");
        assert!(
            driver.contains("browser_drag"),
            "qa/browser_managed no longer drives browser_drag; Chromium's drag \
             row cites it as its effect reading"
        );
        assert!(
            driver.contains("dropped:no"),
            "qa/browser_managed no longer reads the drop target back; a stage \
             that only asserts the CALL succeeded is a protocol reading, not \
             the effect reading this row rests on (判据 §4)"
        );
        assert!(
            page.contains("dropped:html5") && page.contains("dropped:no"),
            "the drag fixture page no longer records which mechanism fired"
        );
    }

    /// `CAP_FIELDS` is what `describe_for_tool`, `capabilities_json` and the
    /// QA `caps` stage all iterate. A field added to the struct and forgotten
    /// here is a capability that silently stops being described, probed or
    /// falsified — the list must be derived from the type it claims to cover
    /// (判据 §3). Derived by reading this file's own source, so adding a field
    /// without a row goes red on the next run.
    #[test]
    fn cap_fields_covers_every_cap_typed_field_of_the_struct() {
        let src = include_str!("capability.rs");
        let declared: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//"))
            .filter_map(|l| l.strip_prefix("pub "))
            .filter_map(|l| l.strip_suffix(": Cap,"))
            .collect();
        assert_eq!(
            declared.len(),
            CAP_FIELDS.len(),
            "struct declares {declared:?}, CAP_FIELDS lists {:?}",
            CAP_FIELDS.map(|(n, _)| n)
        );
        for name in declared {
            assert!(
                CAP_FIELDS.iter().any(|(n, _)| *n == name),
                "field `{name}` has no CAP_FIELDS row, so nothing describes or probes it"
            );
        }
    }

    /// The prose the model reads when it asks. Asserted for the FACTS it must
    /// carry, not byte-for-byte: it names each unsupported verb, names the
    /// engine that does support it, and names the version each row was measured
    /// on — because a capability claim with no measurement date is a name on a
    /// list that expires without telling anyone (判据 §5).
    #[test]
    fn describe_for_tool_names_every_gap_its_remedy_and_its_measurement() {
        let text = describe_for_tool();
        // Derived, not listed: the gaps this must name are exactly the rows
        // that are not `Supported`, so a row that changes state changes what
        // this test demands without anyone editing it (判据 §5 — a hand-written
        // list is the thing that rots).
        let obscura = capabilities(Engine::Obscura);
        let gaps: Vec<&str> = CAP_FIELDS
            .iter()
            .filter(|(_, get)| get(obscura) != Cap::Supported)
            .map(|(n, _)| *n)
            .collect();
        assert!(
            gaps.len() >= 2,
            "this assertion is vacuous if obscura has no gaps: {gaps:?}"
        );
        for verb in gaps {
            assert!(text.contains(verb), "must name the gap {verb}: {text}");
        }
        assert!(
            text.contains("chromium"),
            "must name the engine that has them: {text}"
        );
        assert!(text.contains(obscura.measured_on), "{text}");
        assert!(
            text.contains(capabilities(Engine::Chromium).measured_on),
            "{text}"
        );
        assert!(
            text.contains("switch_engine"),
            "a gap with no named way across is fail-dead (判据 §14): {text}"
        );
    }

    /// **The `Cap` table against Task 0's measurements.**
    ///
    /// The table is a name list, and until this test its only falsifier needed
    /// a real binary, a real machine and eight minutes. This is the same claim
    /// at `cargo test` speed, checked against the probe results Task 0 already
    /// captured — which until now no code read at all.
    ///
    /// `include_str!`, not `std::fs::read_to_string`: a missing matrix is then
    /// a COMPILE error naming the path, rather than a test that quietly finds
    /// nothing and certifies the table by looking nowhere (判据 §3).
    ///
    /// ⚠️ The path is `fixtures/`, beside this file — **not** the docs tree the
    /// task brief named. Task 0's own manifest (`t0-results.md`, the `files`
    /// array of both captures) records that it wrote it here, and it is the
    /// only copy in the repository. The `include_str!` itself now lives at
    /// module scope as [`super::T0_SUPPORT_MATRIX`], because a second reader
    /// arrived.
    #[test]
    fn capability_table_agrees_with_the_t0_support_matrix() {
        const MATRIX: &str = super::T0_SUPPORT_MATRIX;

        /// Comment markers off, whitespace collapsed — so a text quoted
        /// across two wrapped `//` lines still compares equal to the single
        /// line the fixture holds it on.
        fn flatten(src: &str) -> String {
            let joined: Vec<&str> = src
                .lines()
                .map(|l| {
                    let t = l.trim_start();
                    t.strip_prefix("///")
                        .or_else(|| t.strip_prefix("//!"))
                        .or_else(|| t.strip_prefix("//"))
                        .unwrap_or(t)
                })
                .collect();
            joined
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        }

        /// How a row is judged.
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        enum Judge {
            /// `Supported` ⇔ the wire said `ok`. `effect` may be absent here:
            /// these verbs have no separate "did it actually happen" reading,
            /// so the protocol answer IS the capability answer.
            Protocol,
            /// The verb is a report-success-no-op candidate. A protocol `ok`
            /// proves nothing about these, so an `ok` with no boolean `effect`
            /// FAILS by name rather than being read either way (判据 §8, §11).
            ///
            /// The demand is conditioned on `protocol == "ok"` rather than made
            /// unconditionally: obscura's `Page.javascriptDialogOpening` entry
            /// is `{"protocol": "no event within 4s (wait expired, not a
            /// measured absence)", "effect": null}`, and demanding an effect
            /// reading for a call that never succeeded would be a 恒红 arm —
            /// satisfiable only by inventing a measurement (判据 §2).
            ///
            /// ⚠️ That entry is an EXPIRED WAIT, not "a stated absence" — which
            /// is what this doc used to call it. The fixture spells the
            /// difference out in its own text and the paraphrase erased it
            /// (判据 §1: the comment is the lying half). The row it feeds is
            /// still `Unsupported`, on the source survey rather than on this
            /// entry — see `OBSCURA.js_dialogs`.
            /// `every_refusal_text_the_table_reasons_about_is_quoted_verbatim`
            /// is what keeps the next paraphrase from landing.
            Effect,
        }

        /// `(cap field, matrix label, how to judge it)`.
        const ROWS: [(&str, &str, Judge); 4] = [
            // The dialog probe records the EVENT, not the handler: it fires a
            // real `alert()` and waits 4 s. `Page.handleJavaScriptDialog` is
            // its paired label and is deliberately not used — on an engine
            // with no dialog it is `{protocol: "not reachable: no dialog
            // event", effect: null}`, which says nothing about the capability.
            ("js_dialogs", "Page.javascriptDialogOpening", Judge::Effect),
            ("file_upload", "DOM.setFileInputFiles", Judge::Protocol),
            ("pdf", "Page.printToPDF", Judge::Protocol),
            ("insert_text", "Input.insertText", Judge::Protocol),
        ];

        // Every `Cap` field is on exactly one of the two lists. A new
        // capability cannot join the unprobed set by being forgotten — the
        // list is derived from the type it claims to cover (判据 §3).
        assert_eq!(
            ROWS.len() + NOT_PROBED.len(),
            CAP_FIELDS.len(),
            "{} probed + {} unprobed != {} capability fields — a new field must \
             be given a probe or an argued exemption, not neither (and under \
             R39 it must first have a verb that dispatches it)",
            ROWS.len(),
            NOT_PROBED.len(),
            CAP_FIELDS.len()
        );
        for (name, _) in CAP_FIELDS {
            let probed = ROWS.iter().filter(|(n, _, _)| *n == name).count();
            let exempt = NOT_PROBED.iter().filter(|(n, _)| *n == name).count();
            assert_eq!(
                probed + exempt,
                1,
                "capability `{name}` appears {probed} time(s) in ROWS and \
                 {exempt} time(s) in NOT_PROBED; it must appear exactly once"
            );
        }

        let matrix: serde_json::Value =
            serde_json::from_str(MATRIX).expect("t0-support-matrix.json must be valid JSON");

        for engine in Engine::ALL {
            let caps = capabilities(engine);
            // ⚠️ Task 0 keyed the Chromium half `"chrome"`, not `"chromium"`.
            // Mapped in the reader, because the matrix is that task's output:
            // rewriting somebody else's measurement to match our vocabulary is
            // how a fixture stops being evidence (判据 §10 — the wire shape
            // belongs to whoever produced it). The mapping moved to
            // [`super::t0_key`] when the second reader arrived.
            let key = super::t0_key(engine);
            // Task 0 merges each engine's half into the same file, so a missing
            // half means that engine's capture never ran — a fact worth failing
            // on, not working around.
            let half = matrix.get(key).unwrap_or_else(|| {
                panic!(
                    "t0-support-matrix.json has no `{key}` object — that engine's \
                     T0 capture did not run, so this table is unverified for it"
                )
            });

            for (name, label, judge) in ROWS {
                let entry = half.get(label).unwrap_or_else(|| {
                    panic!("t0-support-matrix.json[{key}] has no label {label:?}")
                });
                let protocol = entry
                    .get("protocol")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| panic!("{key}/{label}: entry has no string `protocol`"));
                // Absent and explicit-null are the same answer here, and both
                // mean "not measured".
                let effect = entry.get("effect").and_then(serde_json::Value::as_bool);
                let claimed = CAP_FIELDS
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, get)| get(caps))
                    .expect("checked above");

                // `Partial` has no two-valued reading against this matrix. No
                // row uses it today; one that starts to must state its own
                // reading here first, rather than being folded onto `ok`.
                assert_ne!(
                    claimed,
                    Cap::Partial,
                    "{key}/{label}: `Partial` needs an explicit reading against the matrix"
                );

                if judge == Judge::Effect && protocol == "ok" && effect.is_none() {
                    panic!(
                        "{key}/{label}: this verb is a report-success-no-op candidate, so its \
                         protocol answer ({protocol:?}) says nothing about the capability. \
                         Task 0 must record a boolean `effect` for it — dispatch the event, \
                         then read the flag back. `null` means the read itself failed, and a \
                         failed read may not be spent as either answer."
                    );
                }

                // The rule, one line, both judges: the wire said ok AND nothing
                // observed it doing nothing.
                let measured = protocol == "ok" && effect != Some(false);
                assert_eq!(
                    claimed == Cap::Supported,
                    measured,
                    "{key}/{label}: the table says {claimed:?}, T0 measured \
                     protocol={protocol:?} effect={effect:?}"
                );

                // A refusal must SAY something. `protocol` carries the peer's
                // own text, and an empty one is how "unsupported" comes to mean
                // "nobody looked".
                if protocol != "ok" {
                    assert!(
                        !protocol.trim().is_empty(),
                        "{key}/{label}: not ok, but the matrix records no refusal text"
                    );
                    // ...and this file must QUOTE that text, not paraphrase it.
                    //
                    // The shape: obscura's dialog entry says, in the fixture's
                    // own words, that it records an expired wait and NOT a
                    // measured absence — and the prose beside the row it feeds
                    // called it a stated absence. The value was still right;
                    // the reasoning published beside it was not, and the
                    // comment is the half that lies (判据 §1). A paraphrase is
                    // not a cheaper copy of a measurement, it is a second one.
                    //
                    // ⚠️ **The corpus is the PRODUCTION prefix, and this
                    // comment reproduces none of the texts the rule checks.**
                    //
                    // Two self-matches, found one at a time, which is the
                    // lesson. The first draft quoted an example right here and
                    // was caught by counting copies before mutating (判据 §6):
                    // three, one of them the guard's own. Removing that left
                    // TWO — and the second was `Judge::Effect`'s doc, ten lines
                    // up, also inside `mod tests`. A guard that searches the
                    // whole file finds the prose ABOUT the rule and certifies
                    // the row on it, so paraphrasing the row itself stayed
                    // green (measured by the re-review: 14 passed, 0 failed).
                    // `production_prefix` is what makes the corpus the thing
                    // the rule is about — the same scoping
                    // `chromium.rs`'s `the_launch_source_still_names_the_
                    // requests_own_browser_type` already uses.
                    //
                    // Comment markers are stripped and whitespace collapsed
                    // before comparing, because these texts are line-wrapped
                    // inside `//` blocks; without that this would be 恒红 on a
                    // correctly-quoted file (判据 §2's fourth face).
                    let whole = include_str!("capability.rs");
                    let production = crate::utils::source_scan::production_prefix(whole);
                    assert!(
                        !production.is_empty() && production.len() < whole.len(),
                        "the #[cfg(test)] bound matched nothing or everything — the \
                         corpus is then either this guard's own prose or empty, and \
                         neither can falsify a paraphrase"
                    );
                    let flat = flatten(&production);
                    let needle = flatten(protocol);
                    assert!(
                        flat.contains(&needle),
                        "{key}/{label}: this table reasons about a refusal it does not \
                         quote. The matrix records:\n  {protocol}\nQuote that text \
                         verbatim beside the row it decides — a paraphrase is a second \
                         author for somebody else's measurement."
                    );
                }
            }
        }
    }

    /// The machine-readable form the QA `caps` stage diffs against. One author
    /// with the prose: both walk `CAP_FIELDS`.
    #[test]
    fn capabilities_json_is_one_object_per_engine_keyed_by_cap_field() {
        let json = capabilities_json();
        for engine in Engine::ALL {
            let row = json
                .get(engine.as_str())
                .unwrap_or_else(|| panic!("no {engine} row"));
            assert!(row.get("measured_on").and_then(|v| v.as_str()).is_some());
            for (name, _) in CAP_FIELDS {
                let v = row
                    .get(name)
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{engine}.{name} missing or not a string"));
                assert!(
                    matches!(v, "supported" | "partial" | "unsupported"),
                    "{engine}.{name} = {v:?}"
                );
            }
        }
        // Two rows that differ between the engines, so a `capabilities_json`
        // emitting a constant instead of reading the table goes red here.
        assert_eq!(json["obscura"]["js_dialogs"], "unsupported");
        assert_eq!(json["chromium"]["js_dialogs"], "supported");
        assert_eq!(json["obscura"]["file_upload"], "unsupported");
        assert_eq!(json["chromium"]["file_upload"], "supported");
        // The restored row reads both ways: chromium serves the Fetch
        // handshake, obscura is the fail-closed answer to an unprobed one.
        assert_eq!(json["chromium"]["network_interception"], "supported");
        assert_eq!(json["obscura"]["network_interception"], "unsupported");
        // The second restored row: BOTH engines serve the screencast stream
        // on measurements now (obscura flipped 2026-09-30 — its row kept
        // here so a constant-emitting `capabilities_json` still goes red on
        // the chromium side, and so a careless flip back is caught).
        assert_eq!(json["chromium"]["screencast"], "supported");
        assert_eq!(json["obscura"]["screencast"], "supported");
        // The C3 row: chromium serves the exception fold on the same-pump
        // inference; obscura ships fail-closed until the caps line probes it.
        assert_eq!(json["chromium"]["error_events"], "supported");
        assert_eq!(json["obscura"]["error_events"], "unsupported");
        // The cut rows must not reappear in the published object either — this
        // is the face the QA `caps` stage and the model both read.
        for cut in ["touch", "multi_connection"] {
            assert!(
                json["obscura"].get(cut).is_none(),
                "`{cut}` is back in the published capability object (R39)"
            );
        }
    }
}
