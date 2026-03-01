//! Wizard RPC handlers.
//!
//! Handlers for wizard session operations:
//! - wizard.start - Start a new wizard session
//! - wizard.next - Get next step / answer current step
//! - wizard.cancel - Cancel the wizard session

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use crate::sync_primitives::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::wizard::{
    WizardFlow, WizardNextResult, WizardSession, WizardSessionError, WizardStatus, WizardStep,
};

/// Type alias for an async RPC handler function
type RpcHandler = Box<dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync>;

/// Parameters for wizard.start
#[derive(Debug, Deserialize)]
pub struct WizardStartParams {
    /// Wizard type (e.g., "onboarding", "provider-setup")
    pub wizard_type: String,
    /// Optional initial data
    #[serde(default)]
    pub initial_data: Option<Value>,
}

/// Response for wizard.start
#[derive(Debug, Serialize)]
pub struct WizardStartResponse {
    /// Session ID
    pub session_id: String,
    /// First step
    pub step: Option<WizardStep>,
    /// Status
    pub status: WizardStatus,
}

/// Parameters for wizard.next
#[derive(Debug, Deserialize)]
pub struct WizardNextParams {
    /// Session ID
    pub session_id: String,
    /// Answer to current step (null for notes/intro)
    #[serde(default)]
    pub answer: Option<Value>,
}

/// Parameters for wizard.cancel
#[derive(Debug, Deserialize)]
pub struct WizardCancelParams {
    /// Session ID
    pub session_id: String,
}

/// Response for wizard.cancel
#[derive(Debug, Serialize)]
pub struct WizardCancelResponse {
    /// Whether the cancellation was successful
    pub cancelled: bool,
}

/// Factory function to create wizard flows
pub type WizardFlowFactory = Arc<dyn Fn(&str, Option<Value>) -> Option<Box<dyn WizardFlow>> + Send + Sync>;

/// Wizard session manager
pub struct WizardSessionManager {
    sessions: RwLock<HashMap<String, Arc<WizardSession>>>,
    flow_factory: WizardFlowFactory,
}

impl WizardSessionManager {
    /// Create a new session manager with a flow factory
    pub fn new(flow_factory: WizardFlowFactory) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            flow_factory,
        }
    }

    /// Start a new wizard session
    pub async fn start(&self, wizard_type: &str, initial_data: Option<Value>) -> Result<(String, WizardNextResult), WizardSessionError> {
        // Create the flow
        let flow = (self.flow_factory)(wizard_type, initial_data)
            .ok_or_else(|| WizardSessionError::FlowError(format!("Unknown wizard type: {}", wizard_type)))?;

        // Create the session
        let session = Arc::new(WizardSession::new(flow));
        let session_id = session.id().to_string();

        // Store the session first
        {
            let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
            sessions.insert(session_id.clone(), session.clone());
        }

        // Get the first step (outside the lock)
        let first_result = session.next().await;

        Ok((session_id, first_result))
    }

    /// Get next step or answer current step
    pub async fn next(&self, session_id: &str, answer: Option<Value>) -> Result<WizardNextResult, WizardSessionError> {
        // Get the session (clone Arc, release lock immediately)
        let session = {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            sessions.get(session_id).cloned()
        };

        let session = session.ok_or_else(|| {
            WizardSessionError::StepNotFound(format!("Session not found: {}", session_id))
        })?;

        // Check current status
        if session.is_done() {
            return Ok(WizardNextResult::done());
        }

        // If there's an answer, submit it first
        if let Some(_value) = answer {
            // Answer must include step_id - use wizard.answer instead
            return Err(WizardSessionError::InvalidAnswer(
                "Answer must include step_id - use wizard.answer instead".to_string()
            ));
        }

        // Get next step (outside the lock)
        let result = session.next().await;

        // Clean up if done
        if result.done {
            let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
            sessions.remove(session_id);
        }

        Ok(result)
    }

    /// Answer a step
    pub async fn answer(&self, session_id: &str, step_id: &str, value: Value) -> Result<(), WizardSessionError> {
        // Get the session (clone Arc, release lock immediately)
        let session = {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            sessions.get(session_id).cloned()
        };

        let session = session.ok_or_else(|| {
            WizardSessionError::StepNotFound(format!("Session not found: {}", session_id))
        })?;

        session.answer(step_id, value).await
    }

    /// Cancel a session
    ///
    /// Removes the session from the manager. When the Arc is dropped,
    /// the session's channels will close, triggering cancellation.
    pub fn cancel(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        sessions.remove(session_id).is_some()
    }

    /// Get session status
    pub fn status(&self, session_id: &str) -> Option<WizardStatus> {
        let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
        sessions.get(session_id).map(|s| s.status())
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Option<Arc<WizardSession>> {
        let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
        sessions.get(session_id).cloned()
    }
}

/// Handle wizard.start
pub async fn handle_start(
    req: JsonRpcRequest,
    manager: Arc<WizardSessionManager>,
) -> JsonRpcResponse {
    let params: WizardStartParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    req.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    match manager.start(&params.wizard_type, params.initial_data).await {
        Ok((session_id, first_result)) => {
            let response = WizardStartResponse {
                session_id,
                step: first_result.step,
                status: first_result.status,
            };
            JsonRpcResponse::success(req.id, serde_json::to_value(response).unwrap())
        }
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string())
        }
    }
}

