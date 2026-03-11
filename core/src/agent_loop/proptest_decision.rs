//! Property tests for Decision, Action, and ActionResult serde and classification
//!
//! Tests:
//! 1. Decision serde roundtrip — serialize then deserialize preserves value
//! 2. ActionResult serde roundtrip — serialize then deserialize preserves value
//! 3. Terminal decision classification — Complete, Fail, Silent, HeartbeatOk are terminal
//! 4. ActionResult success classification — ToolSuccess, UserResponse, UserResponseRich, Completed are success

use proptest::prelude::*;
use serde_json::{json, Value};

use super::answer::UserAnswer;
use super::decision::{Action, ActionResult, Decision, QuestionGroup, ToolCallRecord, ToolCallRequest, ToolCallResult, SingleToolResult};
use super::question::{ChoiceOption, QuestionKind, TextValidation};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary JSON Value (limited depth to avoid infinite recursion)
fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        "[a-zA-Z0-9_ ]{0,20}".prop_map(|s| Value::String(s)),
        Just(json!({})),
        Just(json!({"key": "value"})),
        Just(json!({"operation": "read", "path": "/tmp"})),
    ]
}

/// Generate an arbitrary ChoiceOption
fn arb_choice_option() -> impl Strategy<Value = ChoiceOption> {
    ("[a-zA-Z ]{1,20}", proptest::option::of("[a-zA-Z ]{1,30}")).prop_map(|(label, desc)| {
        ChoiceOption {
            label,
            description: desc,
        }
    })
}

/// Generate an arbitrary QuestionKind
fn arb_question_kind() -> impl Strategy<Value = QuestionKind> {
    prop_oneof![
        // Confirmation
        (any::<bool>(), proptest::option::of(("[a-z]{1,10}", "[a-z]{1,10}"))).prop_map(
            |(default, labels)| {
                QuestionKind::Confirmation {
                    default,
                    labels: labels.map(|(a, b)| (a, b)),
                }
            }
        ),
        // SingleChoice
        (
            proptest::collection::vec(arb_choice_option(), 1..=5),
            proptest::option::of(0..5usize)
        )
            .prop_map(|(choices, default_index)| {
                QuestionKind::SingleChoice {
                    choices,
                    default_index,
                }
            }),
        // MultiChoice
        (
            proptest::collection::vec(arb_choice_option(), 1..=5),
            0..3usize,
            proptest::option::of(1..5usize)
        )
            .prop_map(|(choices, min, max)| {
                QuestionKind::MultiChoice {
                    choices,
                    min_selections: min,
                    max_selections: max,
                }
            }),
        // TextInput
        (
            proptest::option::of("[a-zA-Z ]{1,15}"),
            any::<bool>(),
            proptest::option::of(prop_oneof![
                Just(TextValidation::Required),
                (proptest::option::of(0..10usize), proptest::option::of(10..100usize))
                    .prop_map(|(min, max)| TextValidation::Length { min, max }),
            ])
        )
            .prop_map(|(placeholder, multiline, validation)| {
                QuestionKind::TextInput {
                    placeholder,
                    multiline,
                    validation,
                }
            }),
    ]
}

/// Generate an arbitrary QuestionGroup
fn arb_question_group() -> impl Strategy<Value = QuestionGroup> {
    (
        "[a-z]{1,10}",
        "[a-zA-Z ]{1,30}",
        proptest::collection::vec("[a-zA-Z]{1,10}", 1..=4),
    )
        .prop_map(|(id, prompt, options)| QuestionGroup {
            id,
            prompt,
            options,
        })
}

/// Generate an arbitrary Decision
fn arb_decision() -> impl Strategy<Value = Decision> {
    prop_oneof![
        // UseTools
        ("[a-z_]{1,15}", arb_json_value()).prop_map(|(tool_name, arguments)| {
            Decision::UseTools { calls: vec![ToolCallRecord {
                call_id: "proptest".to_string(),
                tool_name,
                arguments,
            }]}
        }),
        // AskUser
        (
            "[a-zA-Z ?]{1,30}",
            proptest::option::of(proptest::collection::vec("[a-zA-Z]{1,10}", 1..=4))
        )
            .prop_map(|(question, options)| Decision::AskUser { question, options }),
        // AskUserMultigroup
        (
            "[a-zA-Z ?]{1,30}",
            proptest::collection::vec(arb_question_group(), 1..=3)
        )
            .prop_map(|(question, groups)| Decision::AskUserMultigroup { question, groups }),
        // AskUserRich
        (
            "[a-zA-Z ?]{1,30}",
            arb_question_kind(),
            proptest::option::of("[a-z]{1,10}")
        )
            .prop_map(|(question, kind, question_id)| {
                Decision::AskUserRich {
                    question,
                    kind,
                    question_id,
                }
            }),
        // Complete
        "[a-zA-Z ]{1,30}".prop_map(|summary| Decision::Complete { summary }),
        // Fail
        "[a-zA-Z ]{1,30}".prop_map(|reason| Decision::Fail { reason }),
        // Silent
        Just(Decision::Silent),
        // HeartbeatOk
        Just(Decision::HeartbeatOk),
    ]
}

