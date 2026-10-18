//! Durable rewrite authorization and original model-call accounting.
//! This target owns one process-wide ledger install; keep both cases in one test.

#![cfg(unix)]

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use alephcore::extension::hooks::HookExecutor;
use alephcore::extension::visibility::ScopeKey;
use alephcore::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
use alephcore::gateway::security::shared_token::SharedTokenManager;
use alephcore::gateway::security::store::SecurityStore;
use alephcore::identity::{self, AgentKeystore, AgentLedger, LedgerAction, LedgerOutcome};
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::exec_approval::{
    grant_fingerprint, ApprovalAction, ApprovalOutcome, ApprovalRequester, ApprovalResponse,
};
use alephcore::sync_primitives::Arc;
use alephcore::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
use alephcore::tools::scoped::ScopedToolService;
use alephcore::tools::service::{ToolError, ToolService};
use alephcore::tools::turn_context::TurnContext;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct RecordingTool {
    calls: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl LoopTool for RecordingTool {
    fn name(&self) -> &str {
        "cron_manage"
    }

    fn description(&self) -> &str {
        "Task11-F records the actual handler effect without changing configuration"
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(input.clone());
        ToolResult::Success { output: input }
    }
}

struct RecordingRequester {
    answers: Mutex<VecDeque<ApprovalOutcome>>,
    seen: Mutex<Vec<ApprovalAction>>,
}

#[async_trait::async_trait]
impl ApprovalRequester for RecordingRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        self.seen.lock().unwrap().push(action.clone());
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("only the original and final approval cards are expected")
            .into()
    }
}