/// Parameters for wizard.answer (extended)
#[derive(Debug, Deserialize)]
pub struct WizardAnswerParams {
    /// Session ID
    pub session_id: String,
    /// Step ID being answered
    pub step_id: String,
    /// Answer value
    pub value: Value,
}

/// Handle wizard.answer
pub async fn handle_answer(
    req: JsonRpcRequest,
    manager: Arc<WizardSessionManager>,
) -> JsonRpcResponse {
    let params: WizardAnswerParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    req.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    // Answer the step
    if let Err(e) = manager.answer(&params.session_id, &params.step_id, params.value).await {
        return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string());
    }

    // Get the next step
    match manager.next(&params.session_id, None).await {
        Ok(result) => {
            JsonRpcResponse::success(req.id, serde_json::to_value(result).unwrap())
        }
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string())
        }
    }
}

/// Handle wizard.next (get next step without answering)
pub async fn handle_next(
    req: JsonRpcRequest,
    manager: Arc<WizardSessionManager>,
) -> JsonRpcResponse {
    let params: WizardNextParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    req.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    match manager.next(&params.session_id, params.answer).await {
        Ok(result) => {
            JsonRpcResponse::success(req.id, serde_json::to_value(result).unwrap())
        }
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string())
        }
    }
}

/// Handle wizard.cancel
pub async fn handle_cancel(
    req: JsonRpcRequest,
    manager: Arc<WizardSessionManager>,
) -> JsonRpcResponse {
    let params: WizardCancelParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    req.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    let cancelled = manager.cancel(&params.session_id);
    let response = WizardCancelResponse { cancelled };
    JsonRpcResponse::success(req.id, serde_json::to_value(response).unwrap())
}

/// Handle wizard.status
pub async fn handle_status(
    req: JsonRpcRequest,
    manager: Arc<WizardSessionManager>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct StatusParams {
        session_id: String,
    }

    let params: StatusParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    req.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    match manager.status(&params.session_id) {
        Some(status) => {
            JsonRpcResponse::success(req.id, json!({ "status": status }))
        }
        None => {
            JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Session not found: {}", params.session_id),
            )
        }
    }
}

/// Create handlers that need the session manager
pub fn create_handlers(
    manager: Arc<WizardSessionManager>,
) -> impl Fn(&str) -> Option<RpcHandler>
{
    move |method: &str| -> Option<RpcHandler> {
        let mgr = manager.clone();
        match method {
            "wizard.start" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_start(req, manager))
                }))
            }
            "wizard.next" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_next(req, manager))
                }))
            }
            "wizard.answer" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_answer(req, manager))
                }))
            }
            "wizard.cancel" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_cancel(req, manager))
                }))
            }
            "wizard.status" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_status(req, manager))
                }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::{RpcPrompter, WizardStep};
    use async_trait::async_trait;

    struct TestFlow {
        name: String,
    }

    #[async_trait]
    impl WizardFlow for TestFlow {
        async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
            prompter.prompt(WizardStep::note("intro", "Welcome!")).await?;
            Ok(())
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    fn test_flow_factory(wizard_type: &str, _initial_data: Option<Value>) -> Option<Box<dyn WizardFlow>> {
        match wizard_type {
            "test" => Some(Box::new(TestFlow { name: "test".to_string() })),
            _ => None,
        }
    }

    #[tokio::test]
    async fn test_session_manager_start() {
        let factory: WizardFlowFactory = Arc::new(test_flow_factory);
        let manager = Arc::new(WizardSessionManager::new(factory));

        let (session_id, result) = manager.start("test", None).await.unwrap();
        assert!(!session_id.is_empty());
        assert!(!result.done);
        assert!(result.step.is_some());
    }

    #[tokio::test]
    async fn test_session_manager_unknown_type() {
        let factory: WizardFlowFactory = Arc::new(test_flow_factory);
        let manager = Arc::new(WizardSessionManager::new(factory));

        let result = manager.start("unknown", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_cancel() {
        let factory: WizardFlowFactory = Arc::new(test_flow_factory);
        let manager = Arc::new(WizardSessionManager::new(factory));

        let (session_id, _) = manager.start("test", None).await.unwrap();

        assert!(manager.cancel(&session_id));
        assert!(!manager.cancel(&session_id)); // Already cancelled
    }

    #[tokio::test]
    async fn test_handle_start() {
        let factory: WizardFlowFactory = Arc::new(test_flow_factory);
        let manager = Arc::new(WizardSessionManager::new(factory));

        let req = JsonRpcRequest::new(
            "wizard.start",
            Some(json!({ "wizard_type": "test" })),
            Some(json!(1)),
        );

        let resp = handle_start(req, manager).await;
        assert!(resp.is_success());
    }

    #[tokio::test]
    async fn test_handle_cancel() {
        let factory: WizardFlowFactory = Arc::new(test_flow_factory);
        let manager = Arc::new(WizardSessionManager::new(factory));

        // Start a session first
        let (session_id, _) = manager.start("test", None).await.unwrap();

        let req = JsonRpcRequest::new(
            "wizard.cancel",
            Some(json!({ "session_id": session_id })),
            Some(json!(2)),
        );

        let resp = handle_cancel(req, manager).await;
        assert!(resp.is_success());
    }
}
