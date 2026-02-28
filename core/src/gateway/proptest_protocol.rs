//! Property-based tests for JSON-RPC 2.0 protocol serde roundtrip.
//!
//! Uses proptest to verify that serialization followed by deserialization
//! preserves all fields for the Gateway protocol types.

use proptest::prelude::*;
use serde_json::Value;

use super::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, ToolCallParams, ToolCallContext, ToolCallResult};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary serde_json::Value (limited depth to avoid stack overflow).
fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9_ ]{0,30}".prop_map(|s| Value::String(s)),
    ]
}

/// Generate a non-null JSON value.
///
/// serde_json deserializes `"field": null` as `None` for `Option<Value>`,
/// so `Some(Value::Null)` does not survive a roundtrip through
/// `skip_serializing_if = "Option::is_none"`. We exclude `Null` from
/// optional-position strategies to test true roundtrip invariants.
fn arb_json_value_non_null() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9_ ]{0,30}".prop_map(|s| Value::String(s)),
    ]
}

/// Generate an optional JSON value (never Some(Null) to ensure roundtrip fidelity).
fn arb_opt_json_value() -> impl Strategy<Value = Option<Value>> {
    prop_oneof![
        Just(None),
        arb_json_value_non_null().prop_map(Some),
    ]
}

/// Strategy for JsonRpcRequest.
fn arb_jsonrpc_request() -> impl Strategy<Value = JsonRpcRequest> {
    (
        "[a-zA-Z][a-zA-Z0-9_.]{0,30}",  // method (non-empty)
        arb_opt_json_value(),              // params
        arb_opt_json_value(),              // id
    )
        .prop_map(|(method, params, id)| {
            JsonRpcRequest::new(method, params, id)
        })
}

/// Strategy for JsonRpcError.
fn arb_jsonrpc_error() -> impl Strategy<Value = JsonRpcError> {
    (
        any::<i32>(),                      // code
        "[a-zA-Z0-9 _.]{1,50}",           // message
        arb_opt_json_value(),              // data
    )
        .prop_map(|(code, message, data)| {
            JsonRpcError { code, message, data }
        })
}

/// Strategy for JsonRpcResponse.
fn arb_jsonrpc_response() -> impl Strategy<Value = JsonRpcResponse> {
    prop_oneof![
        // Success response (result uses non-null to survive skip_serializing_if roundtrip)
        (arb_opt_json_value(), arb_json_value_non_null()).prop_map(|(id, result)| {
            JsonRpcResponse::success(id, result)
        }),
        // Error response
        (arb_opt_json_value(), any::<i32>(), "[a-zA-Z0-9 ]{1,30}").prop_map(|(id, code, msg)| {
            JsonRpcResponse::error(id, code, msg)
        }),
    ]
}

/// Strategy for ToolCallContext.
fn arb_tool_call_context() -> impl Strategy<Value = ToolCallContext> {
    (
        proptest::option::of("[a-z0-9]{4,12}"),   // request_id
        proptest::option::of("[a-z0-9]{4,12}"),   // session_id
        proptest::option::of(any::<u64>()),        // timeout_ms
    )
        .prop_map(|(request_id, session_id, timeout_ms)| {
            ToolCallContext {
                request_id,
                session_id,
                timeout_ms,
            }
        })
}

/// Strategy for ToolCallParams.
fn arb_tool_call_params() -> impl Strategy<Value = ToolCallParams> {
    (
        "[a-zA-Z][a-zA-Z0-9:._]{0,20}",   // tool
        arb_json_value(),                   // args (non-optional, Null is fine)
        proptest::option::of(arb_tool_call_context()),  // context
    )
        .prop_map(|(tool, args, context)| {
            ToolCallParams { tool, args, context }
        })
}

