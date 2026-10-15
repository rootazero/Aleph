//! Actual builtin registry -> operator approval -> fake relay -> isolated Core.
//! Reuses the authenticated relay rig; no production account or network I/O.
use super::*;
use crate::approval::operator_requester::OperatorApprovalRequester;
use crate::exec::manager::ExecApprovalManager;
use crate::exec::ApprovalDecisionType;
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::telecom::lifecycle_tools::{
    LifecycleBinding, LifecycleEffectWitness, LifecyclePeer, LifecyclePeerError,
    LifecyclePeerOutcome, LifecycleWiring, TelecomLifecycleService,
};
use crate::gateway::{
    agent_instance::{AgentInstance, AgentRegistry},
    context::GatewayContext,
    event_bus::GatewayEventBus,
    event_emitter::EventEmitter,
    execution_adapter::ExecutionAdapter,
    execution_engine::{ExecutionError, RunRequest, RunStatus},
    inter_agent_policy::AgentToAgentPolicy,
    session_store::{
        sqlite_backend::{SqliteSessionStore, SqliteSessionStoreConfig},
        SessionStore,
    },
};
use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

struct NoOwnerAdapter;
#[async_trait::async_trait]
impl ExecutionAdapter for NoOwnerAdapter {
    async fn execute(
        &self,
        _: RunRequest,
        _: Arc<AgentInstance>,
        _: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        panic!("ordinary owner execution must not be entered");
    }
    async fn cancel(&self, id: &str) -> Result<(), ExecutionError> {
        Err(ExecutionError::RunNotFound(id.into()))
    }
    async fn get_status(&self, _: &str) -> Option<RunStatus> {
        None
    }
    async fn active_run_count(&self) -> usize {
        0
    }
}
struct DialPeer {
    effects: AtomicUsize,
    unknown: AtomicBool,
    block: AtomicBool,
    entered: Notify,
    gate: Notify,
}
#[async_trait::async_trait]
impl LifecyclePeer for DialPeer {
    async fn initiate(
        &self,
        w: &LifecycleEffectWitness,
    ) -> Result<LifecyclePeerOutcome, LifecyclePeerError> {
        self.entered.notify_one();
        if self.block.load(Ordering::Acquire) {
            self.gate.notified().await;
        }
        w.claim_effect()?;
        self.effects.fetch_add(1, Ordering::AcqRel);
        Ok(if self.unknown.load(Ordering::Acquire) {
            LifecyclePeerOutcome::Unknown
        } else {
            LifecyclePeerOutcome::Known("provider-call".into())
        })
    }
    async fn answer(
        &self,
        w: &LifecycleEffectWitness,
    ) -> Result<LifecyclePeerOutcome, LifecyclePeerError> {
        let id = w.call().unwrap().session().provider_call_id.clone();
        w.claim_effect()?;
        self.effects.fetch_add(1, Ordering::AcqRel);
        Ok(LifecyclePeerOutcome::Known(id))
    }
    async fn hangup(&self, call: &VoiceCallLease) -> Result<(), LifecyclePeerError> {
        call.check(&CancellationToken::new())
            .map_err(|_| LifecyclePeerError)
    }
}
struct Fixture {
    // Drop last: builtin constructors open stores under this isolated home.
    registry: BuiltinToolRegistry,
    service: Arc<TelecomLifecycleService>,
    budget: Arc<PublicRunRegistry>,
    dial: Arc<DialPeer>,
    manager: Arc<ExecApprovalManager>,
    events: broadcast::Receiver<GatewayEventFrame>,
    _home: crate::utils::paths::IsolatedAlephHome,
}
impl Fixture {
    async fn new(rig: &Rig, runner: Arc<Runner>, history: Option<Arc<PublicHistoryStore>>) -> Self {
        let home = crate::utils::paths::IsolatedAlephHome::new();
        let budget = PublicRunRegistry::new(4, 1000);
        let manager = Arc::new(ExecApprovalManager::new());
        let bus = Arc::new(GatewayEventBus::new());
        let events = bus.subscribe_typed();
        let dial = Arc::new(DialPeer {
            effects: AtomicUsize::new(0),
            unknown: AtomicBool::new(false),
            block: AtomicBool::new(false),
            entered: Notify::new(),
            gate: Notify::new(),
        });
        let service = TelecomLifecycleService::new(
            BTreeMap::from([
                ("user".into(), BTreeSet::from(["bind".into()])),
                ("other".into(), BTreeSet::from(["foreign".into()])),
            ]),
            BTreeMap::from([(
                "bind".into(),
                LifecycleBinding {
                    transport: rig.relay.clone(),
                },
            )]),
            dial.clone(),
            LifecycleWiring {
                registry: budget.clone(),
                runner,
                history,
                operator: Arc::new(OperatorApprovalRequester::for_config_tier(
                    manager.clone(),
                    bus,
                )),
                core_target: "test-core".into(),
            },
        );
        let sessions: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::new(SqliteSessionStoreConfig {
                db_path: crate::utils::paths::get_data_dir()
                    .unwrap()
                    .join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let context = Arc::new(
            GatewayContext::new(
                sessions,
                Arc::new(AgentRegistry::new()),
                Arc::new(NoOwnerAdapter),
                Arc::new(AgentToAgentPolicy::permissive()),
            )
            .with_telecom_lifecycle(service.clone()),
        );
        let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
            gateway_context: Some(context),
            ..Default::default()
        })
        .await
        .unwrap();
        for tool in [
            "initiate_call",
            "answer_call",
            "hangup",
            "cancel_output",
            "call_status",
        ] {
            let schema = registry
                .get_tool(tool)
                .unwrap()
                .parameters_schema
                .as_ref()
                .unwrap();
            assert_eq!(schema["additionalProperties"], false);
        }
        Self {
            registry,
            service,
            budget,
            dial,
            manager,
            events,
            _home: home,
        }
    }
    fn turn(&self, user: &str, run: &str) -> TurnContext {
        TurnContext {
            session_key: crate::routing::session_key::SessionKey::main("test-core"),
            run_id: run.into(),
            channel_id: "".into(),
            conversation_id: "".into(),
            caller_role: Some("operator".into()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: self.service.issue_for_authenticated_user(user, run),
        }
    }
    async fn call(
        &self,
        turn: TurnContext,
        name: &str,
        args: Value,
    ) -> crate::error::Result<Value> {
        TURN_CONTEXT
            .scope(turn, self.registry.execute_tool(name, args))
            .await
    }
}
async fn decide(
    events: &mut broadcast::Receiver<GatewayEventFrame>,
    manager: &ExecApprovalManager,
    decision: ApprovalDecisionType,
) {
    loop {
        if let GatewayEventFrame::ApprovalRequested {
            approval_id,
            session_key,
            ..
        } = events.recv().await.unwrap()
        {
            assert!(session_key.is_empty());
            let card = manager.get_pending(&approval_id).unwrap().record;
            assert!(card.operator_only);
            assert_eq!(
                card.allowed_decisions,
                vec![ApprovalDecisionType::AllowOnce, ApprovalDecisionType::Deny]
            );
            assert!(manager.resolve(&approval_id, decision, None));
            break;
        }
    }
}
async fn running(f: &Fixture) -> Value {
    loop {
        let status = f
            .call(f.turn("user", "status-run"), "call_status", json!({}))
            .await
            .unwrap();
        if let Some(call) = status["calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["phase"] == "running")
        {
            return call.clone();
        }
        tokio::task::yield_now().await;
    }
}
fn session(call: &Value) -> CallSession {
    CallSession {
        provider_account_id: "acct".into(),
        phone_call_id: call["call_id"].as_str().unwrap().into(),
        provider_call_id: "provider-call".into(),
        binding_id: "bind".into(),
        connection_generation: 1,
    }
}

#[tokio::test]
async fn registry_initiate_approval_transcript_playback_and_owned_hangup() {
    let mut rig = Rig::new().await;
    let history = store();
    let (runner, mut requests) = Runner::new("answer");
    let mut f = Fixture::new(&rig, runner.clone(), Some(history.clone())).await;
    let turn = f.turn("user", "initiate-run");
    let mut start = Box::pin(
        TURN_CONTEXT.scope(
            turn,
            f.registry
                .execute_tool("initiate_call", json!({"binding_ref":"bind"})),
        ),
    );
    // Drive approval registration in this task without spawning an owner run.
    tokio::select! { biased; r = &mut start => panic!("premature start: {r:?}"), _ = tokio::task::yield_now() => {} }
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 0);
    assert_eq!(f.budget.active_count(), 1);
    decide(&mut f.events, &f.manager, ApprovalDecisionType::AllowOnce).await;
    let start = bounded(start).await.unwrap();
    assert_eq!(start["phase"], "running");
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 1);
    let s = session(&start);
    rig.input
        .send(transcript(&s, 1, 1, "question", false))
        .unwrap();
    let request = bounded(requests.recv()).await.unwrap();
    assert_eq!(request.text, "question");
    assert!(request.history.is_empty());
    let key = PublicHistorySession::from_provenance(request.ctx.provenance());
    let output = as_text(bounded(rig.accepted.recv()).await.unwrap());
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    rig.input
        .send(playback(&s, &output, PlayedExtent::Exact { chars: 6 }))
        .unwrap();
    bounded(observed_playback(
        &mut rig.delivered,
        1,
        PlaybackState::Played,
    ))
    .await;
    let end = bounded(f.call(
        f.turn("user", "new-user-run"),
        "hangup",
        json!({"call_id":s.phone_call_id}),
    ))
    .await
    .unwrap();
    assert_eq!(end["status"]["phase"], "closed");
    assert_eq!(end["status"]["remote_outcome"], "accepted");
    assert!(matches!(
        bounded(rig.accepted.recv()).await.unwrap(),
        CallCommand::Hangup { .. }
    ));
    assert_eq!(f.budget.active_count(), 0);
    assert_eq!(f.budget.tokens_spent(), 10);
    let turns = history.load(&key, chrono::Utc::now()).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].assistant, "answer");
}

