//! OSC dispatch -- `\e] <kind> ; <payload> <ST>`. Title (0/2) and the
//! ConEmu progress level (9;4) are the only kinds retained today.

use super::Performer;

/// Cap on a retained OSC payload, in chars. Same number and same reason as
/// upstream herdr's `AGENT_OSC_MAX_CHARS` (`src/pane/osc.rs`): the payload is
/// untrusted child-process output held for the lifetime of the session, so it
/// is bounded before it is stored.
pub(super) const OSC_PAYLOAD_MAX_CHARS: usize = 256;

impl Performer<'_> {
    /// Retain a ConEmu `OSC 9;4` progress payload.
    ///
    /// `vte` splits an OSC on `;`, so `\e]9;4;3;50\a` arrives as
    /// `["9", "4", "3", "50"]`; rejoining `params[1..]` reproduces the
    /// `"4;3;50"` form the manifests match. Rejoining rather than reading
    /// `params[1]` is also what keeps this correct if `vte` ever stops
    /// splitting -- a single `"4;3;50"` element rejoins to itself.
    ///
    /// Only `9;4` is retained. OSC 9 is a shared namespace: `9;9;<path>` is
    /// ConEmu's cwd report and a bare `9;<text>` is an iTerm2 notification.
    /// Storing those here would overwrite a live progress level with a value
    /// no rule can ever match -- turning "working" into "no evidence" with
    /// nothing on screen to show for it (判据 §8). Upstream herdr does NOT
    /// filter (`herdr src/pane/osc.rs` retains every OSC 9 payload); this
    /// divergence is deliberate, not a porting slip.
    fn retain_osc_progress(&mut self, rest: &[&[u8]]) {
        let joined = rest
            .iter()
            .map(|p| String::from_utf8_lossy(p))
            .collect::<Vec<_>>()
            .join(";");
        let payload: String = joined
            .chars()
            .filter(|ch| !ch.is_control())
            .take(OSC_PAYLOAD_MAX_CHARS)
            .collect();
        if payload == "4" || payload.starts_with("4;") {
            self.screen.state.osc_progress = Some(payload);
        }
    }

    /// The OSC table.
    pub(super) fn osc(&mut self, params: &[&[u8]]) {
        // OSC 0 = icon + title, OSC 2 = title.
        let Some(kind) = params.first() else { return };
        if matches!(*kind, b"0" | b"2") {
            if let Some(raw) = params.get(1) {
                self.screen.state.title = Some(String::from_utf8_lossy(raw).into_owned());
            }
            return;
        }
        if *kind == b"9" {
            self.retain_osc_progress(&params[1..]);
        }
    }
}
