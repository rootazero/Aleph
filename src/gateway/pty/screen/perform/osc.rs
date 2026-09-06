//! OSC dispatch -- `\e] <kind> ; <payload> <ST>`. Title (0/2) and the
//! ConEmu progress level (9;4) are the only kinds retained today.

use super::Performer;

/// Cap on a retained OSC payload, in chars. Same number and same reason as
/// upstream herdr's `AGENT_OSC_MAX_CHARS` (`src/pane/osc.rs`): the payload is
/// untrusted child-process output held for the lifetime of the session, so it
/// is bounded before it is stored.
pub(super) const OSC_PAYLOAD_MAX_CHARS: usize = 256;

/// `OSC 7`'s payload: a `file://` URI naming the shell's working directory.
///
/// Only an EMPTY host or `localhost` names this machine. Anything else
/// describes another machine's filesystem, and a path from it is not this
/// session's cwd — publishing it would be a specific lie about where the
/// terminal is, which reads as fact to everything downstream. Dropped, so
/// the next source in the chain answers instead.
///
/// A bare path with no scheme is rejected too. OSC 7 is defined as a URI;
/// treating an unrecognised payload as a path is how a scheme nobody checked
/// becomes a path somebody trusts.
fn parse_osc7_cwd(payload: &str) -> Option<String> {
    let rest = payload.strip_prefix("file://")?;
    // The first `/` starts the path, so everything before it is the host.
    // A payload with no `/` at all carries no path and is not a location.
    let split = rest.find('/')?;
    let (host, path) = rest.split_at(split);
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    // Sanitised AFTER decoding, not before: `%0A` is a control character
    // that only exists once the escape is resolved, so a filter that ran
    // first would pass it straight through. Same cap and same reason as
    // `retain_osc_progress` -- untrusted child output held for the lifetime
    // of the session is bounded before it is stored.
    let decoded: String = percent_decode(path)
        .chars()
        .filter(|c| !c.is_control())
        .take(OSC_PAYLOAD_MAX_CHARS)
        .collect();
    (!decoded.is_empty()).then(|| strip_uri_drive_slash(&decoded))
}

/// `/C:/Users/x` -> `C:/Users/x`.
///
/// RFC 8089 spells a Windows path in a `file:` URI with the drive letter INSIDE
/// the path component, so a correct emitter sends `file:///C:/Users/x` and the
/// host/path split above hands back a leading `/` that belongs to the URI and
/// not to the path. Every Windows terminal that emits OSC 7 sends that form, and
/// `/C:/Users/x` is a path no Windows API accepts — so before 2026-09-05 the
/// live cwd of any Windows session that reported one was unusable.
///
/// NOT `#[cfg(windows)]`, and that is deliberate twice over: this function reads
/// a URI, which is a platform-independent spelling (a Unix Aleph attached to a
/// remote Windows shell sees the same bytes), and a branch that compiles on no
/// machine the developer runs is a branch nobody can falsify — the same
/// reasoning `foreground::foreground_fact_for_shell` records.
///
/// The test is narrow on purpose: `/` + one ASCII letter + `:` + a separator.
/// A Unix directory literally called `/C:` exists in principle, and requiring
/// the separator is what keeps this from renaming it.
fn strip_uri_drive_slash(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 4
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && (bytes[3] == b'/' || bytes[3] == b'\\')
    {
        return path[1..].to_owned();
    }
    path.to_owned()
}

/// Percent-decoding, bytes first.
///
/// `%E4%B8%AD` is ONE character written as three escapes, so decoding has to
/// accumulate bytes and convert once at the end; resolving each escape to a
/// `char` on its own turns every non-ASCII path into mojibake. A `%` that
/// does not begin a valid escape is kept literally — it is a legal character
/// in a path, and dropping it would silently rename a directory.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = hex_pair(bytes[i + 1], bytes[i + 2]) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(hi: u8, lo: u8) -> Option<u8> {
    let digit = |c: u8| char::from(c).to_digit(16).map(|v| v as u8);
    Some(digit(hi)? * 16 + digit(lo)?)
}

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
            return;
        }
        if *kind == b"7" && params.len() > 1 {
            // Only a URI this machine can vouch for replaces the cwd. A
            // rejected one leaves the previous answer standing: a value that
            // failed its check has the standing to say "I don't know", never
            // to supply an answer (判据 §8).
            if let Some(path) = parse_osc7_cwd(&join_payload(&params[1..])) {
                self.screen.state.cwd = Some(path);
            }
        }
    }
}
