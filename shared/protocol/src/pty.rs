//! Wire contract for the embedded terminal.
//!
//! Both halves of this contract live in one crate on purpose: the server
//! builds its responses *from* these types rather than from `json!` literals,
//! so over-sending a field is a compile-time impossibility rather than
//! something a parse-only reconciliation test would structurally miss.

use serde::{Deserialize, Serialize};

/// The topic live screen diffs are published on. Named here so the server
/// publisher and the Panel subscriber cannot drift.
pub const PTY_SCREEN_TOPIC: &str = "pty.screen";

/// The topic a session's exit is published on.
pub const PTY_EXIT_TOPIC: &str = "pty.exit";

/// The RPC method name for the terminal's session enumeration.
///
/// For every CLIENT that calls it (the Panel's terminal view, and any face
/// added later). Deliberately NOT used at the server's own
/// `registry.register(...)` site: that scanner
/// (`gateway::method_census::sweep_rpc_methods`) requires a STRING LITERAL as
/// the first argument to add a method to its census, so passing this constant
/// there would make the registration invisible to the sweep and redden the
/// census's staleness check. Same reasoning, same shape, as
/// [`crate::runtime::RUNTIME_AGENTS_LIST_METHOD`] — see its doc for the full
/// account of why the server's literal stays independently written.
pub const PTY_LIST_METHOD: &str = "pty.list";

/// A cell (or run) colour. Deliberately *not* internally tagged: an
/// internally-tagged representation (`#[serde(tag = "k")]`) only supports
/// struct-like or unit variants, not tuple variants carrying scalars — and
/// the round-trip contract this type must honour (see the test below) only
/// requires the Rust value to survive `to_string`/`from_str`, not any
/// particular JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PtyColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl PtyColor {
    #[must_use]
    pub const fn indexed(n: u8) -> Self {
        Self::Indexed(n)
    }

    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb(r, g, b)
    }
}

/// SGR attribute bits. One byte on the wire, matching the server's `Attrs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct PtyAttrs(pub u8);

impl PtyAttrs {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    pub const REVERSE: u8 = 1 << 3;

    #[must_use]
    pub const fn has(self, bit: u8) -> bool {
        self.0 & bit == bit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyStyleRun {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub fg: PtyColor,
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub bg: PtyColor,
    #[serde(default, skip_serializing_if = "is_no_attrs")]
    pub attrs: PtyAttrs,
}

fn is_default_colour(c: &PtyColor) -> bool {
    matches!(c, PtyColor::Default)
}

fn is_no_attrs(a: &PtyAttrs) -> bool {
    a.0 == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyRowPatch {
    pub row: u16,
    pub runs: Vec<PtyStyleRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyScreenPatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<PtyRowPatch>,
    /// `(row, col)`, zero-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<(u16, u16)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_screen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// DECTCEM (`?25`). `Some` only on the frame the mode CHANGED, exactly
    /// like `alt_screen`; `None` means "unchanged", never "hidden".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_visible: Option<bool>,
    /// Bracketed paste (`?2004`), same "Some only when it changed" rule.
    ///
    /// A client that has never seen a value must NOT wrap a paste: `None` is
    /// "I do not know", and the weakest assumption is the safe one — wrapping
    /// for a program that never asked for it delivers `ESC [ 200 ~` as
    /// literal keystrokes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bracketed_paste: Option<bool>,
    /// The shell's live working directory as reported by OSC 7, same rule.
    ///
    /// Distinct from `SessionInfo::cwd`, which is where the child was
    /// SPAWNED and never changes. This one is what the shell says it is now,
    /// so a client may show it and must not treat its absence as `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bell: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One live frame. `seq` is per-session and monotonic; a client that receives
/// `seq != last + 1` has missed a frame (the gateway event bus is a bounded
/// broadcast that drops for lagging subscribers) and must re-attach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyScreenFrame {
    pub session_id: String,
    pub seq: u64,
    /// The screen's dimensions as of this frame.
    ///
    /// Beside `seq` rather than inside `patch`, and matching
    /// `PtyAttachResponse`'s shape, because geometry is a property of the
    /// frame rather than of the content delta. Carried on EVERY frame, not
    /// just the ones after a resize: sizing is smallest-wins across attached
    /// clients, so a client can be resized by someone else joining without
    /// having called anything, and there is no response it could read.
    pub rows: u16,
    pub cols: u16,
    pub patch: PtyScreenPatch,
}

/// `pty.attach` — one snapshot. Split across two calls this would open a
/// window where a client holds a screen and a different cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyAttachResponse {
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
    pub patch: PtyScreenPatch,
    pub scrollback_len: u32,
}

/// `pty.spawn`. Carries `seq` so there is no window between the spawn
/// response and the first frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySpawnResponse {
    pub session_id: String,
    pub shell: String,
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
}

/// One row of [`PtyListResponse`] — a session as every CLIENT is allowed to
/// see it.
///
/// Deliberately WITHOUT `created_by`. The server's own `SessionInfo` carries
/// that stamp because `pty.list`'s filter and the four addressed methods are
/// built on it, but no client reads it, and shipping it would tell every
/// operator which of their peers owns which shell. Because the server
/// constructs its response from this type rather than serialising its own
/// struct, re-adding that leak is a compile-time impossibility rather than
/// something a parse-only test would miss.
///
/// `cwd` is the directory the child was SPAWNED in (empty when the spawn
/// inherited the server's), not the shell's live cwd — a shell that has since
/// `cd`'d is not tracked here. The live one rides `PtyScreenPatch::cwd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySessionInfo {
    pub session_id: String,
    pub shell: String,
    /// Deserialisation-only relaxation, for the Panel shell app talking to a
    /// LAN server older than itself: a round-1 row has no `cwd`, and failing
    /// to decode the whole response would read to the terminal view as "no
    /// live sessions" and spawn a second shell beside the running one. `""` is
    /// already this field's value for "inherited the server's directory", so
    /// the default states a fact rather than inventing one. It does NOT
    /// widen the key set — `default` never omits a key on the way out.
    #[serde(default)]
    pub cwd: String,
    /// Unix-epoch SECONDS at spawn time (the runtime panel's `updated_at` is
    /// milliseconds — the two clocks are not interchangeable).
    pub created_at: i64,
    pub closed: bool,
}

