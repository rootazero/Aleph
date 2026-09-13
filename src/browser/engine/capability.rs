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
    // `Input.dispatchDragEvent`: no arm in `domains/input.rs`.
    drag: Cap::Unsupported,
    // `DOM.setFileInputFiles` is a real implementation (`domains/dom.rs:257`),
    // and it is the route `browser_upload` takes. (The OTHER upload route,
    // `Page.setInterceptFileChooserDialog`, is a NOOP at
    // `domains/page.rs:1382` — the CDP backend sets the input's files directly
    // and never waits for a chooser event.)
    file_upload: Cap::Supported,
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
}
