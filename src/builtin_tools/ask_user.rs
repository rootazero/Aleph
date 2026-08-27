//! `ask_user` — mid-task clarification tool.
//!
//! Lets the agent pause, ask the user **one to four** questions through the
//! originating channel, and resume once they answer. The agent loop blocks
//! inside this tool's `call` until every answer arrives, the request is
//! superseded, or the clarification times out.
//!
//! Everything about *reaching* the human — routability, the headless refusal,
//! the channel → event-bus delivery ladder, the secret rule, the wait — lives
//! in [`crate::clarification::ask`], shared with the `scratchpad` plan-approval
//! gate. What is left here is this tool's own job: its argument schema, the
//! normalisation of what a model actually emits, and the shape of the answer it
//! gets back.
//!
//! # Why more than one question
//!
//! A single-question tool makes the model pay a full round trip per question,
//! so it either asks one thing and guesses the rest, or burns N turns. codex
//! (`request_user_input`, 1–3), pi (`questionnaire`, N) and hermes
//! (`clarify` + `multi_select`) all landed on the same answer. The cost of
//! plurality is answered in `clarification::session`: rich clients answer all
//! at once, plain-text surfaces answer one per message, and neither is a
//! degraded mode of the other.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clarification::{
    ask, AskOutcome, ClarificationAnswer, ClarificationDeps, ClarificationManager,
    ClarificationOption, ClarificationQuestion, ClarificationRequest, ClarificationResultType,
};
use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Upper bound on questions per call.
///
/// Four, not "as many as you like": every extra question is one more thing a
/// human has to hold in their head before they can answer any of them, and a
/// sequential surface renders them one at a time — a ten-question wall is a
/// worse interaction than two calls of five. codex caps at 3, Claude Code's own
/// `AskUserQuestion` at 4.
const MAX_QUESTIONS: usize = 4;

/// A single choice offered to the user.
///
/// Accepts either a bare string (`"staging"`) or an object with an
/// explanatory description (`{"label": "staging", "description": "shared QA
/// environment"}`). The bare-string form keeps backward compatibility with
/// the simple `choices: ["a", "b"]` shape.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AskUserChoice {
    /// A simple choice label (also used as the returned value).
    Simple(String),
    /// A labeled choice with a short description shown beside it.
    Detailed {
        /// The choice label (also used as the returned value).
        label: String,
        /// A short description helping the user choose.
        description: String,
    },
}

impl AskUserChoice {
    /// The label/value the user picks and that is returned as the answer.
    fn label(&self) -> &str {
        match self {
            Self::Simple(label) | Self::Detailed { label, .. } => label,
        }
    }

    /// The optional description shown beside the label.
    fn description(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Detailed { description, .. } => Some(description),
        }
    }

    fn to_option(&self) -> ClarificationOption {
        let opt = ClarificationOption::new(self.label(), self.label());
        match self.description() {
            Some(desc) => opt.with_description(desc),
            None => opt,
        }
    }
}

/// One question of an `ask_user` call.
///
/// Argument doc comments here become `description`s in the JSON schema and are
/// billed by `registry_schema_bytes_ratchet`, so they carry only what the
/// schema itself cannot state — a default, or a relationship to another field.
/// The semantics live once in [`AskUserTool::DESCRIPTION`].
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserQuestionArg {
    /// Answer key. Defaults to `q1`, `q2`, … in order.
    #[serde(default)]
    pub id: Option<String>,

    /// 2–3 word chip shown beside the question.
    #[serde(default)]
    pub header: Option<String>,

    /// The question, self-contained — the user sees no other context.
    pub question: String,

    /// Choices: strings, or `{label, description}`.
    #[serde(default)]
    pub choices: Vec<AskUserChoice>,

    /// Accept several picks.
    #[serde(default)]
    pub multi_select: bool,

    /// Answer is a credential: masked input, no messaging channel.
    #[serde(default)]
    pub secret: bool,
}

/// Arguments for the `ask_user` tool.
///
/// Two accepted shapes. The flat `question` / `choices` pair is the one-question
/// form; `questions` is the list form. Supplying both is an error rather than a
/// merge — a merge would have to invent an order, and the order is what the
/// user answers in.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AskUserArgs {
    /// One question. Not with `questions`.
    #[serde(default)]
    pub question: Option<String>,

    /// Choices for `question`.
    #[serde(default)]
    pub choices: Vec<AskUserChoice>,

    /// Up to four questions at once. Not with `question`.
    #[serde(default)]
    pub questions: Vec<AskUserQuestionArg>,
}

