//! Authenticated operator-only telecom A/B control service.
//!
//! Startup registers this service (`telecom.approval.*`) and hands the same
//! instance to the inbound router, which opens a trusted live turn scope only
//! after a public SMS is admitted and retires it on every exit path via
//! `ScopeGuard`. Production wires no private-read driver, so an approved A
//! record is never consumed and no private disclosure (B) is ever produced.
//!
//! The service is separate from `exec.approval.*`: complete bodies and stages
//! are checked by the ledger. Role comes from gateway task-locals, never params.

use super::public_approval::{
    ApprovalSource, PublicApprovalLedger, PublicApprovalScope, PublicApprovalStage,
};
use super::public_context::TelecomChannel;
use crate::gateway::caller_identity;
use crate::gateway::handlers::{parse_params, HandlerRegistry};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INVALID_PARAMS};
use crate::sync_primitives::{Arc, Mutex};
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;

/// Explicit instance state; no process-global installation or silent fallback.
/// The caller retains the ledger Arc for the future public driver's effect fence.
pub struct OperatorControl {
    ledger: Arc<PublicApprovalLedger>,
    scopes: Mutex<HashMap<String, PublicApprovalScope>>,
}

impl OperatorControl {
    pub fn new(ledger: Arc<PublicApprovalLedger>) -> Self {
        Self {
            ledger,
            scopes: Mutex::new(HashMap::new()),
        }
    }

    /// The shared ledger, for the trusted public run driver's private turn
    /// (request A/B, wait, consume). Never handed to the public runner.
    pub fn ledger(&self) -> &Arc<PublicApprovalLedger> {
        &self.ledger
    }

    /// Integration seam ONLY: trusted driver code supplies the current scope,
    /// never caller-controlled metadata. One live turn per binding is supported.
    /// Replacing a turn retires old approvals before the replacement is visible.
    pub fn register_scope(&self, scope: PublicApprovalScope) {
        let mut scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        if scopes
            .get(scope.binding_ref())
            .is_some_and(|old| old != &scope)
        {
            self.ledger.cancel_binding(scope.binding_ref());
        }
        scopes.insert(scope.binding_ref().to_string(), scope);
    }

    /// Integration seam ONLY: caller must invoke on every lifecycle invalidation.
    pub fn retire_scope(&self, binding_ref: &str) {
        let mut scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.remove(binding_ref);
        self.ledger.cancel_binding(binding_ref);
    }

    /// Whether `scope` is still the live scope of its binding. The private
    /// turn re-checks this after drafting and before delivery.
    pub fn is_current(&self, scope: &PublicApprovalScope) -> bool {
        let scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.get(scope.binding_ref()) == Some(scope)
    }

    /// Open approval A for the trusted private-turn driver, under the same
    /// scope lock as [`Self::handle_request`]: a retired or replaced scope
    /// cannot race a stale request into the ledger.
    pub(crate) fn request_private_read_for(
        &self,
        scope: &PublicApprovalScope,
        body: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<String, super::PublicApprovalError> {
        let scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        if scopes.get(scope.binding_ref()) != Some(scope) {
            return Err(super::PublicApprovalError::ScopeMismatch);
        }
        self.ledger.request_private_read(scope.clone(), body, now)
    }

    /// Retire `scope` only if it is still the live scope for its binding.
    /// A finished turn must never retire a newer turn's replacement scope.
    pub fn retire_scope_if_current(&self, scope: &PublicApprovalScope) {
        let mut scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        if scopes.get(scope.binding_ref()) == Some(scope) {
            scopes.remove(scope.binding_ref());
            self.ledger.cancel_binding(scope.binding_ref());
        }
    }

    /// Snapshot of currently-registered scopes, for tests. Production
    /// paths must NOT use this — the operator plane reports pending
    /// requests through the `telecom.approval.pending` JSON-RPC method.
    #[cfg(test)]
    pub fn list_pending_for_test(&self) -> Vec<crate::gateway::telecom::PublicApprovalScope> {
        let scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        scopes.values().cloned().collect()
    }

    pub async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if let Err(e) = operator_only(request.id.clone()) {
            return e;
        }
        let p: RequestPrivateRead = match parse_params(&request) {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Hold the scope lock through insertion so retirement cannot race a
        // cloned stale scope into the ledger after cancellation has finished.
        let scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(scope) = scopes.get(&p.binding_ref) else {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "unknown or inactive telecom binding",
            );
        };
        match self
            .ledger
            .request_private_read(scope.clone(), &p.query, Utc::now())
        {
            Ok(id) => {
                JsonRpcResponse::success(request.id, json!({"id": id, "stage":"private_read"}))
            }
            Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
        }
    }

