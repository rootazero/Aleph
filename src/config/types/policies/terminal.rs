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
    pub enabled: bool,
    /// Server-held scrollback per session (see `gateway::pty::screen::grid`).
    /// Applies to sessions started after a live patch; a session already
    /// running keeps the ring it was built with.
    pub scrollback_lines: u32,
    /// Concurrent session ceiling; beyond it the oldest is killed FIFO.
    /// Read fresh at spawn time (`PtyManager::set_max_sessions`), not a
    /// boot-time snapshot.
    pub max_sessions: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scrollback_lines: 1000,
            max_sessions: 64,
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
}