/// Generate an arbitrary UserAnswer
fn arb_user_answer() -> impl Strategy<Value = UserAnswer> {
    prop_oneof![
        any::<bool>().prop_map(|confirmed| UserAnswer::Confirmation { confirmed }),
        (0..10usize, "[a-zA-Z ]{1,15}").prop_map(|(idx, label)| UserAnswer::SingleChoice {
            selected_index: idx,
            selected_label: label,
        }),
        (
            proptest::collection::vec(0..10usize, 0..=4),
            proptest::collection::vec("[a-zA-Z]{1,10}", 0..=4)
        )
            .prop_map(|(indices, labels)| UserAnswer::MultiChoice {
                selected_indices: indices,
                selected_labels: labels,
            }),
        "[a-zA-Z ]{0,30}".prop_map(|text| UserAnswer::TextInput { text }),
        Just(UserAnswer::Cancelled),
    ]
}

/// Generate an arbitrary ActionResult
fn arb_action_result() -> impl Strategy<Value = ActionResult> {
    prop_oneof![
        // ToolResults (Success)
        (arb_json_value(), any::<u64>()).prop_map(|(output, duration_ms)| {
            ActionResult::ToolResults { results: vec![ToolCallResult {
                call_id: "proptest".to_string(),
                tool_name: "proptest_tool".to_string(),
                result: SingleToolResult::Success { output, duration_ms },
            }]}
        }),
        // ToolResults (Error)
        ("[a-zA-Z ]{1,30}", any::<bool>()).prop_map(|(error, retryable)| {
            ActionResult::ToolResults { results: vec![ToolCallResult {
                call_id: "proptest".to_string(),
                tool_name: "proptest_tool".to_string(),
                result: SingleToolResult::Error { error, retryable },
            }]}
        }),
        // UserResponse
        "[a-zA-Z ]{1,30}".prop_map(|response| ActionResult::UserResponse { response }),
        // UserResponseRich
        arb_user_answer()
            .prop_map(|response| ActionResult::UserResponseRich { response }),
        // Completed
        Just(ActionResult::Completed),
        // Failed
        Just(ActionResult::Failed),
    ]
}

