//! Telecom A/B approval views — the Panel-side projection of the
//! `telecom.approval.*` RPC family (`src/gateway/telecom/operator_control.rs`).
//!
//! Two stages gate a public telecom conversation:
//!   * **A** (`private_read`) — may the model read private context to answer?
//!   * **B** (`disclosure`) — may this final text be sent to the caller?
//!
//! The list RPC only carries a bounded `preview` (≤ 256 bytes). The operator
//! must load the complete body through `telecom.approval.review` before a
//! decision can be sent, and the decision echoes that complete body back so
//! the server can bind the approval to exactly what was reviewed. A preview
//! is never a valid resolve body — [`TelecomApprovalReview`] is the only type
//! the resolve-params builder accepts.

use serde::Deserialize;

/// Which gate a telecom approval belongs to. Wire values are snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelecomApprovalStage {
    /// Stage A — private context read.
    PrivateRead,
    /// Stage B — final disclosure to the public party.
    Disclosure,
}

impl TelecomApprovalStage {
    /// The exact wire value the server expects in `telecom.approval.resolve`.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::PrivateRead => "private_read",
            Self::Disclosure => "disclosure",
        }
    }
}

/// Which public channel the conversation runs on. Wire values are snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelecomApprovalChannel {
    Sms,
    Voice,
}

/// One pending telecom approval as listed by `telecom.approval.pending`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TelecomApprovalView {
    pub id: String,
    pub stage: TelecomApprovalStage,
    pub channel: TelecomApprovalChannel,
    /// Operator-only binding identity; never inferred from public SMS text.
    pub binding_ref: String,
    pub connection_generation: u64,
    pub caller_digest: String,
    pub turn_id: String,
    pub session_epoch: u64,
    /// Bounded display preview (≤ 256 bytes). Display-only — never sent back.
    pub preview: String,
    /// `true` when `preview` is a strict prefix of the complete body.
    #[serde(default)]
    pub truncated: bool,
    /// Absolute epoch-ms deadline stamped by the server.
    pub expires_at_ms: i64,
}

impl TelecomApprovalView {
    /// Whole seconds left before this approval expires, clamped at 0 so an
    /// expired-but-not-yet-refetched row never renders a negative countdown.
    #[must_use]
    pub const fn remaining_secs(&self, now_ms: i64) -> i64 {
        let remaining = self.expires_at_ms - now_ms;
        if remaining < 0 {
            0
        } else {
            remaining / 1000
        }
    }

    /// Whether the deadline has passed at `now_ms`.
    #[must_use]
    pub const fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms <= now_ms
    }
}

/// The complete-body review returned by `telecom.approval.review`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelecomApprovalReview {
    pub view: TelecomApprovalView,
    /// The complete body — the only value a resolve may carry.
    pub body: String,
}

/// Parse the `telecom.approval.pending` result. Items that do not match the
/// contract (unknown stage/channel, missing fields) are dropped individually
/// rather than failing the whole list: an unparseable item cannot be resolved
/// safely anyway, and it must not hide the well-formed ones.
pub fn parse_pending(result: serde_json::Value) -> Result<Vec<TelecomApprovalView>, String> {
    let items = result
        .get("pending")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "telecom.approval.pending: missing `pending` array".to_string())?;
    Ok(items
        .iter()
        .filter_map(|item| serde_json::from_value::<TelecomApprovalView>(item.clone()).ok())
        .collect())
}

/// Parse the `telecom.approval.review` result for `expected_id`. Fails closed
/// unless the server marked the body complete and the id matches the request.
pub fn parse_review(
    result: serde_json::Value,
    expected_id: &str,
) -> Result<TelecomApprovalReview, String> {
    if result.get("complete").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("telecom.approval.review: body is not marked complete".to_string());
    }
    let body = result
        .get("body")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "telecom.approval.review: missing `body`".to_string())?
        .to_string();
    let view: TelecomApprovalView = serde_json::from_value(result)
        .map_err(|e| format!("telecom.approval.review: invalid view: {e}"))?;
    if view.id != expected_id {
        return Err("telecom.approval.review: id mismatch".to_string());
    }
    Ok(TelecomApprovalReview { view, body })
}

