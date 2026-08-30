//! Embedded terminal settings — the session-grained switch for `pty.*`.
//!
//! Lives under `[policies]`, not `[gateway]`: `Config` has no `gateway`
//! field, by design (`dead_keys.rs`'s `"gateway"` entry) — that section is a
//! second parse root read by `GatewayConfig::load_default`
//! (`src/gateway/config.rs`) out of the same `config.toml`, which
//! `apply_live_sections`, `LIVE_SUBSECTIONS`, and `ConfigPatcher` (which
//! round-trips `Config` through JSON) cannot reach. A patch to `[gateway.*]`
//! would be silently dropped and reported as success. `[policies]` already
//! is "what is allowed" (`exec_tier` / `mode` / `spend` / `tool_permissions`
//! / `guardian_review`), and an embedded-terminal switch is exactly that
//! kind of predicate, not a transport setting (host / port / TLS, which
//! `[gateway]` does own).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Embedded terminal settings.
///
/// `enabled` is the session-grained gate. It is default-on because the two
/// floors below it are not optional — operator-only on both the RPC and the
/// subscribe face, and a cwd jail — and because a default-off switch on a
/// freshly wired feature makes "nobody used it" and "nobody could use it"
/// indistinguishable. It is turned off from Panel → Settings → Terminal.
///
/// Three facts a reader who only sees this struct would otherwise have to
/// discover the hard way (full story: `docs/reference/SECURITY.md`'s
/// "内嵌终端" section, `gateway::handlers::pty`'s module doc):
///
/// - Operator-only on BOTH faces — RPC (`method_admin::ADMIN_PREFIXES`) and
///   the subscribe face (`event_scope::default_rules`).
/// - The cwd jail bounds only where a session STARTS. A `cd` once inside is
///   unconstrained — a command-grained policy is not expressible over an
///   interactive byte stream (Enter, inside `vim`, is not a command). This
///   buys "every session's starting point is enumerable and auditable", not
///   "a session cannot leave the workspace".
/// - `pty.*` bypasses `[sandbox.command_policy]` and the exec tier entirely
///   — this session-grained switch is the only predicate this layer has, and
///   it is why `self_config` writes reaching this path always ask, at every
///   tier including `full` (`GATE_DECIDING_CONFIG_PATHS`).
///
/// All three fields are live — see `reload_impact::LIVE_SUBSECTIONS`'s doc
/// for the per-field liveness story, since "declared live" and "every field
/// actually applies without a restart" are not the same claim.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalConfig {
    /// The session-grained gate. Turning this off also kills live sessions
    /// (`live_apply`'s `"policies.terminal"` arm calls `PtyManager::close_all`)
    /// — a gate evaluated only at admission would leave a shell that is
    /// already open still open.
    ///
    /// A `self_config` write to this path always asks for confirmation, at
    /// every execution tier including `full` — see the struct doc above and
    /// `GATE_DECIDING_CONFIG_PATHS`.
    pub enabled: bool,
    /// Server-held scrollback per session (see `gateway::pty::screen::grid`).
    /// Applies to sessions started after a live patch; a session already
    /// running keeps the ring it was built with.
    pub scrollback_lines: u32,
    /// Concurrent session ceiling; beyond it the oldest is killed FIFO.
    /// Read fresh at spawn time (`PtyManager::set_max_sessions`), not a
    /// boot-time snapshot.
    pub max_sessions: usize,
    /// The canvas font stack. A CSS `font-family` list, applied verbatim to
    /// the terminal canvas.
    ///
    /// Default carries Nerd Font names because the shipped default before
    /// this field existed named one font most machines do not have
    /// (`'JetBrains Mono'`) and then fell back to generic `monospace`, which
    /// has NO Private Use Area glyphs — so every powerlevel10k user saw
    /// hollow boxes where their prompt icons should be. CSS fallback is
    /// per-character, so a symbols-only font listed AFTER the text font
    /// supplies the icons without changing any letterform. Segments, in
    /// order:
    ///
    /// - A handful of complete, patched Nerd Font builds -- letters AND
    ///   icons from the same font -- covering the families people actually
    ///   run `install.sh` for: `'Hack Nerd Font Mono'`, `'0xProto Nerd Font
    ///   Mono'`, `'FiraCode Nerd Font Mono'`, `'JetBrainsMono Nerd Font
    ///   Mono'`, `'MesloLGS NF'`, `'CaskaydiaCove Nerd Font Mono'`,
    ///   `'SauceCodePro Nerd Font Mono'`, `'UbuntuMono Nerd Font Mono'`.
    ///   `MesloLGS NF` is the exact font powerlevel10k's own installer
    ///   downloads (no separate `Mono` build exists under that name -- the
    ///   p10k patch is already fixed-width); every other entry names the
    ///   project's own `Mono` variant, not the plain patched build, per the
    ///   warning below. `Hack Nerd Font Mono` and `0xProto Nerd Font Mono`
    ///   are placed first because they are MEASURED, not guessed: a real
    ///   p10k bug report's installed fonts held exactly those two and none
    ///   of the other three names an earlier, shorter list tried first --
    ///   that five-entry list survived only on its LAST named font on that
    ///   exact machine. FiraCode / JetBrainsMono / MesloLGS NF /
    ///   CaskaydiaCove / SauceCodePro / UbuntuMono are NOT verified against
    ///   that machine or any other -- they are a judgement call about what
    ///   else is common, not a second measurement, and that distinction
    ///   matters: do not cite them as confirmed the way the first two are.
    /// - `'JetBrains Mono'` — the pre-existing default, unpatched. Supplies
    ///   letters (no icons) for anyone with only this installed, so the
    ///   letterform this shipped with before is unchanged for them.
    /// - `'Symbols Nerd Font Mono'` — icons only. Many people install just
    ///   this alongside an unrelated coding font; listed last among the
    ///   named fonts so it only ever donates the PUA codepoints nothing
    ///   earlier in the list has. It is what makes the default degrade
    ///   gracefully for someone whose coding font is unpatched: verified on
    ///   the same real machine above, where it was the entry every earlier
    ///   one missed.
    /// - `monospace` — universal safety net if nothing above is installed.
    ///
    /// ⚠️ Use a Nerd Font **Mono** variant. The non-Mono variants draw their
    /// icons double-width, while the server counts every Private Use Area
    /// codepoint as one column (`UnicodeWidthChar`), so the whole row shifts.
    ///
    /// ⚠️ **This list's job is to make the common case work, not to be
    /// complete.** There are roughly sixty patched Nerd Font families in
    /// circulation and this default will never hold them all -- that is
    /// what this field exists for. Do not "finish" this list toward
    /// exhaustiveness; if a font is missing, that is what setting
    /// `font_family` is for, not a reason to add a ninth named entry here.
    /// Grow the documentation of this field before you grow the list.
    pub font_family: String,
    /// Canvas font size in CSS px. Bounded on read by the Panel's
    /// `render::measure`, which clamps to `MIN_FONT_SIZE_PX`..=
    /// `MAX_FONT_SIZE_PX` before the value ever reaches the canvas — a zero
    /// or absurd value would make the cell metrics degenerate and the grid
    /// fit meaningless. (`apply_font`, one call below the clamp, is the
    /// FAMILY's check, not the size's; naming it here sent the reader to a
    /// function that does not contain the bound.)
    pub font_size_px: u32,
}

