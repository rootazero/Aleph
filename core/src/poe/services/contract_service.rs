//! POE Contract Service
//!
//! Business logic for POE contract signing workflow.
//! Handles the full lifecycle: prepare → sign/reject → execute

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tracing::info;

use crate::error::AlephError;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::JsonRpcRequest;
use crate::poe::{
    ContractContext, ContractSummary, ManifestBuilder, PendingContract, PendingContractStore,
    PendingResult, PrepareResult, RejectResult, SignRequest, SignResult, Worker,
};
use crate::poe::handler_types::PoeRunParams;
use crate::poe::trust::{AutoApprovalDecision, TrustContext, TrustEvaluator};
use crate::resilience::database::StateDatabase;
use super::run_service::PoeRunManager;

/// Parameters for poe.prepare request
#[derive(Debug, Clone, Deserialize)]
pub struct PrepareParams {
    /// Natural language instruction
    pub instruction: String,
    /// Optional context for manifest generation
    #[serde(default)]
    pub context: Option<PrepareContext>,
}

/// Context for poe.prepare request
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PrepareContext {
    /// Working directory
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Related files
    #[serde(default)]
    pub files: Vec<String>,
    /// Session key for events
    #[serde(default)]
    pub session_key: Option<String>,
}

impl From<PrepareContext> for ContractContext {
    fn from(ctx: PrepareContext) -> Self {
        ContractContext {
            working_dir: ctx.working_dir,
            files: ctx.files,
            session_key: ctx.session_key,
        }
    }
}

/// Parameters for poe.reject request
#[derive(Debug, Clone, Deserialize)]
pub struct RejectParams {
    /// Contract ID to reject
    pub contract_id: String,
    /// Optional rejection reason
    #[serde(default)]
    pub reason: Option<String>,
}

/// Service for managing POE contract signing workflow.
///
/// Handles the full lifecycle: prepare → sign/reject → execute
pub struct PoeContractService<W: Worker + 'static> {
    /// Manifest builder for generating contracts
    manifest_builder: Arc<ManifestBuilder>,
    /// Store for pending contracts
    contract_store: Arc<PendingContractStore>,
    /// Run manager for executing signed contracts
    run_manager: Arc<PoeRunManager<W>>,
    /// Event bus for publishing events
    event_bus: Arc<GatewayEventBus>,
    /// Trust evaluator for progressive auto-approval (optional)
    trust_evaluator: Option<Arc<dyn TrustEvaluator>>,
    /// State database for historical trust data (optional)
    state_db: Option<Arc<StateDatabase>>,
}

