//! OSC dispatch -- `\e] <kind> ; <payload> <ST>`. Title (0/2) and the
//! ConEmu progress level (9;4) are the only kinds retained today.

use super::Performer;

/// Cap on a retained OSC payload, in chars. Same number and same reason as
/// upstream herdr's `AGENT_OSC_MAX_CHARS` (`src/pane/osc.rs`): the payload is
/// untrusted child-process output held for the lifetime of the session, so it
/// is bounded before it is stored.
pub(super) const OSC_PAYLOAD_MAX_CHARS: usize = 256;

/// The `;`-separated tail of an OSC, put back together.
///
/// One implementation for both kinds on purpose. This file used to hold the
/// right shape (in `retain_osc_progress`) and the wrong one (in the title
/// arm) side by side, each correct-looking on its own -- the same fact with
/// two expressions, where only one of them was true.
fn join_payload(parts: &[&[u8]]) -> String {
    parts
        .iter()
        .map(|p| String::from_utf8_lossy(p))
        .collect::<Vec<_>>()
        .join(";")
}

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
        let payload: String = join_payload(rest)
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
            // Rejoin, do not read `params[1]`: `vte` splits an OSC payload on
            // every `;`, so a title that contains one arrives in pieces and
            // reading the first piece truncates it. The truncation is silent
            // and the result is a plausible title, which is why it survived
            // next to `retain_osc_progress` -- the right shape of this exact
            // code -- for as long as it did.
            if params.len() > 1 {
                self.screen.state.title = Some(join_payload(&params[1..]));
            }
            return;
        }
        if *kind == b"9" {
            self.retain_osc_progress(&params[1..]);
        }
    }
}