#[tokio::test]
async fn registry_answer_cancel_exact_epoch_and_user_owned_status() {
    let mut rig = Rig::new().await;
    let (runner, mut requests) = Runner::new("blocked");
    runner.block.store(true, Ordering::Release);
    let mut f = Fixture::new(&rig, runner.clone(), None).await;
    let incoming = rig.admit().await;
    let s = incoming.lease().session().clone();
    f.service
        .register_incoming("user", "bind", incoming)
        .unwrap();
    let mut answer = Box::pin(
        TURN_CONTEXT.scope(
            f.turn("user", "answer-run"),
            f.registry
                .execute_tool("answer_call", json!({"call_id":s.phone_call_id})),
        ),
    );
    tokio::select! { biased; r = &mut answer => panic!("premature answer: {r:?}"), _ = tokio::task::yield_now() => {} }
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 0);
    decide(&mut f.events, &f.manager, ApprovalDecisionType::AllowOnce).await;
    assert_eq!(bounded(answer).await.unwrap()["phase"], "running");
    assert_eq!(bounded(running(&f)).await["call_id"], s.phone_call_id);
    let denied = f
        .call(
            f.turn("other", "other-run"),
            "call_status",
            json!({"call_id":s.phone_call_id}),
        )
        .await;
    assert!(denied.is_err());
    rig.input
        .send(transcript(&s, 1, 1, "question", false))
        .unwrap();
    bounded(requests.recv()).await.unwrap();
    let rejected = f
        .call(
            f.turn("user", "epoch-run"),
            "cancel_output",
            json!({"call_id":s.phone_call_id,"output_epoch":2}),
        )
        .await
        .unwrap();
    assert_eq!(rejected["outcome"], "refused");
    let cancel = bounded(f.call(
        f.turn("user", "epoch-run"),
        "cancel_output",
        json!({"call_id":s.phone_call_id,"output_epoch":1}),
    ))
    .await
    .unwrap();
    assert_eq!(cancel["outcome"], "accepted");
    assert_eq!(cancel["status"]["output_cancelled"], true);
    assert!(matches!(
        bounded(rig.accepted.recv()).await.unwrap(),
        CallCommand::CancelOutput {
            output_epoch: 1,
            ..
        }
    ));
    assert_eq!(runner.drops.load(Ordering::Acquire), 1);
    bounded(f.call(
        f.turn("user", "later-run"),
        "hangup",
        json!({"call_id":s.phone_call_id}),
    ))
    .await
    .unwrap();
    assert_eq!(f.budget.active_count(), 0);
}