/// `pty.list` — this caller's sessions, already ownership-filtered by the
/// server. One type for the three faces that used to spell this key set
/// independently: the gateway handler, the `terminal` tool and the Panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyListResponse {
    pub sessions: Vec<PtySessionInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire key set is the contract. A superset assertion would pass while
    /// the server over-sends, because serde ignores unknown keys — so this
    /// asserts equality, and derives the expectation from the type itself.
    #[test]
    fn screen_frame_wire_keys_are_exactly_these() {
        let frame = PtyScreenFrame {
            session_id: "s".into(),
            seq: 1,
            rows: 24,
            cols: 80,
            patch: PtyScreenPatch::default(),
        };
        let v = serde_json::to_value(&frame).expect("serialisable");
        let keys: std::collections::BTreeSet<&str> = v
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["patch", "seq", "session_id", "rows", "cols"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    /// Absent optional fields must not occupy wire bytes: a quiet frame is
    /// published every 16 ms per active session.
    #[test]
    fn absent_optionals_are_omitted_from_the_wire() {
        let patch = PtyScreenPatch::default();
        let v = serde_json::to_value(&patch).expect("serialisable");
        let obj = v.as_object().expect("object");
        assert!(!obj.contains_key("cursor"));
        assert!(!obj.contains_key("title"));
        assert!(!obj.contains_key("alt_screen"));
        assert!(!obj.contains_key("cursor_visible"));
        assert!(!obj.contains_key("bracketed_paste"));
        assert!(!obj.contains_key("cwd"));
        assert!(!obj.contains_key("bell"), "false bell must not ship");
    }

    /// `pty.list`'s row shape is the contract, and the KEY SET is where that
    /// contract lives. `SessionInfo::created_by` is a server-side ownership
    /// stamp with no client reader; a superset assertion would let it ship
    /// unnoticed, because serde ignores unknown keys — every client would go
    /// on parsing happily while the wire told each operator who owns which
    /// shell.
    #[test]
    fn pty_list_response_round_trips_and_pins_its_key_set() {
        let resp = PtyListResponse {
            sessions: vec![PtySessionInfo {
                session_id: "s1".into(),
                shell: "zsh".into(),
                cwd: "/tmp/work".into(),
                created_at: 1_700_000_000,
                closed: false,
            }],
        };
        let wire = serde_json::to_value(&resp).expect("serialisable");
        let back: PtyListResponse =
            serde_json::from_value(wire.clone()).expect("must survive the wire");
        assert_eq!(back, resp);

        let keys: std::collections::BTreeSet<&str> = wire["sessions"][0]
            .as_object()
            .expect("a session row is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["closed", "created_at", "cwd", "session_id", "shell"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    /// A round-1 server's `pty.list` row has no `cwd`, and the Panel shell
    /// app is shipped to talk to a LAN server that may be older than it.
    ///
    /// This row must still decode. The failure it prevents is silent, not
    /// loud: the terminal view adopts the first live session this list
    /// reports, so a whole-response decode failure reads to it as "there are
    /// no sessions" and it SPAWNS A SECOND SHELL beside the running one —
    /// exactly the outcome typing the response was meant to rule out. `""`
    /// is not a placeholder here; it is already this field's documented
    /// value for "the spawn inherited the server's directory".
    ///
    /// This is a deserialisation-only relaxation. The key set above is
    /// unaffected: `#[serde(default)]` never omits a key on the way out, so
    /// a current server still ships all five and still cannot ship a sixth.
    #[test]
    fn a_row_from_a_server_that_predates_cwd_still_decodes() {
        let old_wire = serde_json::json!({
            "sessions": [{
                "session_id": "s1",
                "shell": "zsh",
                "created_at": 1_700_000_000,
                "closed": false,
            }]
        });
        let parsed: PtyListResponse = serde_json::from_value(old_wire)
            .expect("an older server's row is not a protocol error");
        assert_eq!(
            parsed.sessions[0].cwd, "",
            "an absent cwd is the same 'inherited / not known' the empty \
             string already means, never a guess"
        );
        assert_eq!(parsed.sessions[0].session_id, "s1");
    }

    #[test]
    fn colour_round_trips_through_all_three_forms() {
        for c in [
            PtyColor::Default,
            PtyColor::Indexed(9),
            PtyColor::Rgb(1, 2, 3),
        ] {
            let s = serde_json::to_string(&c).expect("ser");
            let back: PtyColor = serde_json::from_str(&s).expect("de");
            assert_eq!(c, back, "colour must survive the wire: {s}");
        }
    }
}