/// One answered question.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AskUserAnswer {
    /// The question's `id` (auto-assigned `q1`, `q2`, … when not supplied).
    pub question_id: String,
    /// The user's answer — selected choice label(s), or their own text.
    pub answer: String,
    /// 0-based indices of the choices matched, empty when the user wrote
    /// something not on the list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selected_indices: Vec<u32>,
}

/// Questions the human was never shown, and why.
///
/// One field rather than two (a bare id list plus a sentence somewhere else):
/// the reason is what the model has to act on, and an id list with the reason
/// living in a tool description is a fact split across two places that can
/// drift. Present exactly when something was withheld — the shape answers "did
/// they see everything I asked", so no consumer has to keep a list of the
/// situations in which they might not have.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WithheldQuestions {
    /// The `id`s that were not asked, in the order they were passed.
    pub question_ids: Vec<String>,
    /// What to do about it, phrased for the model.
    pub reason: String,
}

/// Output of the `ask_user` tool.
#[derive(Debug, Clone, Serialize)]
pub struct AskUserOutput {
    /// `"answered"`, `"timeout"`, or `"cancelled"`.
    ///
    /// Describes the questions that were **asked**. When `withheld` is present
    /// the set asked was smaller than the set passed, and `"answered"` means
    /// every *asked* question got an answer — never that nothing was held back.
    pub status: String,

    /// The first answer — the one-question shorthand. Absent on timeout or
    /// cancellation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,

    /// 0-based index of the chosen option for the first question, when a
    /// choice list was offered and the reply matched one of the choices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_index: Option<u32>,

    /// Every answer, in question order. One entry for a single question.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<AskUserAnswer>,

    /// Present only when some questions could not be put to the human at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withheld: Option<WithheldQuestions>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that asks the user clarifying questions and waits for the replies.
#[derive(Clone)]
pub struct AskUserTool {
    deps: ClarificationDeps,
}

impl AskUserTool {
    #[must_use]
    pub const fn new(
        clarification: Arc<ClarificationManager>,
        channels: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            deps: ClarificationDeps::new(clarification, channels),
        }
    }

    /// Turn the tool's two accepted argument shapes into one request.
    ///
    /// Rejects rather than repairs in the three cases where repairing would
    /// mean inventing intent: both shapes at once (what order?), neither
    /// (nothing to ask), and a blank prompt (a question the user cannot read).
    fn build_request(args: &AskUserArgs) -> Result<ClarificationRequest> {
        let flat = args
            .question
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        if flat.is_some() && !args.questions.is_empty() {
            return Err(AlephError::tool(
                "ask_user: pass either `question` (one question) or `questions` (a list), not both",
            ));
        }

        let mut questions: Vec<ClarificationQuestion> = Vec::new();
        if let Some(prompt) = flat {
            questions.push(ClarificationQuestion::select(
                "q1",
                prompt,
                args.choices.iter().map(AskUserChoice::to_option).collect(),
            ));
        } else {
            if args.questions.len() > MAX_QUESTIONS {
                return Err(AlephError::tool(format!(
                    "ask_user: at most {MAX_QUESTIONS} questions per call — ask the rest after \
                     these are answered"
                )));
            }
            for (i, q) in args.questions.iter().enumerate() {
                let prompt = q.question.trim();
                if prompt.is_empty() {
                    return Err(AlephError::tool(format!(
                        "ask_user: question {} has empty text",
                        i + 1
                    )));
                }
                let id =
                    q.id.as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map_or_else(|| format!("q{}", i + 1), ToString::to_string);
                let mut built = ClarificationQuestion::select(
                    &id,
                    prompt,
                    q.choices.iter().map(AskUserChoice::to_option).collect(),
                )
                .with_multi_select(q.multi_select)
                .with_secret(q.secret);
                if let Some(header) = q.header.as_deref() {
                    built = built.with_header(header);
                }
                questions.push(built);
            }
        }

        if questions.is_empty() {
            return Err(AlephError::tool(
                "ask_user: `question` must not be empty — say what you need to know",
            ));
        }
        // Distinct ids or the answer map is ambiguous. Auto-assigned ids are
        // unique by construction, so this only ever fires on model-supplied
        // duplicates.
        let mut seen = std::collections::HashSet::with_capacity(questions.len());
        if let Some(dup) = questions.iter().find(|q| !seen.insert(q.id.as_str())) {
            return Err(AlephError::tool(format!(
                "ask_user: duplicate question id `{}` — each question needs its own",
                dup.id
            )));
        }

        ClarificationRequest::new(questions).map_err(AlephError::tool)
    }

