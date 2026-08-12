use crate::context::DashboardState;
use crate::state::notifications::{AskOptionView, AskQuestionView, PendingAskView};
use serde::Deserialize;

// ============================================================================
// Clarification API (`ask_user` question card)
// ============================================================================

pub struct ClarificationApi;

#[derive(Deserialize)]
struct PendingListResp {
    pending: Vec<PendingItem>,
}

#[derive(Deserialize)]
struct PendingItem {
    /// Clarification registry key — what `clarification.resolve` is called with.
    session_key: String,
    question: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    questions: Vec<QuestionItem>,
    #[serde(default)]
    answered: usize,
}

/// Wire mirror of core's `ClarificationQuestionView`. Every field past `prompt`
/// defaults, so a core that predates the structured view parses as an empty
/// list and the card falls back to `question` / `options`.
#[derive(Deserialize)]
struct QuestionItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    header: Option<String>,
    prompt: String,
    #[serde(default)]
    options: Vec<OptionItem>,
    #[serde(default)]
    multi_select: bool,
    #[serde(default)]
    secret: bool,
}

#[derive(Deserialize)]
struct OptionItem {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct ResolveResp {
    resolved: bool,
    /// Absent on a core that predates multi-question requests — and `0` is the
    /// right reading there, because every request it can produce has exactly
    /// one question.
    #[serde(default)]
    pending_questions: usize,
}

/// What `clarification.resolve` reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveOutcome {
    /// The answer was taken. `false` = the question was already gone (expired /
    /// superseded / run cancelled), so the reply answered nobody and the caller
    /// must NOT treat its text as consumed.
    pub accepted: bool,
    /// Questions still outstanding on this request.
    ///
    /// ⚠️ `accepted && still_asking > 0` is the case that did not exist before
    /// multi-question requests: the answer landed, and the card must STAY.
    /// Dropping it on `accepted` alone strands the rest of the walk behind a
    /// card the user can no longer see.
    pub still_asking: usize,
}

impl ResolveOutcome {
    /// Whether the request is over — nothing more to ask, so the card goes.
    #[must_use]
    pub const fn is_finished(self) -> bool {
        self.still_asking == 0
    }
}

fn to_view(p: PendingItem) -> PendingAskView {
    PendingAskView {
        session_key: p.session_key,
        question: p.question,
        options: p.options,
        questions: p
            .questions
            .into_iter()
            .map(|q| AskQuestionView {
                id: q.id,
                header: q.header,
                prompt: q.prompt,
                options: q
                    .options
                    .into_iter()
                    .map(|o| AskOptionView {
                        label: o.label,
                        description: o.description,
                    })
                    .collect(),
                multi_select: q.multi_select,
                secret: q.secret,
            })
            .collect(),
        answered: p.answered,
    }
}

impl ClarificationApi {
    /// List the questions core is currently parked on.
    ///
    /// The `stream.ask_user` frame is a one-shot push, so a panel that connects
    /// (or reloads) mid-question would otherwise never learn a tool is blocked
    /// on its answer. Same role as `exec.approvals.pending` for approvals.
    pub async fn list_pending(state: &DashboardState) -> Result<Vec<PendingAskView>, String> {
        let result = state
            .rpc_call("clarification.pending", serde_json::Value::Null)
            .await?;
        let resp: PendingListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending clarifications: {e}"))?;
        Ok(resp.pending.into_iter().map(to_view).collect())
    }

    /// Answer the question at the cursor.
    ///
    /// `reply` is interpreted by core exactly as a channel reply is: a bare
    /// 1-based number picks that option, anything else is free text. The panel
    /// sends the index string for a choice click — the same payload Telegram's
    /// `clarify:<idx>` button produces — and never interprets it itself (R4).
    pub async fn resolve(
        state: &DashboardState,
        session_key: &str,
        reply: &str,
    ) -> Result<ResolveOutcome, String> {
        Self::post(
            state,
            serde_json::json!({ "session_key": session_key, "reply": reply }),
        )
        .await
    }

    /// Answer several consecutive questions in one call — the form path, where
    /// the user filled in every remaining question before submitting.
    ///
    /// Same interpreter, same per-question rules; core walks `answers` across
    /// consecutive questions starting at the cursor.
    pub async fn resolve_all(
        state: &DashboardState,
        session_key: &str,
        answers: &[String],
    ) -> Result<ResolveOutcome, String> {
        Self::post(
            state,
            serde_json::json!({ "session_key": session_key, "answers": answers }),
        )
        .await
    }

    async fn post(
        state: &DashboardState,
        params: serde_json::Value,
    ) -> Result<ResolveOutcome, String> {
        let result = state.rpc_call("clarification.resolve", params).await?;
        let resp: ResolveResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse clarification.resolve: {e}"))?;
        Ok(ResolveOutcome {
            accepted: resp.resolved,
            still_asking: resp.pending_questions,
        })
    }
}
