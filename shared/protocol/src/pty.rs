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
            ["patch", "seq", "session_id"]
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
        assert!(!obj.contains_key("bell"), "false bell must not ship");
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