    /// Map what [`ask`] came back with onto the tool output.
    fn result_to_output(outcome: AskOutcome) -> AskUserOutput {
        let AskOutcome {
            result,
            withheld_secret,
        } = outcome;
        let status = match result.result_type {
            ClarificationResultType::Answered => "answered",
            ClarificationResultType::Timeout => "timeout",
            ClarificationResultType::Cancelled => "cancelled",
        };
        let answers: Vec<AskUserAnswer> = result
            .answers
            .iter()
            .map(|a: &ClarificationAnswer| AskUserAnswer {
                question_id: a.question_id.clone(),
                answer: a.value.clone(),
                selected_indices: a.selected_indices.clone(),
            })
            .collect();
        AskUserOutput {
            status: status.to_string(),
            answer: result.value().map(ToString::to_string),
            selected_index: result.selected_index(),
            answers,
            withheld: (!withheld_secret.is_empty()).then(|| WithheldQuestions {
                question_ids: withheld_secret,
                reason: WITHHELD_SECRET_REASON.to_string(),
            }),
        }
    }
}

/// What to tell the model about a question it asked that the human never saw.
///
/// Actionable, and explicitly *not* a retry instruction: asking again over the
/// same channel produces the same result, and the point of answering the rest
/// of the call is that the run can continue.
const WITHHELD_SECRET_REASON: &str =
    "these ask for a credential, and this conversation runs over a messaging channel where the \
     reply would be a permanent message in a third party's history — so they were not shown. The \
     other questions were asked normally. Do not re-ask them here: have the user set the value \
     through the Panel or configuration, and continue without it.";

#[async_trait]
impl AlephTool for AskUserTool {
    const NAME: &'static str = "ask_user";
    const DESCRIPTION: &'static str =
        "Ask the user up to four clarifying questions and wait for the reply, instead of \
         guessing a detail that is theirs to decide. Pass `question` for one, or `questions` \
         for several in one interaction. Choices are strings or `{label, description}`; free \
         text is always accepted, so never add an \"other\" choice. `multi_select` takes several \
         picks; `secret` masks the input and is skipped on a messaging channel. The run pauses \
         until they answer; the replies — or a timeout — come back.";

    type Args = AskUserArgs;
    type Output = AskUserOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let request = Self::build_request(&args)?;
        let outcome = ask(&self.deps, request)
            .await
            .map_err(|e| AlephError::tool(format!("ask_user: {e}")))?;
        Ok(Self::result_to_output(outcome))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::{ClarificationAnswer, ClarificationResult};
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn tool() -> AskUserTool {
        AskUserTool::new(
            Arc::new(ClarificationManager::new()),
            Arc::new(ChannelRegistry::new()),
        )
    }

    fn args(question: &str) -> AskUserArgs {
        AskUserArgs {
            question: Some(question.to_string()),
            choices: vec![],
            questions: vec![],
        }
    }