/// Generate an arbitrary Action
fn arb_action() -> impl Strategy<Value = Action> {
    prop_oneof![
        // ToolCalls
        ("[a-z_]{1,15}", arb_json_value()).prop_map(|(tool_name, arguments)| Action::ToolCalls { calls: vec![ToolCallRequest {
            call_id: "proptest".to_string(),
            tool_name,
            arguments,
        }]}),
        // UserInteraction
        (
            "[a-zA-Z ?]{1,30}",
            proptest::option::of(proptest::collection::vec("[a-zA-Z]{1,10}", 1..=4))
        )
            .prop_map(|(question, options)| Action::UserInteraction { question, options }),
        // UserInteractionMultigroup
        (
            "[a-zA-Z ?]{1,30}",
            proptest::collection::vec(arb_question_group(), 1..=3)
        )
            .prop_map(|(question, groups)| Action::UserInteractionMultigroup { question, groups }),
        // UserInteractionRich
        (
            "[a-zA-Z ?]{1,30}",
            arb_question_kind(),
            proptest::option::of("[a-z]{1,10}")
        )
            .prop_map(|(question, kind, question_id)| {
                Action::UserInteractionRich {
                    question,
                    kind,
                    question_id,
                }
            }),
        // Completion
        "[a-zA-Z ]{1,30}".prop_map(|summary| Action::Completion { summary }),
        // Failure
        "[a-zA-Z ]{1,30}".prop_map(|reason| Action::Failure { reason }),
    ]
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// Decision serde roundtrip: serialize → deserialize preserves value
    #[test]
    fn decision_serde_roundtrip(decision in arb_decision()) {
        let json = serde_json::to_string(&decision).unwrap();
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&parsed, &decision);
    }

    /// ActionResult serde roundtrip: serialize → deserialize preserves value
    #[test]
    fn action_result_serde_roundtrip(result in arb_action_result()) {
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ActionResult = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&parsed, &result);
    }

    /// Action serde roundtrip: serialize → deserialize preserves value
    #[test]
    fn action_serde_roundtrip(action in arb_action()) {
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&parsed, &action);
    }

    /// Terminal decision classification:
    /// Complete, Fail, Silent, HeartbeatOk are terminal;
    /// UseTool, AskUser, AskUserMultigroup, AskUserRich are non-terminal.
    #[test]
    fn terminal_decision_classification(decision in arb_decision()) {
        let is_terminal = decision.is_terminal();
        match &decision {
            Decision::Complete { .. }
            | Decision::Fail { .. }
            | Decision::Silent
            | Decision::HeartbeatOk => {
                prop_assert!(is_terminal, "Expected terminal for {:?}", decision);
            }
            Decision::UseTools { .. }
            | Decision::AskUser { .. }
            | Decision::AskUserMultigroup { .. }
            | Decision::AskUserRich { .. } => {
                prop_assert!(!is_terminal, "Expected non-terminal for {:?}", decision);
            }
        }
    }

    /// ActionResult success classification:
    /// ToolSuccess, UserResponse, UserResponseRich, Completed are success;
    /// ToolError and Failed are not success.
    #[test]
    fn action_result_success_classification(result in arb_action_result()) {
        let is_success = result.is_success();
        match &result {
            ActionResult::ToolResults { ref results } => {
                let all_success = results.iter().all(|r| r.is_success());
                prop_assert_eq!(is_success, all_success, "ToolResults success mismatch for {:?}", result);
            }
            ActionResult::UserResponse { .. }
            | ActionResult::UserResponseRich { .. }
            | ActionResult::Completed => {
                prop_assert!(is_success, "Expected success for {:?}", result);
            }
            ActionResult::Failed => {
                prop_assert!(!is_success, "Expected not success for {:?}", result);
            }
        }
    }

    /// ActionResult retryable classification:
    /// Only ToolError { retryable: true, .. } is retryable.
    #[test]
    fn action_result_retryable_classification(result in arb_action_result()) {
        let is_retryable = result.is_retryable();
        match &result {
            ActionResult::ToolResults { ref results } if results.iter().any(|r| matches!(&r.result, SingleToolResult::Error { retryable: true, .. })) => {
                prop_assert!(is_retryable, "Expected retryable for {:?}", result);
            }
            _ => {
                prop_assert!(!is_retryable, "Expected not retryable for {:?}", result);
            }
        }
    }

    /// Decision → Action conversion preserves semantic meaning
    #[test]
    fn decision_to_action_conversion(decision in arb_decision()) {
        let action: Action = decision.clone().into();
        match &decision {
            Decision::UseTools { calls: ref records } => {
                match &action {
                    Action::ToolCalls { calls: ref requests } => {
                        prop_assert_eq!(requests.len(), records.len());
                        for (req, rec) in requests.iter().zip(records.iter()) {
                            prop_assert_eq!(&req.tool_name, &rec.tool_name);
                            prop_assert_eq!(&req.arguments, &rec.arguments);
                        }
                    }
                    _ => prop_assert!(false, "UseTools should convert to ToolCalls"),
                }
            }
            Decision::AskUser { question, options } => {
                match &action {
                    Action::UserInteraction { question: q, options: o } => {
                        prop_assert_eq!(q, question);
                        prop_assert_eq!(o, options);
                    }
                    _ => prop_assert!(false, "AskUser should convert to UserInteraction"),
                }
            }
            Decision::Complete { summary } => {
                match &action {
                    Action::Completion { summary: s } => {
                        prop_assert_eq!(s, summary);
                    }
                    _ => prop_assert!(false, "Complete should convert to Completion"),
                }
            }
            Decision::Fail { reason } => {
                match &action {
                    Action::Failure { reason: r } => {
                        prop_assert_eq!(r, reason);
                    }
                    _ => prop_assert!(false, "Fail should convert to Failure"),
                }
            }
            Decision::Silent => {
                match &action {
                    Action::Completion { summary } => {
                        prop_assert_eq!(summary, "[silent]");
                    }
                    _ => prop_assert!(false, "Silent should convert to Completion"),
                }
            }
            Decision::HeartbeatOk => {
                match &action {
                    Action::Completion { summary } => {
                        prop_assert_eq!(summary, "[heartbeat_ok]");
                    }
                    _ => prop_assert!(false, "HeartbeatOk should convert to Completion"),
                }
            }
            Decision::AskUserMultigroup { .. } => {
                let is_multigroup = matches!(action, Action::UserInteractionMultigroup { .. });
                prop_assert!(is_multigroup, "AskUserMultigroup should convert to UserInteractionMultigroup");
            }
            Decision::AskUserRich { .. } => {
                let is_rich = matches!(action, Action::UserInteractionRich { .. });
                prop_assert!(is_rich, "AskUserRich should convert to UserInteractionRich");
            }
        }
    }

    /// Action terminal classification:
    /// Completion and Failure are terminal; ToolCall, UserInteraction, etc. are not.
    #[test]
    fn action_terminal_classification(action in arb_action()) {
        let is_terminal = action.is_terminal();
        match &action {
            Action::Completion { .. } | Action::Failure { .. } => {
                prop_assert!(is_terminal, "Expected terminal for {:?}", action);
            }
            Action::ToolCalls { .. }
            | Action::UserInteraction { .. }
            | Action::UserInteractionMultigroup { .. }
            | Action::UserInteractionRich { .. } => {
                prop_assert!(!is_terminal, "Expected non-terminal for {:?}", action);
            }
        }
    }

    /// decision_type returns consistent, non-empty strings
    #[test]
    fn decision_type_non_empty(decision in arb_decision()) {
        let dt = decision.decision_type();
        prop_assert!(!dt.is_empty(), "decision_type() should not be empty");
    }

    /// action_type returns consistent, non-empty strings
    #[test]
    fn action_type_non_empty(action in arb_action()) {
        let at = action.action_type();
        prop_assert!(!at.is_empty(), "action_type() should not be empty");
    }
}
