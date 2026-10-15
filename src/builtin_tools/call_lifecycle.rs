//! Telephone tools are pure typed I/O / 电话工具只承载类型化 I/O.
//! Ownership, admission, approval and effects belong to the telecom service.
//! No JSON field, ambient role or missing user can mint a capability.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{AlephError, Result};
use crate::gateway::telecom::lifecycle_tools::{TelecomLifecycleService, TelecomToolAuthority};
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitiateCallArgs {
    /// Opaque OWN binding reference. Recipient/account are server-derived.
    pub binding_ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerCallArgs {
    pub call_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HangupArgs {
    pub call_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelOutputArgs {
    pub call_id: String,
    pub output_epoch: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallStatusArgs {
    /// Omit for a bounded list of calls owned by this authenticated user.
    #[serde(default)]
    pub call_id: Option<String>,
}

fn capability(service: &Arc<TelecomLifecycleService>) -> Result<TelecomToolAuthority> {
    let turn = crate::tools::turn_context::current_turn_context()
        .ok_or_else(|| AlephError::tool("DENIED: no current telecom turn capability"))?;
    service.authority_for_turn(&turn).map_err(AlephError::tool)
}

macro_rules! lifecycle_tool {
    ($tool:ident, $name:literal, $args:ty, $desc:literal, |$service:ident, $authority:ident, $input:ident| $body:block) => {
        #[derive(Clone)]
        pub struct $tool {
            service: Option<Arc<TelecomLifecycleService>>,
        }
        impl $tool {
            pub fn new(service: Option<Arc<TelecomLifecycleService>>) -> Self {
                Self { service }
            }
        }
        #[async_trait]
        impl AlephTool for $tool {
            const NAME: &'static str = $name;
            const DESCRIPTION: &'static str = $desc;
            type Args = $args;
            type Output = Value;
            async fn call(&self, $input: Self::Args) -> Result<Value> {
                let $service = self.service.as_ref().ok_or_else(|| {
                    AlephError::tool("UNAVAILABLE: telecom lifecycle service not configured")
                })?;
                let $authority = capability($service)?;
                $body
            }
            // Do not echo a malformed private body/phone value in errors.
            fn format_validation_error(_: &serde_json::Error) -> String {
                format!(
                    "{} requires its exact schema; unknown fields are forbidden",
                    Self::NAME
                )
            }
            async fn call_json(&self, args: Value) -> Result<Value> {
                let typed = serde_json::from_value(args)
                    .map_err(|e| AlephError::Validation(Self::format_validation_error(&e)))?;
                self.call(typed).await
            }
        }
    };
}
lifecycle_tool!(InitiateCallTool, "initiate_call", InitiateCallArgs,
    "Request a single approved call to your own server-configured binding. No arbitrary recipient or retries.",
    |service, authority, args| {
        let status = service.initiate(authority, args.binding_ref).await.map_err(AlephError::tool)?;
        Ok(serde_json::to_value(status)?)
    });
lifecycle_tool!(AnswerCallTool, "answer_call", AnswerCallArgs,
    "Answer an authenticated ringing call owned by your binding, after single-use operator approval.",
    |service, authority, args| {
        let status = service.answer(authority, args.call_id).await.map_err(AlephError::tool)?;
        Ok(serde_json::to_value(status)?)
    });
lifecycle_tool!(HangupTool, "hangup", HangupArgs,
    "Stop an owned call or pending operation and await owned coordinator completion. No new approval.",
    |service, authority, args| {
        let (status, outcome) = service.hangup(&authority, &args.call_id).await.map_err(AlephError::tool)?;
        Ok(serde_json::json!({ "status": status, "outcome": outcome }))
    });
lifecycle_tool!(CancelOutputTool, "cancel_output", CancelOutputArgs,
    "Cancel exactly the current output epoch of an owned call. Acceptance is not confirmed playback clearing.",
    |service, authority, args| {
        let (status, outcome) = service.cancel_output(&authority, &args.call_id, args.output_epoch).await.map_err(AlephError::tool)?;
        Ok(serde_json::json!({ "status": status, "outcome": outcome }))
    });
lifecycle_tool!(CallStatusTool, "call_status", CallStatusArgs,
    "Get structural status for an owned call, or at most 256 owned records. Never returns phone numbers or conversation text.",
    |service, authority, args| {
        let status = service.status(&authority, args.call_id.as_deref()).map_err(AlephError::tool)?;
        Ok(serde_json::json!({ "calls": status }))
    });

/// Registry-owned dispatch group, always registered even without a service.
#[derive(Default)]
pub struct CallLifecycleTools;
impl CallLifecycleTools {
    pub(crate) fn register(tools: &mut HashMap<String, UnifiedTool>) {
        // Generate schemas without constructing a configured service.
        macro_rules! add_schema {
            ($tool:ty) => {{
                let mut tool = UnifiedTool::new(
                    format!("builtin:{}", <$tool>::NAME),
                    <$tool>::NAME,
                    <$tool>::DESCRIPTION,
                    ToolSource::Builtin,
                );
                tool.parameters_schema = Some(
                    serde_json::to_value(schemars::schema_for!(<$tool as AlephTool>::Args))
                        .expect("lifecycle schema"),
                );
                tool.max_result_tokens = <$tool>::MAX_RESULT_TOKENS;
                tools.insert(<$tool>::NAME.into(), tool);
            }};
        }
        add_schema!(InitiateCallTool);
        add_schema!(AnswerCallTool);
        add_schema!(HangupTool);
        add_schema!(CancelOutputTool);
        add_schema!(CallStatusTool);
    }
    pub(crate) async fn execute(
        &self,
        service: Option<Arc<TelecomLifecycleService>>,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        match name {
            "initiate_call" => InitiateCallTool::new(service).call_json(args).await,
            "answer_call" => AnswerCallTool::new(service).call_json(args).await,
            "hangup" => HangupTool::new(service).call_json(args).await,
            "cancel_output" => CancelOutputTool::new(service).call_json(args).await,
            "call_status" => CallStatusTool::new(service).call_json(args).await,
            _ => Err(AlephError::tool("unknown telephone lifecycle tool")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_reject_all_raw_authority_fields() {
        for key in [
            "phone",
            "recipient",
            "account",
            "generation",
            "role",
            "approval_ref",
            "operation_id",
            "budget",
        ] {
            let mut value = serde_json::json!({"binding_ref": "own"});
            value[key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<InitiateCallArgs>(value).is_err());
            let mut call = serde_json::json!({"call_id": "own"});
            call[key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<AnswerCallArgs>(call.clone()).is_err());
            assert!(serde_json::from_value::<HangupArgs>(call.clone()).is_err());
            assert!(serde_json::from_value::<CallStatusArgs>(call.clone()).is_err());
            call["output_epoch"] = serde_json::json!(1);
            assert!(serde_json::from_value::<CancelOutputArgs>(call).is_err());
        }
    }
    #[tokio::test]
    async fn unavailable_is_real_dispatch_not_a_missing_tool() {
        let tools = CallLifecycleTools;
        for (name, args) in [
            ("initiate_call", serde_json::json!({"binding_ref":"own"})),
            ("answer_call", serde_json::json!({"call_id":"own"})),
            ("hangup", serde_json::json!({"call_id":"own"})),
            (
                "cancel_output",
                serde_json::json!({"call_id":"own","output_epoch":1}),
            ),
            ("call_status", serde_json::json!({})),
        ] {
            assert!(tools
                .execute(None, name, args)
                .await
                .unwrap_err()
                .to_string()
                .contains("UNAVAILABLE"));
        }
    }
}