    fn routable_turn() -> TurnContext {
        TurnContext {
            session_key: crate::routing::session_key::SessionKey::ephemeral("ask-user-test"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "user-1".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    #[tokio::test]
    async fn errors_when_question_empty() {
        let err = tool()
            .call(args("   "))
            .await
            .expect_err("empty question must be rejected");
        assert!(err.to_string().contains("must not be empty"), "{err}");
    }

    #[tokio::test]
    async fn errors_without_turn_context() {
        // No TURN_CONTEXT scope — the tool cannot reach any channel.
        let err = tool()
            .call(args("Which one?"))
            .await
            .expect_err("missing turn context must be rejected");
        assert!(
            err.to_string().contains("interactive channel turn"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn errors_when_no_transport_can_reach_the_user() {
        // Routable turn, but the registry has no such channel AND the turn has
        // no gateway run (`run_id` empty) to publish an `AskUser` frame
        // against — neither transport can reach the user, so the pending
        // clarification is rolled back instead of blocking on a reply that can
        // never arrive.
        let err = TURN_CONTEXT
            .scope(routable_turn(), async {
                tool().call(args("Which one?")).await
            })
            .await
            .expect_err("delivery failure must surface as an error");
        assert!(err.to_string().contains("failed to deliver"), "{err}");
    }

    #[test]
    fn build_request_flat_form_makes_one_question() {
        let request = AskUserTool::build_request(&AskUserArgs {
            question: Some("Pick?".into()),
            choices: vec![
                AskUserChoice::Simple("alpha".into()),
                AskUserChoice::Detailed {
                    label: "beta".into(),
                    description: "the other one".into(),
                },
            ],
            questions: vec![],
        })
        .expect("flat form builds");
        assert_eq!(request.len(), 1);
        let q = request
            .first()
            .expect("constructor-built request is non-empty");
        assert_eq!(q.id, "q1");
        assert_eq!(q.prompt, "Pick?");
        // Description is wired onto the option, not merely rendered — this is
        // the field that used to reach channels and nothing else.
        assert_eq!(q.options[0].value, "alpha");
        assert!(q.options[0].description.is_none());
        assert_eq!(q.options[1].description.as_deref(), Some("the other one"));
    }

    #[test]
    fn build_request_list_form_assigns_ids_and_carries_flags() {
        let request = AskUserTool::build_request(&AskUserArgs {
            question: None,
            choices: vec![],
            questions: vec![
                AskUserQuestionArg {
                    id: None,
                    header: Some("Env".into()),
                    question: "Where?".into(),
                    choices: vec![AskUserChoice::Simple("prod".into())],
                    multi_select: false,
                    secret: false,
                },
                AskUserQuestionArg {
                    id: Some("token".into()),
                    header: None,
                    question: "API token?".into(),
                    choices: vec![],
                    multi_select: false,
                    secret: true,
                },
            ],
        })
        .expect("list form builds");
        assert_eq!(request.len(), 2);
        assert_eq!(
            request.questions()[0].id,
            "q1",
            "omitted ids are positional"
        );
        assert_eq!(request.questions()[0].header.as_deref(), Some("Env"));
        assert_eq!(request.questions()[1].id, "token");
        assert!(request.questions()[1].secret);
    }

    /// Merging the two shapes would have to invent an order, and the order is
    /// what the user answers in.
    #[test]
    fn build_request_refuses_both_shapes_at_once() {
        let err = AskUserTool::build_request(&AskUserArgs {
            question: Some("Pick?".into()),
            choices: vec![],
            questions: vec![AskUserQuestionArg {
                id: None,
                header: None,
                question: "Also this?".into(),
                choices: vec![],
                multi_select: false,
                secret: false,
            }],
        })
        .expect_err("ambiguous argument shape must be rejected");
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn build_request_caps_the_question_count() {
        let many: Vec<AskUserQuestionArg> = (0..MAX_QUESTIONS + 1)
            .map(|i| AskUserQuestionArg {
                id: None,
                header: None,
                question: format!("Q{i}?"),
                choices: vec![],
                multi_select: false,
                secret: false,
            })
            .collect();
        let err = AskUserTool::build_request(&AskUserArgs {
            question: None,
            choices: vec![],
            questions: many,
        })
        .expect_err("over-long question lists must be rejected");
        assert!(err.to_string().contains("at most"), "{err}");
    }

    /// Duplicate ids make the answer set ambiguous for the model that asked.
    #[test]
    fn build_request_refuses_duplicate_ids() {
        let dup = |id: &str| AskUserQuestionArg {
            id: Some(id.to_string()),
            header: None,
            question: "?".into(),
            choices: vec![],
            multi_select: false,
            secret: false,
        };
        let err = AskUserTool::build_request(&AskUserArgs {
            question: None,
            choices: vec![],
            questions: vec![dup("env"), dup("env")],
        })
        .expect_err("duplicate ids must be rejected");
        assert!(err.to_string().contains("duplicate question id"), "{err}");
    }

    #[test]
    fn ask_user_choice_deserializes_string_and_object_forms() {
        // Backward-compatible bare-string form.
        let simple: AskUserChoice = serde_json::from_str(r#""staging""#).unwrap();
        assert_eq!(simple.label(), "staging");
        assert!(simple.description().is_none());
        // Richer object form.
        let detailed: AskUserChoice =
            serde_json::from_str(r#"{"label":"prod","description":"live traffic"}"#).unwrap();
        assert_eq!(detailed.label(), "prod");
        assert_eq!(detailed.description(), Some("live traffic"));
    }

    /// The pre-multi-question call shape must keep deserializing verbatim —
    /// it is what every existing prompt, skill and transcript emits.
    #[test]
    fn legacy_single_question_arguments_still_parse() {
        let args: AskUserArgs =
            serde_json::from_str(r#"{"question":"Deploy where?","choices":["staging","prod"]}"#)
                .expect("legacy shape parses");
        let request = AskUserTool::build_request(&args).expect("legacy shape builds");
        assert_eq!(request.len(), 1);
        assert_eq!(
            request
                .first()
                .expect("constructor-built request is non-empty")
                .options
                .len(),
            2
        );
    }

    /// Nothing withheld — the ordinary case, and the one that must keep
    /// `withheld` absent from the JSON entirely.
    fn asked(result: ClarificationResult) -> AskOutcome {
        AskOutcome {
            result,
            withheld_secret: Vec::new(),
        }
    }

    #[test]
    fn result_to_output_maps_each_status() {
        let answered = AskUserTool::result_to_output(asked(ClarificationResult::answered(vec![
            ClarificationAnswer {
                question_id: "q1".into(),
                selected_indices: vec![],
                value: "hi".into(),
            },
        ])));
        assert_eq!(answered.status, "answered");
        assert_eq!(answered.answer.as_deref(), Some("hi"));
        assert_eq!(answered.answers.len(), 1);
        assert!(answered.withheld.is_none());

        let selected = AskUserTool::result_to_output(asked(ClarificationResult::answered(vec![
            ClarificationAnswer {
                question_id: "q1".into(),
                selected_indices: vec![1],
                value: "beta".into(),
            },
        ])));
        assert_eq!(selected.selected_index, Some(1));

        let timed_out = AskUserTool::result_to_output(asked(ClarificationResult::timeout()));
        assert_eq!(timed_out.status, "timeout");
        assert!(timed_out.answer.is_none());
        assert!(timed_out.answers.is_empty());

        let cancelled = AskUserTool::result_to_output(asked(ClarificationResult::cancelled()));
        assert_eq!(cancelled.status, "cancelled");
    }

    /// A partly-askable call reports BOTH halves: the answers it got and the
    /// questions the human never saw. `status` describes the questions that
    /// were asked, so the only thing standing between "answered" and "answered
    /// everything" is this field being present — which is why it carries its
    /// own reason rather than an id list the model has to interpret.
    #[test]
    fn a_withheld_secret_is_named_alongside_the_answers_it_did_get() {
        let out = AskUserTool::result_to_output(AskOutcome {
            result: ClarificationResult::answered(vec![ClarificationAnswer {
                question_id: "env".into(),
                selected_indices: vec![0],
                value: "staging".into(),
            }]),
            withheld_secret: vec!["token".into()],
        });
        assert_eq!(out.status, "answered");
        assert_eq!(out.answers.len(), 1);
        let withheld = out.withheld.expect("a withheld question must be reported");
        assert_eq!(withheld.question_ids, vec!["token".to_string()]);
        // Actionable, and explicitly not a retry instruction.
        assert!(
            withheld.reason.contains("Do not re-ask"),
            "{}",
            withheld.reason
        );
        assert!(withheld.reason.contains("Panel"), "{}", withheld.reason);

        // Absent — not `null`, not an empty object — when nothing was withheld.
        let clean = serde_json::to_value(AskUserTool::result_to_output(asked(
            ClarificationResult::timeout(),
        )))
        .expect("output serializes");
        assert!(
            clean.get("withheld").is_none(),
            "an ordinary call must not carry an empty withheld object: {clean}"
        );
    }

    /// The one-question shorthands (`answer` / `selected_index`) are the FIRST
    /// answer, never a silent join — a model that reads only `answer` on a
    /// multi-question call must get one question's answer, not a merged blob.
    #[test]
    fn multi_question_output_keeps_the_shorthand_pointing_at_the_first() {
        let out = AskUserTool::result_to_output(asked(ClarificationResult::answered(vec![
            ClarificationAnswer {
                question_id: "env".into(),
                selected_indices: vec![0],
                value: "staging".into(),
            },
            ClarificationAnswer {
                question_id: "ticket".into(),
                selected_indices: vec![],
                value: "ALEPH-1".into(),
            },
        ])));
        assert_eq!(out.answer.as_deref(), Some("staging"));
        assert_eq!(out.selected_index, Some(0));
        assert_eq!(out.answers.len(), 2);
        assert_eq!(out.answers[1].question_id, "ticket");
        assert_eq!(out.answers[1].answer, "ALEPH-1");
    }
}
