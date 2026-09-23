//! Boot-time activation gate: after every dependency has had its chance to
//! report, which plugins are still not in a terminal state, and what are
//! they waiting for.
//!
//! The dsh counterpart is `assertEntriesActivated` (evidence
//! `scan-dsh-cordis.md` §5.1, Top-8 #2): a PENDING unit names the services it
//! is still missing and boot fails. Aleph is a daemon, so the default posture
//! is one log line per plugin plus the `extension/plugins-activated` doctor
//! check; `ALEPH_ACTIVATION_GATE=fatal` (QA / test profiles) turns the same
//! report into a refusal to keep running.
//!
//! "Terminal" is [`PluginStatus::is_terminal`] — an exhaustive match on the
//! enum, so this gate cannot fall out of step with a new variant.

use crate::extension::registry::PluginRegistry;
use crate::extension::types::PluginStatus;

/// Every plugin that did not reach a terminal state, with what it waits for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ActivationReport {
    pub non_terminal: Vec<(String, Vec<String>)>,
}

impl ActivationReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.non_terminal.is_empty()
    }

    /// One operator-readable line per plugin, e.g.
    /// `plugin `x` did not activate: waiting on mcp:plugin:x/srv`.
    #[must_use]
    pub fn render_lines(&self) -> Vec<String> {
        self.non_terminal
            .iter()
            .map(|(id, waiting)| {
                format!(
                    "plugin `{id}` did not activate: waiting on {}",
                    waiting.join(", ")
                )
            })
            .collect()
    }
}

/// Sweep the registry. Sorted by id so two boots render identically.
#[must_use]
pub fn assess(registry: &PluginRegistry) -> ActivationReport {
    let mut non_terminal: Vec<(String, Vec<String>)> = registry
        .list_plugins()
        .into_iter()
        .filter(|p| !p.status.is_terminal())
        .map(|p| {
            let waiting = match &p.status {
                PluginStatus::Pending { waiting_on } => waiting_on.clone(),
                // `is_terminal` is false only for Pending today; if a second
                // non-terminal variant appears, `is_terminal`'s exhaustive
                // match forces this arm to be revisited too.
                _ => Vec::new(),
            };
            (p.id.clone(), waiting)
        })
        .collect();
    non_terminal.sort();
    ActivationReport { non_terminal }
}

/// What boot does with a non-clean report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePosture {
    /// One `warn!` line per plugin; the daemon keeps running (default).
    Log,
    /// `error!` + exit 78: a QA fixture that boots with a plugin stuck
    /// pending has found the thing this gate exists to find.
    Fatal,
}

/// Classify a raw `ALEPH_ACTIVATION_GATE` value, trimmed and compared
/// case-insensitively: `Some(posture)` for the two recognised words,
/// `None` for anything else — including `None` itself (unset), the empty
/// string, and a typo (`"Fatel"`, `"1"`, `"yes"`).
///
/// Split out from [`posture_from_env`] so recognition is a pure, testable
/// predicate, separate from what a caller does when it comes back `None`
/// (the boot call site logs it; this function stays free of I/O so a test
/// can assert on the classification directly rather than on log output).
#[must_use]
pub fn classify_env_value(value: Option<&str>) -> Option<GatePosture> {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("fatal") => Some(GatePosture::Fatal),
        Some("log") => Some(GatePosture::Log),
        _ => None,
    }
}

/// `ALEPH_ACTIVATION_GATE`: only the word `fatal` (any case) is fatal;
/// anything else, including a typo, is `Log` — a misspelling must not be
/// able to kill a daemon. An unrecognised value (as opposed to unset or
/// the word `log`) is logged by the caller — see the boot call site in
/// `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, which
/// calls [`classify_env_value`] itself to tell "unset"/"log" apart from a
/// typo before falling back to this function's default.
#[must_use]
pub fn posture_from_env(value: Option<&str>) -> GatePosture {
    classify_env_value(value).unwrap_or(GatePosture::Log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{PluginKind, PluginOrigin, PluginRecord, PluginRegistry, PluginStatus};

    fn reg_with(statuses: &[(&str, PluginStatus)]) -> PluginRegistry {
        let mut r = PluginRegistry::new();
        for (id, s) in statuses {
            let mut rec = PluginRecord::new(
                (*id).into(),
                (*id).into(),
                PluginKind::Mcp,
                PluginOrigin::Global,
            );
            rec.status = s.clone();
            r.register_plugin(rec);
        }
        r
    }

    #[test]
    fn assess_lists_only_non_terminal_plugins_with_their_dependencies() {
        let r = reg_with(&[
            ("ok", PluginStatus::Loaded),
            ("off", PluginStatus::Disabled),
            ("bad", PluginStatus::Error("x".into())),
            ("no", PluginStatus::Blocked("policy".into())),
            (
                "wait",
                PluginStatus::Pending {
                    waiting_on: vec!["mcp:plugin:wait/srv".into()],
                },
            ),
        ]);
        let report = assess(&r);
        assert!(!report.is_clean());
        assert_eq!(
            report.non_terminal,
            vec![("wait".to_string(), vec!["mcp:plugin:wait/srv".to_string()])]
        );
        let lines = report.render_lines();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("wait") && lines[0].contains("mcp:plugin:wait/srv"),
            "{lines:?}"
        );
    }

    #[test]
    fn assess_is_clean_when_every_plugin_is_terminal() {
        let r = reg_with(&[
            ("ok", PluginStatus::Loaded),
            ("bad", PluginStatus::Error("x".into())),
        ]);
        assert!(assess(&r).is_clean());
        assert!(assess(&PluginRegistry::new()).is_clean());
    }

    #[test]
    fn posture_defaults_to_log_and_only_the_word_fatal_is_fatal() {
        assert!(matches!(posture_from_env(None), GatePosture::Log));
        assert!(matches!(posture_from_env(Some("log")), GatePosture::Log));
        assert!(matches!(
            posture_from_env(Some("fatal")),
            GatePosture::Fatal
        ));
        assert!(matches!(
            posture_from_env(Some("FATAL")),
            GatePosture::Fatal
        ));
        // An unrecognised value must not silently become fatal — nor silently
        // become "log": it is logged as unrecognised by the caller; here it
        // maps to Log so a typo cannot kill a daemon.
        assert!(matches!(posture_from_env(Some("yes")), GatePosture::Log));
    }

    /// `posture_from_env`'s default (previous test) hides WHETHER a value
    /// was recognised behind the single `Log` result it shares with "unset"
    /// and the word `log` itself — the caller needs to tell those apart to
    /// log an unrecognised value instead of swallowing it silently.
    /// `classify_env_value` is the pure predicate that makes that
    /// distinguishable and testable without capturing log output.
    #[test]
    fn classify_env_value_tells_unrecognised_apart_from_unset_and_log() {
        assert_eq!(classify_env_value(None), None);
        assert_eq!(classify_env_value(Some("log")), Some(GatePosture::Log));
        assert_eq!(classify_env_value(Some("LOG")), Some(GatePosture::Log));
        assert_eq!(classify_env_value(Some("fatal")), Some(GatePosture::Fatal));
        assert_eq!(
            classify_env_value(Some(" Fatal  ")),
            Some(GatePosture::Fatal)
        );
        // The unrecognised arm: a typo, a stray truthy-looking value, and
        // the empty string are all `None` — never coerced into either
        // recognised posture.
        for bogus in ["Fatel", "1", "yes", "true", ""] {
            assert_eq!(
                classify_env_value(Some(bogus)),
                None,
                "{bogus:?} must classify as unrecognised, not silently default"
            );
        }
    }
}