    pub async fn handle_list(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if let Err(e) = operator_only(request.id.clone()) {
            return e;
        }
        let p: ListParams = match request
            .params
            .as_ref()
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
        {
            Ok(v) => v.unwrap_or(ListParams { limit: None }),
            Err(_) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    "invalid telecom approval list params",
                )
            }
        };
        // The ledger itself caps live records at 256. Never hide a pending
        // approval behind an unpageable default list limit.
        let limit = p
            .limit
            .unwrap_or(super::public_approval::MAX_APPROVAL_RECORDS)
            .min(super::public_approval::MAX_APPROVAL_RECORDS);
        let mut items = self.ledger.operator_views(Utc::now());
        items.sort_by(|a, b| a.expires_at.cmp(&b.expires_at).then(a.id.cmp(&b.id)));
        let has_more = items.len() > limit;
        items.truncate(limit);
        JsonRpcResponse::success(
            request.id,
            json!({
                "pending": items.iter().map(|v| view(v, false)).collect::<Vec<_>>(),
                "has_more": has_more,
            }),
        )
    }

    pub async fn handle_review(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if let Err(e) = operator_only(request.id.clone()) {
            return e;
        }
        let p: ReviewParams = match parse_params(&request) {
            Ok(v) => v,
            Err(e) => return e,
        };
        match self.ledger.operator_view(&p.id, Utc::now()) {
            Some(v) => JsonRpcResponse::success(request.id, view(&v, true)),
            None => JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "unknown or retired approval request",
            ),
        }
    }

    pub async fn handle_resolve(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if let Err(e) = operator_only(request.id.clone()) {
            return e;
        }
        let p: ResolveParams = match parse_params(&request) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if p.body.len() > super::public_approval::MAX_APPROVAL_BODY_BYTES {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "approval body exceeds the operator-visible bound",
            );
        }
        match self.ledger.decide_exact(
            &p.id,
            p.stage.stage(),
            &p.body,
            ApprovalSource::operator_control_plane(),
            p.approve,
            Utc::now(),
        ) {
            Ok(()) => JsonRpcResponse::success(
                request.id,
                json!({"ok":true,"stage":wire_stage(p.stage.stage())}),
            ),
            Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
        }
    }
}

/// Retires a registered trusted A/B scope when dropped.
///
/// Manual retire after `.await` misses two exit paths: the enclosing future
/// being dropped mid-run (e.g. connection teardown) and a panic unwinding
/// through it. Tying retire to `Drop` makes the scope's lifetime exactly the
/// lifetime of the admitted public turn that opened it. Retiring only takes
/// synchronous mutexes, so it is safe to call from `Drop`.
pub(crate) struct ScopeGuard {
    control: Arc<OperatorControl>,
    scope: PublicApprovalScope,
}

impl ScopeGuard {
    /// Register `scope` on `control` and return a guard that retires it.
    pub(crate) fn register(control: Arc<OperatorControl>, scope: PublicApprovalScope) -> Self {
        control.register_scope(scope.clone());
        Self { control, scope }
    }

    /// The live turn scope this guard keeps registered.
    pub(crate) fn scope(&self) -> &PublicApprovalScope {
        &self.scope
    }

