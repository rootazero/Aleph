//! `Runtime.*` — evaluating in the page.

use serde_json::{json, Value};

use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

#[derive(Clone, Debug, PartialEq)]
pub struct EvalResult {
    /// The page's value. `Null` when the expression produced `undefined` — a real JS result, not
    /// a missing field.
    pub value: Value,
    /// Set when the expression threw. Never folded into `value`: a thrown error that came back as
    /// `null` would read to the caller as "the page said null" (判据 §8).
    pub exception: Option<String>,
}

pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Runtime.enable", json!({})).await?;
    Ok(())
}

/// `returnByValue` is always true: without it the reply is a remote handle and `value` is simply
/// absent, which would be read as `null` by anything downstream.
pub async fn evaluate(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    expression: &str,
    await_promise: bool,
) -> Result<EvalResult> {
    const M: &str = "Runtime.evaluate";
    let reply = conn
        .call(
            session,
            M,
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": await_promise,
            }),
        )
        .await?;
    Ok(eval_result(&reply))
}

/// CDP takes `CallArgument` objects, not bare values: a bare array is silently dropped and the
/// function runs with no arguments at all.
pub async fn call_function_on(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    object_id: &str,
    declaration: &str,
    args: Vec<Value>,
) -> Result<EvalResult> {
    const M: &str = "Runtime.callFunctionOn";
    let arguments: Vec<Value> = args.into_iter().map(|v| json!({ "value": v })).collect();
    let reply = conn
        .call(
            session,
            M,
            json!({
                "objectId": object_id,
                "functionDeclaration": declaration,
                "arguments": arguments,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await?;
    Ok(eval_result(&reply))
}

fn eval_result(reply: &Value) -> EvalResult {
    let value = reply
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);
    let exception = reply.get("exceptionDetails").map(|details| {
        details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| details.get("text").and_then(Value::as_str))
            .unwrap_or("<exception with no description>")
            .to_string()
    });
    EvalResult { value, exception }
}