#[tokio::test]
async fn rewritten_calls_persist_distinct_authorizations_and_original_call_accounting() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("security.db");
    let store = Arc::new(SecurityStore::open(&db).unwrap());
    let vault = Arc::new(SharedTokenManager::new(
        store.clone(),
        dir.path().join("task11.vault"),
    ));
    vault.generate_token().expect("temporary vault master key");
    let ledger = Arc::new(AgentLedger::new(Arc::new(AgentKeystore::new(store, vault))));
    let _writer = identity::install(ledger.clone())
        .expect("this dedicated integration target installs the ledger once");

    let original = json!({"operation": "list", "job": "original"});
    let final_input = json!({"operation": "delete", "job": "final"});
    let original_fp = grant_fingerprint("cron_manage", &original);
    let final_fp = grant_fingerprint("cron_manage", &final_input);
    assert_ne!(original_fp, final_fp);

    for (agent, final_approved) in [
        ("task11-f-final-approved", true),
        ("task11-f-final-denied", false),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(RecordingTool {
            calls: calls.clone(),
            inputs: inputs.clone(),
        }));
        let requester = Arc::new(RecordingRequester {
            answers: Mutex::new(VecDeque::from([
                ApprovalOutcome::Approved,
                if final_approved {
                    ApprovalOutcome::Approved
                } else {
                    ApprovalOutcome::Denied
                },
            ])),
            seen: Mutex::new(Vec::new()),
        });

        // Legacy manual assembly isolates dispatch/audit from hook admission.
        // The command is static: no model input is interpolated into a shell.
        let hook = HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::Normal,
            matcher: Some("^cron_manage$".to_string()),
            actions: vec![HookAction::Command {
                command: r#"printf '%s\n' 'update_input: {"operation":"delete","job":"final"}'"#
                    .to_string(),
            }],
            plugin_name: "task11-f".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
            declared_event: None,
            scope_key: ScopeKey::Global,
        };
        let svc = ScopedToolService::new(Arc::new(registry), BTreeSet::new())
            .with_turn_context(TurnContext {
                session_key: SessionKey::main(agent),
                run_id: String::new(),
                channel_id: String::new(),
                conversation_id: String::new(),
                caller_role: Some("guest".to_string()),
                channel_tool_permissions: None,
                unattended: false,
                plan_gate: None,
                side_question: false,
                telecom_authority: None,
            })
            .with_config_approval(requester.clone())
            .with_hook_executor(Arc::new(HookExecutor::new(vec![hook])), agent);

        let (result, effective) = svc
            .execute_with_cancel_effective(
                "cron_manage",
                original.clone(),
                CancellationToken::new(),
            )
            .await;
        if final_approved {
            result.expect("both approvals allow the rewritten call to finish successfully");
            assert_eq!(effective, Some(final_input.clone()));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                inputs.lock().unwrap().as_slice(),
                std::slice::from_ref(&final_input)
            );
        } else {
            match result {
                Err(ToolError::PermissionDenied { name, reason }) => {
                    assert_eq!(name, "cron_manage");
                    assert!(
                        reason.contains("configuration") && reason.contains("operator"),
                        "expected the config operator gate: {reason}"
                    );
                }
                other => panic!("final denial must be a typed operator refusal: {other:?}"),
            }
            assert_eq!(effective, None);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(inputs.lock().unwrap().is_empty());
        }
        {
            let seen = requester.seen.lock().unwrap();
            assert_eq!(seen.len(), 2, "each distinct input needs its own real card");
            assert_eq!(seen[0].tool_name, "cron_manage");
            assert_eq!(seen[1].tool_name, "cron_manage");
            assert_eq!(seen[0].grant_key.as_deref(), Some(original_fp.as_str()));
            assert_eq!(seen[1].grant_key.as_deref(), Some(final_fp.as_str()));
            assert_ne!(seen[0].grant_key, seen[1].grant_key);
            assert_ne!(seen[0].summary, seen[1].summary);
            assert!(seen[0].summary.contains("job=original"));
            assert!(seen[0].summary.contains("operation=list"));
            assert!(seen[1].summary.contains("job=final"));
            assert!(seen[1].summary.contains("operation=delete"));
            assert!(requester.answers.lock().unwrap().is_empty());
        }

        // FIFO barrier, then a second DB connection: no guessed sleeps or
        // captured/simulated rows can stand in for the durable writer output.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), identity::flush())
                .await
                .expect("signed ledger FIFO flush completes within five seconds")
        );
        let reopened = SecurityStore::open(&db).expect("reopen the on-disk security DB");
        let chain = reopened
            .ledger_chain(agent)
            .expect("persisted chain readable");
        let mut expected = vec![
            (
                LedgerAction::ApprovalGranted,
                LedgerOutcome::Ok,
                Some(original_fp.as_str()),
            ),
            (
                if final_approved {
                    LedgerAction::ApprovalGranted
                } else {
                    LedgerAction::ApprovalDenied
                },
                if final_approved {
                    LedgerOutcome::Ok
                } else {
                    LedgerOutcome::Denied
                },
                Some(final_fp.as_str()),
            ),
        ];
        if final_approved {
            // Existing accounting names the original model action even though
            // the handler ran final_input. Never attribute ToolCall to final_fp.
            expected.push((
                LedgerAction::ToolCall,
                LedgerOutcome::Ok,
                Some(original_fp.as_str()),
            ));
        }
        assert_eq!(
            chain.len(),
            expected.len() + 1,
            "exact chain size for {agent}"
        );
        assert_eq!(chain[0].action, LedgerAction::IdentityCreated);
        assert_eq!(chain[0].outcome, LedgerOutcome::Ok);
        assert_eq!(chain[0].args_fp, None);
        for (index, row) in chain.iter().enumerate() {
            assert_eq!(row.agent_id, agent);
            assert_eq!(row.seq, index as i64 + 1);
        }
        let tool_rows: Vec<_> = chain
            .iter()
            .filter(|row| {
                row.target == "cron_manage"
                    && matches!(
                        row.action,
                        LedgerAction::ApprovalGranted
                            | LedgerAction::ApprovalDenied
                            | LedgerAction::ToolCall
                            | LedgerAction::ToolDenied
                    )
            })
            .collect();
        assert_eq!(
            tool_rows.len(),
            expected.len(),
            "no missing or extra tool rows"
        );
        let actual: Vec<_> = tool_rows
            .iter()
            .map(|row| (row.action, row.outcome, row.args_fp.as_deref()))
            .collect();
        assert_eq!(
            actual, expected,
            "durable enqueue order and fingerprints for {agent}"
        );
        let tool_calls: Vec<_> = chain
            .iter()
            .filter(|row| row.action == LedgerAction::ToolCall)
            .collect();
        assert_eq!(tool_calls.len(), usize::from(final_approved));
        assert!(!tool_calls
            .iter()
            .any(|row| row.args_fp.as_deref() == Some(final_fp.as_str())));
        let report = ledger.verify(agent).expect("verify the real signed chain");
        assert!(report.ok, "{agent}: {:?}", report.faults);
    }
    // _writer remains owned until both scenarios and durable verifications finish.
}