    /// The control plane the scope is registered on.
    pub(crate) fn control(&self) -> &Arc<OperatorControl> {
        &self.control
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        // Compare-and-retire: a newer turn on the same binding keeps its scope.
        self.control.retire_scope_if_current(&self.scope);
    }
}

impl std::fmt::Debug for OperatorControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperatorControl(<redacted>)")
    }
}

// The error is the ready-to-send RPC response; boxing it buys nothing here.
#[allow(clippy::result_large_err)]
fn operator_only(id: Option<serde_json::Value>) -> Result<(), JsonRpcResponse> {
    // Unlike some internal gates, absent identity is NOT trusted here.
    if caller_identity::current_caller_role().as_deref() != Some("operator") {
        return Err(JsonRpcResponse::error(
            id,
            AUTH_REQUIRED,
            "authenticated operator required",
        ));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestPrivateRead {
    binding_ref: String,
    query: String,
}
impl std::fmt::Debug for RequestPrivateRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestPrivateRead(<redacted>)")
    }
}
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    limit: Option<usize>,
}
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewParams {
    id: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveParams {
    id: String,
    stage: WireStage,
    approve: bool,
    body: String,
}
impl std::fmt::Debug for ResolveParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveParams")
            .field("id", &self.id)
            .field("stage", &self.stage)
            .field("approve", &self.approve)
            .field("body", &"<redacted>")
            .finish()
    }
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireStage {
    PrivateRead,
    Disclosure,
}
impl WireStage {
    fn stage(&self) -> PublicApprovalStage {
        match self {
            Self::PrivateRead => PublicApprovalStage::PrivateRead,
            Self::Disclosure => PublicApprovalStage::Disclosure,
        }
    }
}
fn wire_stage(s: PublicApprovalStage) -> &'static str {
    match s {
        PublicApprovalStage::PrivateRead => "private_read",
        PublicApprovalStage::Disclosure => "disclosure",
    }
}
fn wire_channel(c: TelecomChannel) -> &'static str {
    match c {
        TelecomChannel::Sms => "sms",
        TelecomChannel::Voice => "voice",
    }
}
fn bounded_preview(body: &str) -> (&str, bool) {
    let mut end = body.len().min(256);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    (&body[..end], end < body.len())
}
fn view(v: &super::public_approval::PublicApprovalView, complete: bool) -> serde_json::Value {
    let (preview, truncated) = bounded_preview(&v.body);
    let mut out = json!({"id": v.id, "stage": wire_stage(v.stage), "channel": wire_channel(v.channel),
        "binding_ref": v.binding_ref, "connection_generation": v.connection_generation,
        "caller_digest": v.caller_digest, "turn_id": v.turn_id, "session_epoch": v.session_epoch,
        "preview": preview, "truncated": truncated, "expires_at_ms": v.expires_at.timestamp_millis()});
    if complete {
        out["body"] = json!(v.body);
        out["complete"] = json!(true);
    }
    out
}

/// Registers the operator JSON-RPC surface. Startup calls this only when the
/// shared ledger was installed, and passes the same instance to the inbound
/// router so operator decisions and live turn scopes share one ledger.
pub fn register_handlers(registry: &mut HandlerRegistry, control: Arc<OperatorControl>) {
    let c = control.clone();
    registry.register("telecom.approval.request", move |req| {
        let c = c.clone();
        async move { c.handle_request(req).await }
    });
    let c = control.clone();
    registry.register("telecom.approval.pending", move |req| {
        let c = c.clone();
        async move { c.handle_list(req).await }
    });
    let c = control.clone();
    registry.register("telecom.approval.review", move |req| {
        let c = c.clone();
        async move { c.handle_review(req).await }
    });
    registry.register("telecom.approval.resolve", move |req| {
        let c = control.clone();
        async move { c.handle_resolve(req).await }
    });
}

#[cfg(test)]
mod tests {
    use super::super::public_approval::{
        PublicApprovalError, MAX_APPROVAL_BODY_BYTES, MAX_APPROVAL_RECORDS,
    };
    use super::*;
    use crate::gateway::telecom::public_context::test_support::sample_public_context;