#[tokio::test]
async fn registry_denied_or_forged_authority_never_reaches_peer() {
    let rig = Rig::new().await;
    let (runner, _) = Runner::new("unused");
    let mut f = Fixture::new(&rig, runner, None).await;
    for mut turn in [
        f.turn("unknown", "run"),
        f.turn("user", "run"),
        f.turn("user", "revoked"),
    ] {
        if turn.run_id == "run" && turn.telecom_authority.is_some() {
            turn.run_id = "wrong-run".into();
        }
        if turn.run_id == "revoked" {
            turn.telecom_authority.as_ref().unwrap().revoke();
        }
        assert!(f
            .call(turn, "initiate_call", json!({"binding_ref":"bind"}))
            .await
            .is_err());
    }
    assert!(f
        .call(
            f.turn("user", "run"),
            "initiate_call",
            json!({"binding_ref":"foreign"})
        )
        .await
        .is_err());
    assert!(f
        .call(
            f.turn("user", "run"),
            "initiate_call",
            json!({"binding_ref":"bind","role":"operator"})
        )
        .await
        .is_err());
    let mut start = Box::pin(
        TURN_CONTEXT.scope(
            f.turn("user", "deny-run"),
            f.registry
                .execute_tool("initiate_call", json!({"binding_ref":"bind"})),
        ),
    );
    tokio::select! { biased; r = &mut start => panic!("premature deny: {r:?}"), _ = tokio::task::yield_now() => {} }
    decide(&mut f.events, &f.manager, ApprovalDecisionType::Deny).await;
    assert_eq!(bounded(start).await.unwrap()["phase"], "denied");
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 0);
    assert_eq!(f.budget.active_count(), 0);
    assert_eq!(f.budget.tokens_spent(), 0);
}