/// The pre-existing size, unchanged: bumping this default would shift every
/// existing user's row count on upgrade, which is a separate decision from
/// fixing the missing-icon default.
pub const DEFAULT_TERMINAL_FONT_SIZE_PX: u32 = 14;

/// See `TerminalConfig::font_family`'s doc for what each segment is for and
/// why this list stops where it does.
pub const DEFAULT_TERMINAL_FONT_FAMILY: &str = "'Hack Nerd Font Mono', '0xProto Nerd Font Mono', \
     'FiraCode Nerd Font Mono', 'JetBrainsMono Nerd Font Mono', 'MesloLGS NF', \
     'CaskaydiaCove Nerd Font Mono', 'SauceCodePro Nerd Font Mono', 'UbuntuMono Nerd Font Mono', \
     'JetBrains Mono', 'Symbols Nerd Font Mono', monospace";

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scrollback_lines: 1000,
            max_sessions: 64,
            font_family: DEFAULT_TERMINAL_FONT_FAMILY.to_string(),
            font_size_px: DEFAULT_TERMINAL_FONT_SIZE_PX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default-on. A default-off switch on a freshly wired feature makes
    /// "nobody used it" and "nobody could" look identical — which is exactly
    /// how pty.* stayed clientless for four rounds.
    #[test]
    fn the_terminal_is_enabled_by_default() {
        assert!(TerminalConfig::default().enabled);
        assert_eq!(TerminalConfig::default().scrollback_lines, 1000);
        assert_eq!(TerminalConfig::default().max_sessions, 64);
    }

    #[test]
    fn the_terminal_section_parses_from_toml() {
        let cfg: TerminalConfig =
            toml::from_str("enabled = false\nscrollback_lines = 200\n").expect("parse");
        assert!(!cfg.enabled);
        assert_eq!(cfg.scrollback_lines, 200);
        assert_eq!(cfg.max_sessions, 64, "unset fields keep their defaults");
    }

    /// The size default is pinned byte-for-byte: bumping it would shift row
    /// count for every existing user on upgrade, a decision this task does
    /// not make. The family default must contain a Nerd Font name — this is
    /// the assertion that keeps a future "cleanup" from quietly reverting
    /// the fix this struct exists for back to a letters-only stack.
    #[test]
    fn default_font_stack_carries_the_size_unchanged_and_a_nerd_font_name() {
        let cfg = TerminalConfig::default();
        assert_eq!(
            cfg.font_size_px, 14,
            "must match the pre-field default exactly"
        );
        assert!(
            cfg.font_family.contains("Nerd Font") || cfg.font_family.contains("MesloLGS"),
            "default font stack lost its Nerd Font entry -- p10k icons \
             regress to hollow boxes for every user who never sets this: {:?}",
            cfg.font_family
        );
        assert!(
            cfg.font_family.contains("monospace"),
            "default font stack must still end in a universal fallback: {:?}",
            cfg.font_family
        );
    }

    /// Only `font_family` set: `font_size_px` must keep its default rather
    /// than becoming Rust's own `u32::default()` (0), which downstream would
    /// make the cell metrics degenerate.
    #[test]
    fn setting_only_the_family_leaves_the_size_at_its_default() {
        let cfg: TerminalConfig =
            toml::from_str("font_family = \"'My Font', monospace\"\n").expect("parse");
        assert_eq!(cfg.font_size_px, 14);
        assert_eq!(cfg.font_family, "'My Font', monospace");
    }
}