    const METHODS: [&str; 4] = [
        "telecom.approval.request",
        "telecom.approval.pending",
        "telecom.approval.review",
        "telecom.approval.resolve",
    ];
    const CANARY: &str = "PRIVATE-CANARY-operator-7731";
    fn scope() -> PublicApprovalScope {
        PublicApprovalScope::new(&sample_public_context("op-bind"), "c", "a", "t", 1)
    }
    fn service() -> (
        Arc<PublicApprovalLedger>,
        Arc<OperatorControl>,
        HandlerRegistry,
    ) {
        let ledger = Arc::new(PublicApprovalLedger::new());
        let control = Arc::new(OperatorControl::new(ledger.clone()));
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, control.clone());
        (ledger, control, registry)
    }
    async fn call(
        registry: &HandlerRegistry,
        method: &str,
        params: serde_json::Value,
        role: Option<&str>,
    ) -> JsonRpcResponse {
        let req = JsonRpcRequest::with_id(method, Some(params), json!(1));
        caller_identity::CALLER_ROLE
            .scope(role.map(str::to_string), registry.handle(&req))
            .await
    }

    #[tokio::test]
    async fn all_methods_refuse_nonoperators_before_parsing_or_lookup() {
        let (ledger, _, registry) = service();
        let id = ledger
            .request_private_read(scope(), CANARY, Utc::now())
            .unwrap();
        for role in [None, Some("member"), Some("guest")] {
            for method in METHODS {
                // A forged role in params is never authority, even with a real id.
                let out = call(
                    &registry,
                    method,
                    json!({"id":id,"role":"operator","body":CANARY}),
                    role,
                )
                .await;
                assert_eq!(out.error.unwrap().code, AUTH_REQUIRED, "{method} {role:?}");
            }
        }
        // Also refuse a call with no task-local scope at all.
        let out = registry
            .handle(&JsonRpcRequest::with_id(METHODS[0], None, json!(1)))
            .await;
        assert_eq!(out.error.unwrap().code, AUTH_REQUIRED);
        assert!(ledger.operator_view(&id, Utc::now()).is_some());
    }

    #[tokio::test]
    async fn instance_handlers_reach_only_the_explicit_shared_ledger() {
        let (ledger, control, registry) = service();
        let (other_ledger, other_control, other_registry) = service();
        control.register_scope(scope());
        other_control.register_scope(scope());
        let out = call(
            &registry,
            METHODS[0],
            json!({"binding_ref":"op-bind","query":CANARY}),
            Some("operator"),
        )
        .await;
        let id = out.result.unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(ledger.operator_view(&id, Utc::now()).unwrap().body, CANARY);
        assert!(other_ledger.operator_view(&id, Utc::now()).is_none());
        let out = call(
            &other_registry,
            METHODS[2],
            json!({"id":id}),
            Some("operator"),
        )
        .await;
        assert_eq!(out.error.unwrap().code, INVALID_PARAMS);
        control.retire_scope("op-bind");
        assert!(ledger.operator_view(&id, Utc::now()).is_none());
        let out = call(
            &registry,
            METHODS[0],
            json!({"binding_ref":"op-bind","query":CANARY}),
            Some("operator"),
        )
        .await;
        assert_eq!(out.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn review_is_complete_but_preview_cannot_resolve_stage_or_body_mismatch() {
        let (ledger, _, registry) = service();
        let body = "私".repeat(MAX_APPROVAL_BODY_BYTES / 3);
        let id = ledger
            .request_private_read(scope(), &body, Utc::now())
            .unwrap();
        let page = call(&registry, METHODS[1], json!({}), Some("operator"))
            .await
            .result
            .unwrap();
        let summary = &page["pending"][0];
        let preview = summary["preview"].as_str().unwrap();
        assert!(preview.len() <= 256);
        assert_eq!(summary["truncated"], true);
        assert!(summary.get("body").is_none());
        assert_eq!(summary["binding_ref"], "op-bind");
        assert_eq!(summary["connection_generation"], 1);
        assert!(summary["caller_digest"].is_string());
        assert_eq!(summary["turn_id"], "t");
        assert_eq!(summary["session_epoch"], 1);
        let review = call(&registry, METHODS[2], json!({"id":id}), Some("operator"))
            .await
            .result
            .unwrap();
        assert_eq!(review["body"], body);
        assert_eq!(review["complete"], true);
        for field in [
            "binding_ref",
            "connection_generation",
            "caller_digest",
            "turn_id",
            "session_epoch",
        ] {
            assert_eq!(
                review[field], summary[field],
                "review identity drift: {field}"
            );
        }
        for approve in [true, false] {
            for (stage, supplied, expected) in [
                ("disclosure", body.as_str(), PublicApprovalError::WrongStage),
                ("private_read", preview, PublicApprovalError::BodyMismatch),
                (
                    "private_read",
                    "changed body",
                    PublicApprovalError::BodyMismatch,
                ),
            ] {
                let out = call(
                    &registry,
                    METHODS[3],
                    json!({"id":id,"stage":stage,"approve":approve,"body":supplied}),
                    Some("operator"),
                )
                .await;
                let error = out.error.unwrap();
                assert_eq!(error.code, INVALID_PARAMS);
                assert_eq!(error.message, expected.to_string());
                assert!(ledger.operator_view(&id, Utc::now()).is_some());
            }
        }
        let out = call(
            &registry,
            METHODS[3],
            json!({"id":id,"stage":"private_read","approve":true,"body":body}),
            Some("operator"),
        )
        .await;
        assert!(out.error.is_none());
        assert_eq!(
            ledger
                .consume(
                    &id,
                    PublicApprovalStage::PrivateRead,
                    ApprovalSource::operator_control_plane(),
                    &scope(),
                    &body,
                    Utc::now()
                )
                .unwrap()
                .into_body(),
            body
        );
    }

    #[tokio::test]
    async fn pending_page_and_input_bounds_are_enforced() {
        let (ledger, control, registry) = service();
        control.register_scope(scope());
        for _ in 0..MAX_APPROVAL_RECORDS {
            ledger
                .request_private_read(scope(), CANARY, Utc::now())
                .unwrap();
        }
        for (params, n, more) in [
            (json!({}), MAX_APPROVAL_RECORDS, false),
            (json!({"limit":1000}), MAX_APPROVAL_RECORDS, false),
            (json!({"limit":1}), 1, true),
            (json!({"limit":0}), 0, true),
        ] {
            let page = call(&registry, METHODS[1], params, Some("operator"))
                .await
                .result
                .unwrap();
            assert_eq!(page["pending"].as_array().unwrap().len(), n);
            assert_eq!(page["has_more"], more);
        }
        let id = ledger.operator_views(Utc::now())[0].id.clone();
        for params in [
            json!({"id":id,"stage":"private_read","approve":true}),
            json!({"id":id,"stage":"private_read","approve":true,"body":CANARY,"role":"operator"}),
            json!({"id":id,"stage":"private_read","approve":true,"body":"x".repeat(MAX_APPROVAL_BODY_BYTES + 1)}),
        ] {
            let out = call(&registry, METHODS[3], params, Some("operator")).await;
            assert_eq!(out.error.unwrap().code, INVALID_PARAMS);
            assert!(ledger.operator_view(&id, Utc::now()).is_some());
        }
        let out = call(
            &registry,
            METHODS[0],
            json!({"binding_ref":"op-bind","query":"x".repeat(MAX_APPROVAL_BODY_BYTES + 1)}),
            Some("operator"),
        )
        .await;
        assert_eq!(out.error.unwrap().code, INVALID_PARAMS);
        assert_eq!(
            ledger.operator_views(Utc::now()).len(),
            MAX_APPROVAL_RECORDS
        );
    }

    #[test]
    fn scope_replacement_cancels_previous_turn_approvals() {
        let (ledger, control, _) = service();
        control.register_scope(scope());
        let id = ledger
            .request_private_read(scope(), CANARY, Utc::now())
            .unwrap();
        control.register_scope(scope());
        assert!(ledger.operator_view(&id, Utc::now()).is_some());
        control.register_scope(PublicApprovalScope::new(
            &sample_public_context("op-bind"),
            "c",
            "a",
            "next-turn",
            2,
        ));
        assert!(ledger.operator_view(&id, Utc::now()).is_none());
    }

    #[test]
    fn preview_handles_utf8_boundaries_and_debug_redacts_sensitive_fields() {
        for body in [
            String::new(),
            "x".repeat(256),
            "x".repeat(257),
            "x".repeat(255) + "私",
        ] {
            let (preview, truncated) = bounded_preview(&body);
            assert!(preview.len() <= 256);
            assert!(body.starts_with(preview));
            assert_eq!(truncated, preview.len() < body.len());
        }
        let request = RequestPrivateRead {
            binding_ref: CANARY.into(),
            query: CANARY.into(),
        };
        let resolve = ResolveParams {
            id: "id".into(),
            stage: WireStage::PrivateRead,
            approve: true,
            body: CANARY.into(),
        };
        let (_, control, _) = service();
        assert!(!format!("{request:?} {resolve:?} {control:?}").contains(CANARY));
    }

    /// Dropping `ScopeGuard` (the run driver's only retire path) removes the
    /// scope and cancels the binding's pending ledger requests.
    #[test]
    fn scope_guard_drop_retires_scope_and_cancels_pending() {
        let (ledger, c, _) = service();
        let guard = ScopeGuard::register(c.clone(), scope());
        let now = Utc::now();
        let id = ledger
            .request_private_read(scope(), "owner files", now)
            .expect("pending private-read request");
        assert!(ledger.operator_view(&id, now).is_some());
        assert!(c
            .list_pending_for_test()
            .iter()
            .any(|s| s.binding_ref() == "op-bind"));

        drop(guard);

        assert!(
            c.list_pending_for_test()
                .iter()
                .all(|s| s.binding_ref() != "op-bind"),
            "scope must be removed when the guard drops"
        );
        assert!(
            ledger.operator_view(&id, now).is_none(),
            "pending request must be cancelled when the guard drops"
        );
    }

    /// An older turn's guard must not retire a newer turn's scope on the same
    /// binding (compare-and-retire).
    #[test]
    fn stale_scope_guard_does_not_retire_newer_scope() {
        let (_, c, _) = service();
        let ctx = sample_public_context("op-bind");
        let mk = || {
            PublicApprovalScope::new(
                &ctx,
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().to_string(),
                ctx.provenance().connection_generation(),
            )
        };
        let old = ScopeGuard::register(c.clone(), mk());
        let newer = mk();
        let new_guard = ScopeGuard::register(c.clone(), newer.clone());
        drop(old);
        assert!(c.list_pending_for_test().iter().any(|s| s == &newer));
        drop(new_guard);
        assert!(c
            .list_pending_for_test()
            .iter()
            .all(|s| s.binding_ref() != "op-bind"));
    }

    #[test]
    fn ordinary_registry_lacks_the_service_and_startup_wires_it() {
        let ordinary = HandlerRegistry::new();
        for method in METHODS {
            assert!(!ordinary.has_method(method));
        }
        // Positive wiring guard: startup registers the operator surface and
        // hands the same (optional, fail-closed) instance to the inbound router.
        let startup = include_str!("../../bin/aleph-server/commands/start/mod.rs");
        let production = crate::utils::source_scan::strip_comment_lines(startup);
        assert!(production.contains("operator_control::register_handlers("));
        assert!(production.contains("if let Some(control) = trusted_operator_control.as_ref()"));
        let router_call = production
            .find("initialize_inbound_router(\n")
            .expect("startup calls initialize_inbound_router");
        assert!(
            production[router_call..].contains("trusted_operator_control.clone(),"),
            "the inbound router must receive the trusted operator control"
        );
    }
}