impl<W: Worker + 'static> PoeContractService<W> {
    /// Create a new contract service.
    pub fn new(
        manifest_builder: Arc<ManifestBuilder>,
        run_manager: Arc<PoeRunManager<W>>,
        event_bus: Arc<GatewayEventBus>,
    ) -> Self {
        Self {
            manifest_builder,
            contract_store: Arc::new(PendingContractStore::new()),
            run_manager,
            event_bus,
            trust_evaluator: None,
            state_db: None,
        }
    }

    /// Set the trust evaluator for progressive auto-approval.
    pub fn with_trust_evaluator(mut self, evaluator: Arc<dyn TrustEvaluator>) -> Self {
        self.trust_evaluator = Some(evaluator);
        self
    }

    /// Set the state database for historical trust data enrichment.
    pub fn with_state_db(mut self, db: Arc<StateDatabase>) -> Self {
        self.state_db = Some(db);
        self
    }

    /// Get access to the contract store.
    pub fn contract_store(&self) -> &Arc<PendingContractStore> {
        &self.contract_store
    }

    /// Get access to the run manager.
    pub fn run_manager(&self) -> &Arc<PoeRunManager<W>> {
        &self.run_manager
    }

    /// Prepare a new contract from instruction.
    ///
    /// Generates a SuccessManifest using ManifestBuilder and stores it
    /// in the pending contracts store awaiting signature.
    pub async fn prepare(&self, params: PrepareParams) -> Result<PrepareResult, AlephError> {
        // 1. Build context string for ManifestBuilder
        let context_str = params.context.as_ref().and_then(|ctx| {
            let contract_ctx: ContractContext = ctx.clone().into();
            contract_ctx.to_context_string()
        });

        // 2. Generate manifest using ManifestBuilder
        let manifest = self
            .manifest_builder
            .build(&params.instruction, context_str.as_deref())
            .await?;

        // 3. Generate contract ID
        let contract_id = format!(
            "contract-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        );

        // 3.5: Evaluate trust for potential auto-approval
        let auto_approved = if let Some(ref evaluator) = self.trust_evaluator {
            let pattern_id = manifest.task_id.clone();
            let mut context = TrustContext::new()
                .with_pattern_id(&pattern_id)
                .with_file_count(manifest.hard_constraints.len());

            // Enrich with historical data if StateDB is available
            if let Some(ref db) = self.state_db {
                if let Ok(Some(score_row)) = db.get_trust_score(&pattern_id).await {
                    context = context.with_history(
                        score_row.trust_score,
                        score_row.total_executions,
                    );
                    if score_row.trust_score >= 0.9 && score_row.total_executions >= 5 {
                        context = context.with_crystallized_skill();
                    }
                }
            }

            let decision = evaluator.evaluate(&manifest, &context);
            match decision {
                AutoApprovalDecision::AutoApprove { ref reason, confidence } => {
                    info!(
                        contract_id = %contract_id,
                        reason = %reason,
                        confidence = %confidence,
                        "Contract auto-approved by trust evaluator"
                    );
                    true
                }
                AutoApprovalDecision::RequireSignature { ref reason } => {
                    info!(
                        contract_id = %contract_id,
                        reason = %reason,
                        "Trust evaluator requires signature"
                    );
                    false
                }
            }
        } else {
            false
        };

        // 4. Create pending contract
        let mut contract = PendingContract::new(
            contract_id.clone(),
            params.instruction.clone(),
            manifest.clone(),
        );

        if let Some(ctx) = params.context {
            contract = contract.with_context(ctx.into());
        }

        // 5. Store in pending contracts (even if auto-approved, for audit trail)
        self.contract_store.insert(contract).await;

        info!(
            contract_id = %contract_id,
            auto_approved = %auto_approved,
            "Contract prepared{}",
            if auto_approved { ", auto-approved" } else { ", awaiting signature" }
        );

        // 6. Emit event
        self.emit_event(
            "poe.contract_generated",
            &json!({
                "contract_id": contract_id,
                "objective": manifest.objective,
                "constraint_count": manifest.hard_constraints.len(),
                "metric_count": manifest.soft_metrics.len(),
                "auto_approved": auto_approved,
            }),
        );

        Ok(PrepareResult {
            contract_id,
            manifest,
            created_at: Utc::now().to_rfc3339(),
            instruction: params.instruction,
            auto_approved,
        })
    }

    /// Sign a pending contract and start execution.
    ///
    /// Optionally applies amendments before execution.
    pub async fn sign(&self, params: SignRequest) -> Result<SignResult, AlephError> {
        // 1. Take contract from store (atomic remove + return)
        let contract = self
            .contract_store
            .take(&params.contract_id)
            .await
            .ok_or_else(|| {
                AlephError::NotFound(format!(
                    "Contract {} not found or already signed",
                    params.contract_id
                ))
            })?;

        // 2. Apply amendments if provided
        let final_manifest = match (&params.amendments, &params.manifest_override) {
            // Natural language amendment
            (Some(amendment), None) => {
                self.manifest_builder
                    .amend(&contract.manifest, amendment)
                    .await?
            }
            // JSON override
            (None, Some(override_manifest)) => {
                ManifestBuilder::merge_override(&contract.manifest, override_manifest)
            }
            // Both: merge first, then amend
            (Some(amendment), Some(override_manifest)) => {
                let merged = ManifestBuilder::merge_override(&contract.manifest, override_manifest);
                self.manifest_builder.amend(&merged, amendment).await?
            }
            // No modifications
            (None, None) => contract.manifest.clone(),
        };

        info!(
            contract_id = %params.contract_id,
            task_id = %final_manifest.task_id,
            amendments = params.amendments.is_some(),
            "Contract signed, starting execution"
        );

        // 3. Emit signed event
        self.emit_event(
            "poe.signed",
            &json!({
                "contract_id": params.contract_id,
                "task_id": final_manifest.task_id,
                "amendments_applied": params.amendments.is_some() || params.manifest_override.is_some(),
                "signed_at": Utc::now().to_rfc3339(),
            }),
        );

        // 4. Start POE execution via run manager
        let run_params = PoeRunParams {
            manifest: final_manifest.clone(),
            instruction: contract.instruction,
            stream: params.stream,
            config: None,
        };

        let run_result = self
            .run_manager
            .start_run(run_params)
            .await
            .map_err(AlephError::other)?;

        Ok(SignResult {
            task_id: run_result.task_id,
            session_key: run_result.session_key,
            signed_at: Utc::now().to_rfc3339(),
            final_manifest,
        })
    }

    /// Reject a pending contract.
    pub async fn reject(&self, params: RejectParams) -> RejectResult {
        let removed = self.contract_store.remove(&params.contract_id).await;

        if removed {
            info!(
                contract_id = %params.contract_id,
                reason = ?params.reason,
                "Contract rejected"
            );

            self.emit_event(
                "poe.rejected",
                &json!({
                    "contract_id": params.contract_id,
                    "reason": params.reason.as_deref().unwrap_or("User cancelled"),
                    "rejected_at": Utc::now().to_rfc3339(),
                }),
            );
        }

        RejectResult {
            contract_id: params.contract_id,
            rejected: removed,
        }
    }

    /// List all pending contracts.
    pub async fn pending(&self) -> PendingResult {
        let contracts = self.contract_store.list().await;
        let count = contracts.len();

        PendingResult {
            contracts: contracts.into_iter().map(ContractSummary::from).collect(),
            count,
        }
    }

    /// Emit an event to the event bus.
    fn emit_event<T: Serialize>(&self, topic: &str, data: &T) {
        if let Ok(data_value) = serde_json::to_value(data) {
            let notification = JsonRpcRequest::notification(
                topic,
                Some(json!({
                    "topic": topic,
                    "data": data_value,
                    "timestamp": Utc::now().timestamp_millis()
                })),
            );
            if let Ok(json) = serde_json::to_string(&notification) {
                self.event_bus.publish(json);
            }
        }
    }
}