#[tokio::test(start_paused = true)]
async fn registry_approval_deadline_denies_without_peer_effect_or_permit_leak() {
    let rig = Rig::new().await;
    let (runner, _) = Runner::new("unused");
    let f = Fixture::new(&rig, runner, None).await;
    let mut start = Box::pin(
        TURN_CONTEXT.scope(
            f.turn("user", "expiry-run"),
            f.registry
                .execute_tool("initiate_call", json!({"binding_ref":"bind"})),
        ),
    );
    tokio::select! { biased;
        r = &mut start => panic!("premature start: {r:?}"),
        _ = tokio::task::yield_now() => {},
    }
    assert_eq!(f.budget.active_count(), 1);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(start.await.unwrap()["phase"], "denied");
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 0);
    assert_eq!(f.budget.active_count(), 0);
    assert!(f.manager.list_pending().is_empty());
}

#[tokio::test]
async fn registry_unknown_is_once_only_and_seals_budget_before_next_admission() {
    let rig = Rig::new().await;
    let (runner, _) = Runner::new("unused");
    let mut f = Fixture::new(&rig, runner, None).await;
    f.dial.unknown.store(true, Ordering::Release);
    let mut start = Box::pin(
        TURN_CONTEXT.scope(
            f.turn("user", "unknown-run"),
            f.registry
                .execute_tool("initiate_call", json!({"binding_ref":"bind"})),
        ),
    );
    tokio::select! { biased; r = &mut start => panic!("premature start: {r:?}"), _ = tokio::task::yield_now() => {} }
    decide(&mut f.events, &f.manager, ApprovalDecisionType::AllowOnce).await;
    let result = bounded(start).await.unwrap();
    assert_eq!(result["phase"], "unknown");
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 1);
    assert_eq!(f.budget.tokens_spent(), 1000);
    assert_eq!(f.budget.active_count(), 0);
    assert!(f
        .call(
            f.turn("user", "new-run"),
            "initiate_call",
            json!({"binding_ref":"bind"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("budget"));
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn registry_disconnect_after_approval_at_peer_wait_suppresses_effect() {
    let rig = Rig::new().await;
    let (runner, _) = Runner::new("unused");
    let mut f = Fixture::new(&rig, runner, None).await;
    f.dial.block.store(true, Ordering::Release);
    let mut start = Box::pin(
        TURN_CONTEXT.scope(
            f.turn("user", "run"),
            f.registry
                .execute_tool("initiate_call", json!({"binding_ref":"bind"})),
        ),
    );
    tokio::select! { biased; r = &mut start => panic!("premature start: {r:?}"), _ = tokio::task::yield_now() => {} }
    decide(&mut f.events, &f.manager, ApprovalDecisionType::AllowOnce).await;
    tokio::select! { biased; r = &mut start => panic!("premature peer: {r:?}"), _ = f.dial.entered.notified() => {} }
    rig.relay.disconnect();
    f.dial.gate.notify_one();
    assert_eq!(bounded(start).await.unwrap()["phase"], "unknown");
    assert_eq!(f.dial.effects.load(Ordering::Acquire), 0);
    assert_eq!(f.budget.tokens_spent(), 1000);
    assert_eq!(f.budget.active_count(), 0);
}

#[tokio::test]
async fn lifecycle_service_drop_seals_unknown_cleanup_before_releasing_admission() {
    for before_first_poll in [true, false] {
        let rig = Rig::new().await;
        let (runner, _) = Runner::new("unused");
        bounded(
            crate::gateway::telecom::lifecycle_tools::test_support::aborted_cleanup_seals_spend(
                rig.relay,
                runner,
                before_first_poll,
            ),
        )
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn ringing_revocation_cancellation_and_deadline_release_the_slot() {
    for mode in ["disconnect", "cancel", "deadline"] {
        let rig = Rig::new().await;
        let (runner, _) = Runner::new("unused");
        let f = Fixture::new(&rig, runner, None).await;
        let incoming = rig.admit().await;
        let call = f
            .service
            .register_incoming("user", "bind", incoming)
            .unwrap();
        assert_eq!(f.budget.active_count(), 1);
        match mode {
            "disconnect" => rig.relay.disconnect(),
            "cancel" => {
                assert!(f.budget.cancel_binding("bind"));
            }
            _ => {
                tokio::task::yield_now().await;
                tokio::time::advance(Duration::from_secs(60)).await;
            }
        }
        for _ in 0..20 {
            if f.budget.active_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(f.budget.active_count(), 0, "{mode}");
        let status = f
            .call(
                f.turn("user", "status"),
                "call_status",
                json!({"call_id":call.call_id}),
            )
            .await
            .unwrap();
        assert_eq!(status["calls"][0]["phase"], "closed");
        assert!(status["calls"][0]["remote_outcome"].is_null());
        if mode == "disconnect" {
            rig.relay.connect(policy()).await.unwrap();
        }
        let ctx = rig.relay.connection_witness().unwrap();
        assert!(matches!(
            f.budget.try_admit(ctx.context()),
            PublicAdmission::Admitted(_)
        ));
    }
}