/// Build `telecom.approval.resolve` params from a loaded review — always the
/// complete reviewed body, never the list preview. Taking the review (not
/// loose id/stage/body) makes a preview structurally unsendable. The server
/// uses `deny_unknown_fields`, so exactly these four keys are sent.
#[must_use]
pub fn resolve_params(review: &TelecomApprovalReview, approve: bool) -> serde_json::Value {
    serde_json::json!({
        "id": review.view.id,
        "stage": review.view.stage.wire(),
        "approve": approve,
        "body": review.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view_json(id: &str, stage: &str) -> serde_json::Value {
        json!({
            "id": id, "stage": stage, "channel": "sms",
            "binding_ref": "binding-1", "connection_generation": 7,
            "caller_digest": "digest-1", "turn_id": "turn-1", "session_epoch": 9,
            "preview": "short", "truncated": true, "expires_at_ms": 10_000
        })
    }

    #[test]
    fn parse_pending_reads_preview_and_truncated() {
        let list = parse_pending(json!({
            "pending": [view_json("a", "private_read"), {
                "id": "b", "stage": "disclosure", "channel": "voice",
                "binding_ref": "binding-2", "connection_generation": 8,
                "caller_digest": "digest-2", "turn_id": "turn-2", "session_epoch": 10,
                "preview": "full text", "truncated": false, "expires_at_ms": 5
            }],
            "has_more": false
        }))
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].preview, "short");
        assert!(list[0].truncated);
        assert_eq!(list[0].stage, TelecomApprovalStage::PrivateRead);
        assert_eq!(list[0].binding_ref, "binding-1");
        assert_eq!(list[0].connection_generation, 7);
        assert_eq!(list[0].caller_digest, "digest-1");
        assert_eq!(list[0].turn_id, "turn-1");
        assert_eq!(list[0].session_epoch, 9);
        assert_eq!(list[1].channel, TelecomApprovalChannel::Voice);
        assert!(!list[1].truncated);
    }

    #[test]
    fn parse_pending_drops_unknown_stage_but_keeps_the_rest() {
        let list = parse_pending(json!({
            "pending": [view_json("a", "bogus"), view_json("b", "disclosure")]
        }))
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "b");
    }

    #[test]
    fn missing_identity_fails_closed_in_list_and_review() {
        let mut raw = view_json("a", "private_read");
        raw.as_object_mut().unwrap().remove("caller_digest");
        assert!(parse_pending(json!({"pending": [raw.clone()]}))
            .unwrap()
            .is_empty());
        raw["complete"] = json!(true);
        raw["body"] = json!("complete");
        assert!(parse_review(raw, "a").is_err());
    }

    #[test]
    fn parse_pending_requires_pending_array() {
        assert!(parse_pending(json!({})).is_err());
    }

    #[test]
    fn remaining_secs_counts_down_and_clamps() {
        let v = parse_pending(json!({"pending": [view_json("a", "disclosure")]}))
            .unwrap()
            .remove(0);
        assert_eq!(v.remaining_secs(0), 10);
        assert_eq!(v.remaining_secs(9_500), 0);
        assert_eq!(v.remaining_secs(20_000), 0);
        assert!(!v.is_expired(9_999));
        assert!(v.is_expired(10_000));
    }

    #[test]
    fn stage_wire_values_round_trip() {
        for stage in [
            TelecomApprovalStage::PrivateRead,
            TelecomApprovalStage::Disclosure,
        ] {
            let parsed: TelecomApprovalStage = serde_json::from_value(json!(stage.wire())).unwrap();
            assert_eq!(parsed, stage);
        }
        assert_eq!(TelecomApprovalStage::PrivateRead.wire(), "private_read");
        assert_eq!(TelecomApprovalStage::Disclosure.wire(), "disclosure");
    }

    #[test]
    fn parse_review_requires_complete_and_matching_id() {
        let mut ok = view_json("a", "disclosure");
        ok["body"] = json!("the complete body");
        ok["complete"] = json!(true);
        let review = parse_review(ok.clone(), "a").unwrap();
        assert_eq!(review.body, "the complete body");
        assert_eq!(review.view.binding_ref, "binding-1");
        assert_eq!(review.view.connection_generation, 7);
        assert_eq!(review.view.caller_digest, "digest-1");
        assert_eq!(review.view.turn_id, "turn-1");
        assert_eq!(review.view.session_epoch, 9);
        assert!(parse_review(ok.clone(), "other").is_err());

        let mut incomplete = ok.clone();
        incomplete["complete"] = json!(false);
        assert!(parse_review(incomplete, "a").is_err());

        let mut no_body = ok;
        no_body.as_object_mut().unwrap().remove("body");
        assert!(parse_review(no_body, "a").is_err());
    }

    #[test]
    fn resolve_params_carry_the_complete_body_not_the_preview() {
        let mut raw = view_json("a", "private_read");
        raw["body"] = json!("short and then the rest of the complete body");
        raw["complete"] = json!(true);
        let review = parse_review(raw, "a").unwrap();
        assert_eq!(review.view.preview, "short");

        let params = resolve_params(&review, true);
        assert_eq!(
            params,
            json!({
                "id": "a",
                "stage": "private_read",
                "approve": true,
                "body": "short and then the rest of the complete body",
            })
        );
        // Exactly four keys: the server rejects unknown fields.
        assert_eq!(params.as_object().unwrap().len(), 4);
        assert_ne!(params["body"], json!(review.view.preview));
    }

    #[test]
    fn resolve_params_deny_maps_stage() {
        let mut raw = view_json("x", "disclosure");
        raw["body"] = json!("b");
        raw["complete"] = json!(true);
        let review = parse_review(raw, "x").unwrap();
        let p = resolve_params(&review, false);
        assert_eq!(p["stage"], "disclosure");
        assert_eq!(p["approve"], false);
    }
}
