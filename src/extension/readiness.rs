//! Readiness: which terminal state a mounted plugin has reached, derived from
//! what its declared dependencies reported.
//!
//! dsh's `assertEntriesActivated` (evidence `scan-dsh-cordis.md` §5.1) is the
//! model: a unit that is neither ACTIVE nor FAILED must be able to NAME what
//! it is waiting for, and that name must come from a declaration, not a
//! boolean somebody set. Here the declarations are the plugin's `.mcp.json`
//! servers; each one's dependency is the MCP manager (attached or not) and
//! the actor's answer to its start request (`add_transient_server_detached`'s
//! receiver, P1.4). The answer is the report; nothing is polled.
//!
//! This module is pure: every input is a value, so the rules are tested
//! without a manager, a process or a clock. The two writers of the result —
//! `lifecycle.rs::mount_parsed` (the no-handle case) and
//! `lifecycle.rs::watch_server_starts` (before and after the receivers settle)
//! — both go through [`derive_readiness`] and `ExtensionManager::write_readiness`;
//! nothing else assigns `PluginStatus::Pending`.

use crate::extension::types::PluginStatus;

/// What one declared server's start request reported so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStart {
    /// The actor answered `Ok`: handshake done, tools listed.
    Started,
    /// The actor answered `Err`: spawn / handshake failed, with its reason.
    Failed(String),
    /// No answer yet, or the sender was dropped. "I don't know" — a wait,
    /// never a failure (判据 §8).
    Unanswered,
}

/// Everything the derivation needs, as values.
pub struct ReadinessInputs<'a> {
    /// The `mcp_server` step ran. `false` when `mount` recorded
    /// `scope.skip("mcp_server", …)` because no MCP handle was attached.
    pub manager_attached: bool,
    /// `(server_id, report)` for every server the mount enqueued.
    pub servers: &'a [(String, ServerStart)],
}

/// The terminal (or not) state of one MCP-kind plugin. See the module doc
/// for the rules; each is a row of the tests below.
#[must_use]
pub fn derive_readiness(i: &ReadinessInputs<'_>) -> PluginStatus {
    if !i.manager_attached {
        return PluginStatus::Pending {
            waiting_on: vec!["mcp:manager".into()],
        };
    }
    let mut waiting: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for (server_id, report) in i.servers {
        // Exhaustive on purpose: a new report word must be placed here.
        match report {
            ServerStart::Started => {}
            ServerStart::Unanswered => waiting.push(format!("mcp:{server_id}")),
            ServerStart::Failed(e) => failed.push(format!("mcp:{server_id}: {e}")),
        }
    }
    if !failed.is_empty() {
        failed.sort();
        return PluginStatus::Error(failed.join("; "));
    }
    if !waiting.is_empty() {
        waiting.sort();
        waiting.dedup();
        return PluginStatus::Pending {
            waiting_on: waiting,
        };
    }
    PluginStatus::Loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(servers: &[(String, ServerStart)]) -> ReadinessInputs<'_> {
        ReadinessInputs {
            manager_attached: true,
            servers,
        }
    }

    #[test]
    fn no_manager_is_pending_on_the_manager_regardless_of_servers() {
        let servers = vec![("plugin:p/a".to_string(), ServerStart::Started)];
        let mut i = inputs(&servers);
        i.manager_attached = false;
        assert_eq!(
            derive_readiness(&i),
            PluginStatus::Pending {
                waiting_on: vec!["mcp:manager".into()]
            }
        );
    }

    #[test]
    fn every_report_word_is_classified() {
        let cases = [
            (ServerStart::Unanswered, Some("mcp:plugin:p/a")),
            (ServerStart::Started, None),
        ];
        for (report, expect_wait) in cases {
            let servers = vec![("plugin:p/a".to_string(), report)];
            let got = derive_readiness(&inputs(&servers));
            match expect_wait {
                Some(w) => assert_eq!(
                    got,
                    PluginStatus::Pending {
                        waiting_on: vec![w.into()]
                    }
                ),
                None => assert_eq!(got, PluginStatus::Loaded),
            }
        }
        let servers = vec![(
            "plugin:p/a".to_string(),
            ServerStart::Failed("spawn: ENOENT".into()),
        )];
        match derive_readiness(&inputs(&servers)) {
            PluginStatus::Error(e) => {
                assert!(e.contains("plugin:p/a") && e.contains("ENOENT"), "{e}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failure_outranks_a_wait_and_names_only_the_failed_server() {
        let servers = vec![
            (
                "plugin:p/a".to_string(),
                ServerStart::Failed("ENOENT".into()),
            ),
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
            ("plugin:p/c".to_string(), ServerStart::Started),
        ];
        match derive_readiness(&inputs(&servers)) {
            PluginStatus::Error(e) => {
                assert!(e.contains("plugin:p/a"), "{e}");
                assert!(!e.contains("plugin:p/b"), "a wait is not a failure: {e}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn waiting_on_is_sorted_and_deduplicated() {
        let servers = vec![
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
            ("plugin:p/a".to_string(), ServerStart::Unanswered),
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
        ];
        assert_eq!(
            derive_readiness(&inputs(&servers)),
            PluginStatus::Pending {
                waiting_on: vec!["mcp:plugin:p/a".into(), "mcp:plugin:p/b".into()]
            }
        );
    }

    #[test]
    fn no_servers_and_a_manager_is_loaded() {
        assert_eq!(derive_readiness(&inputs(&[])), PluginStatus::Loaded);
    }
}
