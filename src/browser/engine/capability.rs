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
/// with no client is not delivered. Four rows were cut on exactly that test:
/// `touch` (no `browser_tap`; nothing dispatches `Input.dispatchTouchEvent`),
/// `screencast` (nothing calls `Page.startScreencast` at HEAD or in Tasks
/// 12-19), `network_interception` (spec §7.2 defers `Fetch.*`; `rg "Fetch\."
/// src/ crates/` at HEAD returns nothing), and `multi_connection` (an
/// architectural property, not a verb — it belongs in the live-view design
/// notes, not in a table the model reads as a menu).
pub struct EngineCapabilities {
    /// `browser_dialog` — `Page.handleJavaScriptDialog`, plus the
    /// `Page.javascriptDialogOpening` event the backend tracks to know a
    /// dialog is open at all.
    pub js_dialogs: Cap,
    /// `browser_drag`.
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
    /// The build each row above was measured against. A capability claim with
    /// no measurement date is a list that expires without telling anyone.
    pub measured_on: &'static str,
}

/// The field list every consumer walks: the prose, the JSON, and the QA
/// prober. A field added to the struct and forgotten here would silently stop
/// being described, serialised or falsified, so Task 16's
/// `cap_fields_covers_every_cap_typed_field_of_the_struct` derives the expected
/// set from this file's own source (判据 §3).
pub const CAP_FIELDS: [(&str, fn(&EngineCapabilities) -> Cap); 5] = [
    ("js_dialogs", |c| c.js_dialogs),
    ("drag", |c| c.drag),
    ("file_upload", |c| c.file_upload),
    ("pdf", |c| c.pdf),
    ("insert_text", |c| c.insert_text),
];

static OBSCURA: EngineCapabilities = EngineCapabilities {
    // Neither `Page.javascriptDialogOpening` nor `Page.handleJavaScriptDialog`
    // exists anywhere in `obscura-cdp`: a page that opens an alert simply
    // never reports one.
    js_dialogs: Cap::Unsupported,
    // **Unsupported as the fail-closed answer to an unknown, not as a
    // measurement.** `browser_drag` does NOT send `Input.dispatchDragEvent`:
    // `cdp_backend::actions::drag` sends six `Input.dispatchMouseEvent`s
    // (move, press, three interpolated moves, release), which obscura answers
    // `ok`. Task 0 probed `Input.dispatchDragEvent` — absent on obscura, and a
    // report-success-no-op on Chrome — but that is a method no Aleph verb
    // dispatches, so it certifies nothing either way about this row.
    //
    // What is actually unknown is whether obscura delivers a synthetic mouse
    // sequence to a page's own drag listeners. Nobody has measured it. An
    // unknown may not be spent as a permission (判据 §8), so the row stays
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
    measured_on: "v0.2.2",
};

static CHROMIUM: EngineCapabilities = EngineCapabilities {
    js_dialogs: Cap::Supported,
    drag: Cap::Supported,
    file_upload: Cap::Supported,
    pdf: Cap::Supported,
    insert_text: Cap::Supported,
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
        "Browser engines. obscura is the default; chromium is the escape hatch, reached with \
         browser_session{action:\"switch_engine\", engine:\"chromium\"} (cookies and open URLs \
         move across; scroll position and page state do not).\n",
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

    /// The four rows R39 cut, asserted as ABSENT.
    ///
    /// A cut that is only recorded in a comment comes back: the next reader
    /// measures obscura's screencast, finds it works, and adds the row without
    /// noticing that no verb reaches it. The table is a menu the model reads
    /// (`browser_session{action:"capabilities"}`), so a row it cannot order is
    /// worse than a missing one — 判据 §9, and 判据 §17's "point at the line
    /// that renders it" applied to a capability instead of a UI string.
    #[test]
    fn no_capability_row_exists_without_a_verb_that_dispatches_it() {
        let src = include_str!("capability.rs");
        for (cut, why) in [
            (
                "touch",
                "no browser_tap verb; nothing dispatches Input.dispatchTouchEvent",
            ),
            (
                "screencast",
                "nothing calls Page.startScreencast at HEAD or in Tasks 12-19",
            ),
            (
                "network_interception",
                "spec §7.2 defers Fetch.*; no verb enables it",
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
    /// only copy in the repository.
    #[test]
    fn capability_table_agrees_with_the_t0_support_matrix() {
        const MATRIX: &str = include_str!("fixtures/t0-support-matrix.json");

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
            /// unconditionally: obscura answers `Page.javascriptDialogOpening`
            /// with a stated absence and `effect: null`, and demanding an
            /// effect reading for a call that never succeeded would be a 恒红
            /// arm — satisfiable only by inventing a measurement (判据 §2).
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

        /// Rows Task 0's probe does not settle, each with the reason.
        ///
        /// **`drag` is here because the label and the verb are different
        /// methods.** Task 0 probed `Input.dispatchDragEvent`;
        /// `cdp_backend::actions::drag` sends `Input.dispatchMouseEvent` six
        /// times and never sends `dispatchDragEvent` at all. Judging the row on
        /// that label would be 判据 §12 — deriving a decision in one space and
        /// applying it in another — and it would have flipped Chromium's row to
        /// `Unsupported` on a measurement of a method Aleph does not use. The
        /// method the verb DOES send is `ok` on both engines with no effect
        /// reading, i.e. unmeasured, so the row is decided by the fail-closed
        /// rule stated beside it in `OBSCURA` instead.
        const NOT_PROBED: [(&str, &str); 1] = [(
            "drag",
            "T0's Input.dispatchDragEvent label measures a method no browser_* verb \
             dispatches; browser_drag sends Input.dispatchMouseEvent, whose effect T0 \
             did not read on either engine",
        )];

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
            // Mapped here, in the reader, because the matrix is that task's
            // output: rewriting somebody else's measurement to match our
            // vocabulary is how a fixture stops being evidence (判据 §10 — the
            // wire shape belongs to whoever produced it).
            let key = match engine {
                Engine::Chromium => "chrome",
                Engine::Obscura => engine.as_str(),
            };
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
        // The cut rows must not reappear in the published object either — this
        // is the face the QA `caps` stage and the model both read.
        for cut in [
            "touch",
            "screencast",
            "network_interception",
            "multi_connection",
        ] {
            assert!(
                json["obscura"].get(cut).is_none(),
                "`{cut}` is back in the published capability object (R39)"
            );
        }
    }
}