/// Strategy for ToolCallResult.
fn arb_tool_call_result() -> impl Strategy<Value = ToolCallResult> {
    prop_oneof![
        // Success result
        (arb_json_value(), any::<u64>()).prop_map(|(output, exec_ms)| {
            ToolCallResult::success(output, exec_ms)
        }),
        // Failure result
        ("[a-zA-Z0-9 ]{1,30}", any::<u64>()).prop_map(|(error, exec_ms)| {
            ToolCallResult::failure(error, exec_ms)
        }),
    ]
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// JsonRpcRequest: serialize then deserialize preserves all fields.
    #[test]
    fn jsonrpc_request_serde_roundtrip(req in arb_jsonrpc_request()) {
        let json_str = serde_json::to_string(&req).unwrap();
        let parsed: JsonRpcRequest = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(&parsed.jsonrpc, &req.jsonrpc);
        prop_assert_eq!(&parsed.method, &req.method);
        prop_assert_eq!(&parsed.params, &req.params);
        prop_assert_eq!(&parsed.id, &req.id);
    }

    /// JsonRpcError: serialize then deserialize preserves all fields.
    #[test]
    fn jsonrpc_error_serde_roundtrip(err in arb_jsonrpc_error()) {
        let json_str = serde_json::to_string(&err).unwrap();
        let parsed: JsonRpcError = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(parsed.code, err.code);
        prop_assert_eq!(&parsed.message, &err.message);
        prop_assert_eq!(&parsed.data, &err.data);
    }

    /// jsonrpc version field is always "2.0" after constructing via `new` and roundtrip.
    #[test]
    fn jsonrpc_version_always_two_dot_zero(req in arb_jsonrpc_request()) {
        // After construction
        prop_assert_eq!(&req.jsonrpc, "2.0");

        // After roundtrip
        let json_str = serde_json::to_string(&req).unwrap();
        let parsed: JsonRpcRequest = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed.jsonrpc, "2.0");
    }

    /// Request with None params: roundtrip omits params field but restores as None.
    #[test]
    fn request_none_params_roundtrip(
        method in "[a-zA-Z][a-zA-Z0-9_.]{0,20}",
        id in arb_opt_json_value(),
    ) {
        let req = JsonRpcRequest::new(method, None, id);
        prop_assert!(req.params.is_none());

        let json_str = serde_json::to_string(&req).unwrap();
        // With skip_serializing_if, "params" key should not appear in JSON
        prop_assert!(!json_str.contains("\"params\""));

        let parsed: JsonRpcRequest = serde_json::from_str(&json_str).unwrap();
        prop_assert!(parsed.params.is_none());
    }

    /// JsonRpcResponse serde roundtrip preserves fields.
    #[test]
    fn jsonrpc_response_serde_roundtrip(resp in arb_jsonrpc_response()) {
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(&parsed.jsonrpc, "2.0");
        prop_assert_eq!(&parsed.result, &resp.result);
        prop_assert_eq!(&parsed.id, &resp.id);

        // Compare error fields individually (JsonRpcError does not impl PartialEq)
        match (&parsed.error, &resp.error) {
            (Some(pe), Some(oe)) => {
                prop_assert_eq!(pe.code, oe.code);
                prop_assert_eq!(&pe.message, &oe.message);
                prop_assert_eq!(&pe.data, &oe.data);
            }
            (None, None) => {}
            _ => prop_assert!(false, "error field mismatch"),
        }
    }

    /// ToolCallParams serde roundtrip.
    #[test]
    fn tool_call_params_serde_roundtrip(params in arb_tool_call_params()) {
        let json_str = serde_json::to_string(&params).unwrap();
        let parsed: ToolCallParams = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(&parsed.tool, &params.tool);
        prop_assert_eq!(&parsed.args, &params.args);

        match (&parsed.context, &params.context) {
            (Some(pc), Some(oc)) => {
                prop_assert_eq!(&pc.request_id, &oc.request_id);
                prop_assert_eq!(&pc.session_id, &oc.session_id);
                prop_assert_eq!(pc.timeout_ms, oc.timeout_ms);
            }
            (None, None) => {}
            _ => prop_assert!(false, "context field mismatch"),
        }
    }

    /// ToolCallResult serde roundtrip.
    #[test]
    fn tool_call_result_serde_roundtrip(result in arb_tool_call_result()) {
        let json_str = serde_json::to_string(&result).unwrap();
        let parsed: ToolCallResult = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(&parsed.output, &result.output);
        prop_assert_eq!(parsed.execution_time_ms, result.execution_time_ms);
        prop_assert_eq!(parsed.success, result.success);
        prop_assert_eq!(&parsed.error, &result.error);
    }
}
