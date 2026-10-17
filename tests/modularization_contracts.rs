//! Core-side identity tests for the `aleph-slot-api` migration (Task 1).
//!
//! These integration tests pin that the migration **did not break identity**.
//! `alephcore`'s re-export shims must point at the same Rust types as
//! `aleph_slot_api`, not at parallel definitions; otherwise `Result<_,
//! crate::error::AlephError>` and `Result<_, aleph_slot_api::AlephError>`
//! would stop composing and the 12+ call sites that already use the old
//! path would silently lose their type. They also pin that the `MissingInput`
//! formatter still produces the same 100-character soft cap that the brief
//! requires.
//!
//! Anything that fails here means the migration is broken at the type
//! level — not a behaviour red, an identity red.

use std::any::TypeId;

/// The `alephcore` re-export must be **the same** type as the
/// `aleph_slot_api` definition. `pub use` shares `TypeId`; a `pub type
/// Foo = Bar;` alias also shares `TypeId`; a **second definition** would
/// not. We check both directions: every value of the old path is also of
/// the new path, and vice versa.
#[test]
fn core_type_identity_matches_api() {
    // The four identity-bearing types.
    assert_eq!(
        TypeId::of::<alephcore::error::AlephError>(),
        TypeId::of::<aleph_slot_api::AlephError>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::error::AlephException>(),
        TypeId::of::<aleph_slot_api::AlephException>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::error::ErrorClass>(),
        TypeId::of::<aleph_slot_api::ErrorClass>(),
    );
    // `Result<T>` is a type alias, so `TypeId::of::<Result<()>>()` would
    // erase the alias anyway. We assert via the aliasing path instead:
    // a `Result<()>` from each side must be the same.
    fn alias_check() -> (alephcore::error::Result<()>, aleph_slot_api::Result<()>) {
        (Ok(()), Ok(()))
    }
    let _ = alias_check;

    // Construct a value via the **old** path and confirm it is also a
    // value of the **new** path. This is what would break if anyone
    // accidentally added a second `AlephError` definition here.
    let old_path: alephcore::error::AlephError = alephcore::error::AlephError::cancelled();
    // Move the value through the new path. This compiles only if the
    // old path and the new path resolve to the same Rust type (a
    // different `struct` or `enum` definition would not accept the
    // move). `AlephError` does not derive `Clone`, so we cannot round-
    // trip via `clone()` — the move itself is the proof.
    let new_path: aleph_slot_api::AlephError = old_path;
    // Round-trip through `class()` — only possible if both names resolve
    // to the same type and its method table.
    assert_eq!(new_path.class(), aleph_slot_api::ErrorClass::Recoverable);
}

/// `MissingInput::user_friendly_message` must use 100 CHARACTERS (not 100
/// bytes) when truncating, and the truncation marker must be present when
/// the message exceeds the budget. The brief is explicit: "不能改成 100
/// bytes".
#[test]
fn missing_input_truncates_at_100_chars_not_bytes() {
    // 100 chars / 300 bytes — under the char-based budget, no ellipsis.
    let exactly_100_chars: String = "中".repeat(100);
    let err = alephcore::error::AlephError::MissingInput {
        task_id: "task-1".to_string(),
        task_name: "测试".to_string(),
        message: exactly_100_chars.clone(),
    };
    let rendered = err.user_friendly_message();
    assert!(
        rendered.contains(&exactly_100_chars),
        "Message truncated at byte boundary; expected full 100 chars present: {rendered}"
    );

    // 101 chars — over the budget, must be cut at 100 chars + "...".
    let over_limit: String = "中".repeat(101);
    let err = alephcore::error::AlephError::MissingInput {
        task_id: "task-1".to_string(),
        task_name: "测试".to_string(),
        message: over_limit,
    };
    let rendered = err.user_friendly_message();
    assert!(
        rendered.contains("中..."),
        "Over-limit message not truncated at the 100-char marker: {rendered}"
    );
}

/// The four sibling truncate functions must still be defined here. They
/// were never moved; if someone accidentally wipes them during the
/// `truncate_text` re-export shim, the WeChat/env-cap call sites start
/// failing to compile. We do not assert their full behaviour (covered in
/// `src/utils/text_format.rs::tests`), only that they exist at the old
/// path and still produce strings.
#[test]
fn core_keeps_sibling_truncate_helpers() {
    // Each must accept the same signatures they had pre-migration.
    let _ = alephcore::utils::text_format::truncate_with_marker("hello", 2, "…");
    let _ = alephcore::utils::text_format::truncate_chars("hello", 2);
    let _ = alephcore::utils::text_format::truncate_reserving("hello", 4, "…");
    let _ = alephcore::utils::text_format::truncate_bytes("hello", 4);
}

/// The re-exported `truncate_text` at the old path is the **same**
/// function as `aleph_slot_api::text::truncate_text` — not a copy. We
/// compare function pointers; same address means same machine code, which
/// means the re-export shim is not silently re-defining the function.
///
/// Note: rustc warns that function pointer addresses are not guaranteed
/// to be unique across codegen units, so this is an *informational*
/// identity check, not a hard guarantee. In practice `pub use` aliases
/// the same symbol and the addresses do match under the standard
/// codegen. The compile-time fact — that `crate::utils::text_format::
/// truncate_text` resolves through the `pub use` to the API crate's
/// symbol — is the load-bearing guarantee; the address check is a
/// belt-and-suspenders sanity test.
#[test]
fn core_truncate_text_re_export_is_same_function_pointer() {
    #[allow(unpredictable_function_pointer_comparisons)]
    {
        let local: fn(&str, usize) -> String = alephcore::utils::text_format::truncate_text;
        let api: fn(&str, usize) -> String = aleph_slot_api::text::truncate_text;
        assert_eq!(
            local as usize, api as usize,
            "Re-export shim is not actually a `pub use`"
        );
    }
}

/// `AlephException`'s three `From` impls must continue to work through
/// the old re-export. Specifically: callers in alephcore convert an
/// `AlephError` into an `AlephException` to throw across the UniFFI
/// boundary; that path must still go through the API crate without any
/// second `impl From` here (Rust would refuse two `From` impls for the
/// same pair anyway, but the value-side check is what catches a wrong
/// body).
#[test]
fn exception_from_aleph_error_collapses_through_re_export() {
    let ex: alephcore::error::AlephException = alephcore::error::AlephError::cancelled().into();
    // `AlephException` deliberately does NOT derive `PartialEq` (the
    // UniFFI 0.25 surface keeps it minimal), so use `matches!` for the
    // variant check.
    assert!(matches!(ex, aleph_slot_api::AlephException::Error));
    assert!(matches!(ex, alephcore::error::AlephException::Error));
}

// =========================================================================
// Task 2: tool contract / taxonomy / descriptor identity
// =========================================================================

/// Every Task 2 type reachable at its old Core path must be **the same**
/// Rust type as the `aleph_slot_api` definition (same discipline as the
/// Task 1 identity test above: `pub use` shares `TypeId`, a second
/// definition would not).
#[test]
fn task2_type_identity_matches_api() {
    // tool.rs — loop-side contract types.
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolError>(),
        TypeId::of::<aleph_slot_api::tool::ToolError>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::service::ToolError>(),
        TypeId::of::<aleph_slot_api::tool::ToolError>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolDefinition>(),
        TypeId::of::<aleph_slot_api::tool::ToolDefinition>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolDefinitionMetadata>(),
        TypeId::of::<aleph_slot_api::tool::ToolDefinitionMetadata>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::service::RefusedBy>(),
        TypeId::of::<aleph_slot_api::tool::RefusedBy>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolSource>(),
        TypeId::of::<aleph_slot_api::descriptor::ToolSource>(),
    );

    // session/events.rs — the three migrated event payload types.
    assert_eq!(
        TypeId::of::<alephcore::session::events::ToolOutput>(),
        TypeId::of::<aleph_slot_api::tool::ToolOutput>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::session::events::ToolOutputMetadata>(),
        TypeId::of::<aleph_slot_api::tool::ToolOutputMetadata>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::session::events::ToolImage>(),
        TypeId::of::<aleph_slot_api::tool::ToolImage>(),
    );

    // descriptor.rs — capability contract types.
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolCapabilityDescriptor>(),
        TypeId::of::<aleph_slot_api::descriptor::ToolCapabilityDescriptor>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::DescriptorError>(),
        TypeId::of::<aleph_slot_api::descriptor::DescriptorError>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ReplayPolicy>(),
        TypeId::of::<aleph_slot_api::descriptor::ReplayPolicy>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ReplayContractFingerprint>(),
        TypeId::of::<aleph_slot_api::descriptor::ReplayContractFingerprint>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ImplementationContract>(),
        TypeId::of::<aleph_slot_api::descriptor::ImplementationContract>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolCallIdentity>(),
        TypeId::of::<aleph_slot_api::descriptor::ToolCallIdentity>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::ToolKind>(),
        TypeId::of::<aleph_slot_api::descriptor::ToolKind>(),
    );

    // error_kind.rs — taxonomy, both old paths.
    assert_eq!(
        TypeId::of::<alephcore::tools::error_kind::ToolErrorKind>(),
        TypeId::of::<aleph_slot_api::error_kind::ToolErrorKind>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::service::ToolErrorKind>(),
        TypeId::of::<aleph_slot_api::error_kind::ToolErrorKind>(),
    );

    // concurrency.rs — scheduling claim types.
    assert_eq!(
        TypeId::of::<alephcore::tools::concurrency::ConcurrencyClaim>(),
        TypeId::of::<aleph_slot_api::concurrency::ConcurrencyClaim>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::concurrency::ExclusiveScope>(),
        TypeId::of::<aleph_slot_api::concurrency::ExclusiveScope>(),
    );

    // handlers/mod.rs — handler trait + filter (trait object identity).
    assert_eq!(
        TypeId::of::<dyn alephcore::tools::handlers::ToolHandler>(),
        TypeId::of::<dyn aleph_slot_api::handler::ToolHandler>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tools::handlers::McpServerFilter>(),
        TypeId::of::<aleph_slot_api::handler::McpServerFilter>(),
    );

    // metadata.rs — catalog/metadata types via the tool_metadata facade.
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::UnifiedTool>(),
        TypeId::of::<aleph_slot_api::metadata::UnifiedTool>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolSource>(),
        TypeId::of::<aleph_slot_api::metadata::ToolSource>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolCategory>(),
        TypeId::of::<aleph_slot_api::metadata::ToolCategory>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolPriority>(),
        TypeId::of::<aleph_slot_api::metadata::ToolPriority>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolSafetyLevel>(),
        TypeId::of::<aleph_slot_api::metadata::ToolSafetyLevel>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolDefinition>(),
        TypeId::of::<aleph_slot_api::metadata::ToolDefinition>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ToolSourceType>(),
        TypeId::of::<aleph_slot_api::metadata::ToolSourceType>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::tool_metadata::ChannelType>(),
        TypeId::of::<aleph_slot_api::metadata::ChannelType>(),
    );
}

/// Bidirectional move: a value constructed through the old Core path must
/// be assignable to the new API path and back. This is the compile-time
/// proof that no parallel definition exists for the migrated types.
#[test]
fn task2_old_new_bidirectional_assignment() {
    // UnifiedTool: build via old path, move to new path, move back.
    let old: alephcore::tool_metadata::UnifiedTool = alephcore::tool_metadata::UnifiedTool::new(
        "id-1".to_string(),
        "search".to_string(),
        "desc".to_string(),
        alephcore::tool_metadata::ToolSource::Builtin,
    );
    let new: aleph_slot_api::metadata::UnifiedTool = old;
    let _back: alephcore::tool_metadata::UnifiedTool = new;

    // ToolDefinition: same in both directions, including the descriptor
    // ToolSource payload.
    let old_def = alephcore::tools::ToolDefinition {
        name: "search".to_string(),
        description: "desc".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        source: alephcore::tools::ToolSource::Builtin,
        metadata: alephcore::tools::ToolDefinitionMetadata::default(),
    };
    let new_def: aleph_slot_api::tool::ToolDefinition = old_def;
    let _back: alephcore::tools::ToolDefinition = new_def;

    // ToolError: construct via old path, consume via new path method table.
    let old_err = alephcore::tools::ToolError::Cancelled {
        name: "search".to_string(),
    };
    let new_err: aleph_slot_api::tool::ToolError = old_err;
    assert_eq!(
        new_err.kind(),
        aleph_slot_api::error_kind::ToolErrorKind::Cancelled
    );

    // ToolOutput: event payload moves between session path and API path.
    let old_out = alephcore::session::events::ToolOutput {
        value: serde_json::json!("hello"),
        metadata: alephcore::session::events::ToolOutputMetadata::default(),
    };
    let new_out: aleph_slot_api::tool::ToolOutput = old_out;
    let _back: alephcore::session::events::ToolOutput = new_out;
}

/// The cross-batch refusal cause is one constant: the thinker path
/// re-export must be the same bytes as the API definition (pinned lowercase
/// prose; a re-worded copy would split the run-summary fingerprinting).
#[test]
fn cross_batch_refused_cause_is_single_definition() {
    assert_eq!(
        alephcore::thinker::nudges::CROSS_BATCH_REFUSED_CAUSE,
        aleph_slot_api::error_kind::CROSS_BATCH_REFUSED_CAUSE,
    );
    assert_eq!(
        alephcore::tools::error_kind::CROSS_BATCH_REFUSED_CAUSE,
        "this exact call already failed earlier in the run and was not run again; \
         the earlier error stands — read it before deciding what to do next"
    );
}

/// The descriptor constants keep their original types and values
/// (u32 schema version, 32-byte digest, u8 fingerprint version 1).
#[test]
fn descriptor_constants_pin_original_types_and_values() {
    let schema_version: u32 = alephcore::tools::descriptor::SCHEMA_VERSION;
    assert_eq!(schema_version, 1);
    assert_eq!(alephcore::tools::descriptor::MAX_NAME_LEN, 64);
    assert_eq!(alephcore::tools::descriptor::REPLAY_CONTRACT_DIGEST_LEN, 32);
    let fp_version: u8 = alephcore::tools::descriptor::REPLAY_CONTRACT_FINGERPRINT_VERSION;
    assert_eq!(fp_version, 1);
}

/// `ToolCallIdentity` must stay `Copy` (session reduction and the replay
/// adapter move it out of borrows).
#[test]
fn tool_call_identity_is_copy() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<alephcore::tools::ToolCallIdentity>();
    assert_copy::<aleph_slot_api::descriptor::ToolCallIdentity>();
    assert_copy::<alephcore::tools::ReplayContractFingerprint>();
}

// =========================================================================
// Task 3: search / embedding contract identity
// =========================================================================

/// Every Task 3 type reachable at its old Core path must be **the same**
/// Rust type as the `aleph_slot_api` definition (same discipline as the
/// Task 1/2 identity tests: `pub use` shares `TypeId`, a second definition
/// would not). For the two traits the *trait object* identity is what
/// matters — every Core provider/handle stores `Arc<dyn SearchProvider>` /
/// `Arc<dyn EmbeddingProvider>` behind the old path.
#[test]
fn task3_type_identity_matches_api() {
    // search: options / recency / result / capabilities.
    assert_eq!(
        TypeId::of::<alephcore::search::SearchOptions>(),
        TypeId::of::<aleph_slot_api::search::SearchOptions>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::search::Recency>(),
        TypeId::of::<aleph_slot_api::search::Recency>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::search::SearchResult>(),
        TypeId::of::<aleph_slot_api::search::SearchResult>(),
    );
    assert_eq!(
        TypeId::of::<alephcore::search::SearchCapabilities>(),
        TypeId::of::<aleph_slot_api::search::SearchCapabilities>(),
    );

    // The provider trait object, both directions.
    assert_eq!(
        TypeId::of::<dyn alephcore::search::SearchProvider>(),
        TypeId::of::<dyn aleph_slot_api::search::SearchProvider>(),
    );

    // embedding: the trait object via both old paths (the module path and
    // the `memory` facade re-export) and the new one.
    assert_eq!(
        TypeId::of::<dyn alephcore::memory::embedding_provider::EmbeddingProvider>(),
        TypeId::of::<dyn aleph_slot_api::embedding::EmbeddingProvider>(),
    );
    assert_eq!(
        TypeId::of::<dyn alephcore::memory::EmbeddingProvider>(),
        TypeId::of::<dyn aleph_slot_api::embedding::EmbeddingProvider>(),
    );
}

/// Bidirectional move: a value constructed through the old Core path must
/// be assignable to the new API path and back — the compile-time proof
/// that no parallel definition exists. Includes the trait-object coercion:
/// an `Arc<dyn ...>` built through the old path must coerce through the
/// new path without a cast.
#[test]
fn task3_old_new_bidirectional_assignment() {
    // SearchOptions: build via old path, move to new, move back.
    let old: alephcore::search::SearchOptions = alephcore::search::SearchOptions {
        recency: Some(alephcore::search::Recency::Week),
        ..Default::default()
    };
    let new: aleph_slot_api::search::SearchOptions = old;
    let _back: alephcore::search::SearchOptions = new;

    // SearchResult: the exact constructor through the old path, then move.
    let old: alephcore::search::SearchResult =
        alephcore::search::SearchResult::new("t".to_string(), "u".to_string(), "s".to_string());
    let new: aleph_slot_api::search::SearchResult = old;
    let _back: alephcore::search::SearchResult = new;

    // Recency + SearchCapabilities are Copy — the assignment still pins
    // that both paths name the same type.
    let old: alephcore::search::Recency = alephcore::search::Recency::Year;
    let new: aleph_slot_api::search::Recency = old;
    let _back: alephcore::search::Recency = new;
    let old: alephcore::search::SearchCapabilities = Default::default();
    let new: aleph_slot_api::search::SearchCapabilities = old;
    let _back: alephcore::search::SearchCapabilities = new;

    // The trait objects: an implementor compiled against the OLD path must
    // be storable behind the NEW path's `dyn` (and vice versa) — only true
    /// when both names resolve to one trait.
    struct Stub;
    #[async_trait::async_trait]
    impl alephcore::search::SearchProvider for Stub {
        async fn search(
            &self,
            _query: &str,
            _options: &alephcore::search::SearchOptions,
        ) -> alephcore::error::Result<Vec<alephcore::search::SearchResult>> {
            Ok(vec![])
        }
        fn name(&self) -> &str {
            "stub"
        }
        fn is_available(&self) -> bool {
            false
        }
    }
    let via_old: std::sync::Arc<dyn alephcore::search::SearchProvider> = std::sync::Arc::new(Stub);
    let via_new: std::sync::Arc<dyn aleph_slot_api::search::SearchProvider> = via_old;
    let _back: std::sync::Arc<dyn alephcore::search::SearchProvider> = via_new;

    struct StubEmbed;
    #[async_trait::async_trait]
    impl alephcore::memory::EmbeddingProvider for StubEmbed {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, alephcore::error::AlephError> {
            Ok(vec![])
        }
        async fn embed_batch(
            &self,
            _texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, alephcore::error::AlephError> {
            Ok(vec![])
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }
    let via_old: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider> =
        std::sync::Arc::new(StubEmbed);
    let via_new: std::sync::Arc<dyn aleph_slot_api::embedding::EmbeddingProvider> = via_old;
    let _back: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider> = via_new;
}

/// The `max_results` ceiling is one constant behind two paths — the value
/// (100) is the wire contract the Panel boundary was built on.
#[test]
fn task3_max_search_results_constant_is_shared() {
    let old: usize = alephcore::search::MAX_SEARCH_RESULTS;
    let new: usize = aleph_slot_api::search::MAX_SEARCH_RESULTS;
    assert_eq!(old, new);
    assert_eq!(old, 100);
}

// =========================================================================
// Task 4: channel / cancellation / approval / webhook contract identity
// =========================================================================

#[test]
fn task4_type_identity_and_bidirectional_assignment() {
    // Each conversion is checked by the compiler even without constructing
    // every value. It also prevents a parallel definition with similar fields.
    macro_rules! same_type {
        ($old:ty, $new:ty) => {{
            assert_eq!(TypeId::of::<$old>(), TypeId::of::<$new>());
            fn round_trip(value: $old) -> $old {
                let converted_new: $new = value;
                let converted_old: $old = converted_new;
                converted_old
            }
            let _ = round_trip;
        }};
    }
    macro_rules! channel_type {
        ($name:ident) => {
            same_type!(
                alephcore::gateway::channel::$name,
                aleph_slot_api::channel::$name
            );
        };
    }
    channel_type!(ChannelId);
    channel_type!(ConversationId);
    channel_type!(MessageId);
    channel_type!(UserId);
    channel_type!(InboundMessage);
    channel_type!(OutboundMessage);
    channel_type!(Attachment);
    channel_type!(InlineKeyboard);
    channel_type!(InlineKeyboardRow);
    channel_type!(InlineButton);
    channel_type!(ChannelCapabilities);
    channel_type!(StreamProtocol);
    channel_type!(ChannelInfo);
    channel_type!(ChannelStatus);
    channel_type!(SendResult);
    channel_type!(ChannelHealth);
    channel_type!(HealthStatus);
    channel_type!(ChannelState);
    channel_type!(InboundMessageSender);
    channel_type!(ChannelError);
    channel_type!(ChannelConfig);
    channel_type!(CallbackQuery);
    channel_type!(ConversationKind);
    channel_type!(ConversationRef);
    channel_type!(ConversationPage);
    channel_type!(MessageMeta);
    channel_type!(PairingData);
    same_type!(
        alephcore::gateway::channel::ChannelResult<()>,
        aleph_slot_api::channel::ChannelResult<()>
    );
    same_type!(
        alephcore::gateway::cancellation::CancellationToken,
        aleph_slot_api::cancellation::CancellationToken
    );
    macro_rules! channel_approval_type {
        ($name:ident) => {
            same_type!(
                alephcore::gateway::channel_approval::$name,
                aleph_slot_api::approval::$name
            );
        };
    }
    channel_approval_type!(ApprovalAction);
    channel_approval_type!(AuthorizationResult);
    channel_approval_type!(RenderedApproval);
    channel_approval_type!(PendingApproval);
    same_type!(
        alephcore::exec::socket::ApprovalDecisionType,
        aleph_slot_api::approval::ApprovalDecisionType
    );
    same_type!(
        alephcore::exec::approval::types::ApprovalRequest,
        aleph_slot_api::approval::ApprovalRequest
    );
    same_type!(
        alephcore::exec::approval::types::CommandApprovalRequest,
        aleph_slot_api::approval::CommandApprovalRequest
    );
    same_type!(
        alephcore::sandbox::exec_approval::gate::ApprovalOutcome,
        aleph_slot_api::approval::ApprovalOutcome
    );
    same_type!(
        alephcore::sandbox::exec_approval::gate::ApprovalResponse,
        aleph_slot_api::approval::ApprovalResponse
    );
    same_type!(
        alephcore::sandbox::exec_approval::grants::GrantScope,
        aleph_slot_api::approval::GrantScope
    );
}

#[test]
fn task4_trait_object_identity_and_bidirectional_assignment() {
    macro_rules! same_trait {
        ($old:path, $new:path) => {{
            assert_eq!(TypeId::of::<dyn $old>(), TypeId::of::<dyn $new>());
            fn round_trip(value: std::sync::Arc<dyn $old>) -> std::sync::Arc<dyn $old> {
                let converted_new: std::sync::Arc<dyn $new> = value;
                let converted_old: std::sync::Arc<dyn $old> = converted_new;
                converted_old
            }
            let _ = round_trip;
        }};
    }
    same_trait!(
        alephcore::gateway::channel::Channel,
        aleph_slot_api::channel::Channel
    );
    same_trait!(
        alephcore::gateway::channel::ChannelFactory,
        aleph_slot_api::channel::ChannelFactory
    );
    same_trait!(
        alephcore::gateway::channel::NativeStreamHandler,
        aleph_slot_api::channel::NativeStreamHandler
    );
    same_trait!(
        alephcore::gateway::channel_approval::ChannelApprovalCapability,
        aleph_slot_api::approval::ChannelApprovalCapability
    );
    same_trait!(
        alephcore::gateway::channel_approval::ChannelApprovalExt,
        aleph_slot_api::approval::ChannelApprovalExt
    );
    same_trait!(
        alephcore::gateway::webhook_receiver::WebhookHandler,
        aleph_slot_api::webhook::WebhookHandler
    );
}

#[test]
fn task4_webhook_http_types_match_axum() {
    // Implement the API trait with axum's re-exported signature types. This
    // compiles only when the trait's http::HeaderMap/bytes::Bytes are identical.
    struct Handler;
    #[async_trait::async_trait]
    impl aleph_slot_api::webhook::WebhookHandler for Handler {
        fn verify(&self, _headers: &axum::http::HeaderMap, _body: &[u8]) -> bool {
            true
        }
        async fn handle(
            &self,
            _headers: &axum::http::HeaderMap,
            _body: axum::body::Bytes,
        ) -> aleph_slot_api::channel::ChannelResult<Vec<aleph_slot_api::channel::InboundMessage>>
        {
            Ok(vec![])
        }
        fn path(&self) -> &str {
            "/webhook/identity"
        }
    }
    let old: std::sync::Arc<dyn alephcore::gateway::webhook_receiver::WebhookHandler> =
        std::sync::Arc::new(Handler);
    let new: std::sync::Arc<dyn aleph_slot_api::webhook::WebhookHandler> = old;
    let back: std::sync::Arc<dyn alephcore::gateway::webhook_receiver::WebhookHandler> = new;
    assert!(back.verify(&axum::http::HeaderMap::new(), b"payload"));

    let old: axum::body::Bytes = axum::body::Bytes::from_static(b"payload");
    let new: bytes::Bytes = old;
    let back: axum::body::Bytes = new;
    assert_eq!(back.as_ref(), b"payload");
    assert_eq!(
        TypeId::of::<axum::body::Bytes>(),
        TypeId::of::<bytes::Bytes>()
    );
}

#[test]
fn task5_legacy_hook_identity_and_core_config_defaults() {
    use aleph_slot_api::hooks as new;
    use alephcore::extension as old;

    macro_rules! same_type {
        ($name:ident) => {
            assert_eq!(TypeId::of::<old::$name>(), TypeId::of::<new::$name>());
        };
    }
    same_type!(HookEvent);
    same_type!(MatchSubject);
    same_type!(HookKind);
    same_type!(HookPriority);
    same_type!(HookAction);
    for event in old::HookEvent::ALL {
        let value: new::HookEvent = event;
        let back: old::HookEvent = value;
        assert_eq!(event, back);
    }
    let subject: new::MatchSubject = old::HookEvent::SessionStart.match_subject();
    let back: old::MatchSubject = subject;
    assert_eq!(back.as_str(), "session_source");
    let kind: new::HookKind = old::HookKind::Interceptor;
    let back: old::HookKind = kind;
    assert_eq!(back, old::HookKind::Interceptor);
    let priority: new::HookPriority = old::HookPriority::High;
    let back: old::HookPriority = priority;
    assert_eq!(back.as_i32(), -100);
    let action: new::HookAction = old::HookAction::Command {
        command: "literal".into(),
    };
    let back: old::HookAction = action;
    assert_eq!(
        serde_json::to_value(back).unwrap(),
        serde_json::json!({"type":"command","command":"literal"})
    );
    assert_eq!(old::SESSION_SOURCES_FIRED, new::SESSION_SOURCES_FIRED);
    assert_eq!(old::SESSION_SOURCE_STARTUP, new::SESSION_SOURCE_STARTUP);

    let fixture = serde_json::json!({"event":"PreToolUse","actions":[{"type":"command","command":"c"}],"scope_key":{"kind":"global"}});
    // ScopeKey's exact legacy serde is sourced from its existing constructor.
    let mut fixture = fixture;
    fixture["scope_key"] =
        serde_json::to_value(alephcore::extension::visibility::ScopeKey::Global).unwrap();
    let config: old::HookConfig = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(config.kind, new::HookKind::Observer);
    assert_eq!(config.priority, new::HookPriority::Normal);
    assert!(config.plugin_name.is_empty());
    assert!(config.plugin_root.as_os_str().is_empty());
    assert!(config.matcher.is_none());
    assert!(config.handler.is_none());
    assert!(config.timeout_secs.is_none());
    assert!(config.declared_event.is_none());
    assert_eq!(config.event_name(), "before_tool_call");
    fixture.as_object_mut().unwrap().remove("scope_key");
    assert!(serde_json::from_value::<old::HookConfig>(fixture).is_err());
}

#[test]
fn task4_approval_and_webhook_constants_use_old_paths() {
    assert_eq!(
        alephcore::gateway::channel::CB_MESSAGE_ID_PREFIX,
        aleph_slot_api::channel::CB_MESSAGE_ID_PREFIX
    );
    assert_eq!(alephcore::gateway::channel::CB_MESSAGE_ID_PREFIX, "cb_");
    assert_eq!(
        alephcore::gateway::webhook_receiver::WEBHOOK_ROUTE_PREFIX,
        aleph_slot_api::webhook::WEBHOOK_ROUTE_PREFIX
    );
    assert_eq!(
        alephcore::gateway::webhook_receiver::WEBHOOK_ROUTE_PREFIX,
        "/webhook"
    );
    assert_eq!(
        alephcore::exec::allowed_decisions::full_set(),
        aleph_slot_api::approval::full_set()
    );
}

// =========================================================================
// Task 7: AI DTO identity; the legacy and pure provider TRAITS stay distinct.
// =========================================================================

#[test]
fn task7_ai_dto_identity_through_old_module_and_api_root_exports() {
    macro_rules! same_type {
        ($old:ty, $module:ty, $root:ty) => {{
            assert_eq!(TypeId::of::<$old>(), TypeId::of::<$module>());
            assert_eq!(TypeId::of::<$module>(), TypeId::of::<$root>());
            fn round_trip(value: $old) -> $old {
                let converted_module: $module = value;
                let converted_root: $root = converted_module;
                let converted_old: $old = converted_root;
                converted_old
            }
            let _ = round_trip;
        }};
    }
    same_type!(
        alephcore::providers::adapter::ToolChoice,
        aleph_slot_api::ai::ToolChoice,
        aleph_slot_api::ToolChoice
    );
    same_type!(
        alephcore::providers::adapter::RequestPayload<'static>,
        aleph_slot_api::ai::RequestPayload<'static>,
        aleph_slot_api::RequestPayload<'static>
    );
    same_type!(
        alephcore::thinker::prompt_builder::SystemPromptPart,
        aleph_slot_api::ai::SystemPromptPart,
        aleph_slot_api::SystemPromptPart
    );
    same_type!(
        alephcore::agents::thinking::ThinkLevel,
        aleph_slot_api::ai::ThinkLevel,
        aleph_slot_api::ThinkLevel
    );
    same_type!(
        alephcore::agents::ThinkLevel,
        aleph_slot_api::ai::ThinkLevel,
        aleph_slot_api::ThinkLevel
    );
    macro_rules! message_type {
        ($name:ident) => {
            same_type!(
                alephcore::providers::message::$name,
                aleph_slot_api::ai_message::$name,
                aleph_slot_api::$name
            );
        };
    }
    message_type!(UnifiedMessage);
    message_type!(ContentBlock);
    message_type!(CacheControl);
    message_type!(EphemeralTtl);
    macro_rules! response_type {
        ($name:ident) => {
            same_type!(
                alephcore::providers::adapter::$name,
                aleph_slot_api::ai_response::$name,
                aleph_slot_api::$name
            );
        };
    }
    response_type!(ProviderResponse);
    response_type!(NativeToolCall);
    response_type!(StopReason);
    response_type!(TokenCost);
    response_type!(TokenUsage);
    // Preserve the additional old convenience exports that actually exist.
    same_type!(
        alephcore::providers::ProviderResponse,
        aleph_slot_api::ai_response::ProviderResponse,
        aleph_slot_api::ProviderResponse
    );
    same_type!(
        alephcore::providers::NativeToolCall,
        aleph_slot_api::ai_response::NativeToolCall,
        aleph_slot_api::NativeToolCall
    );
    same_type!(
        alephcore::providers::StopReason,
        aleph_slot_api::ai_response::StopReason,
        aleph_slot_api::StopReason
    );
    same_type!(
        alephcore::providers::TokenUsage,
        aleph_slot_api::ai_response::TokenUsage,
        aleph_slot_api::TokenUsage
    );
    same_type!(
        alephcore::providers::RequestPayload<'static>,
        aleph_slot_api::ai::RequestPayload<'static>,
        aleph_slot_api::RequestPayload<'static>
    );
    same_type!(
        alephcore::providers::delta::ProviderDelta,
        aleph_slot_api::ai_delta::ProviderDelta,
        aleph_slot_api::ProviderDelta
    );
    same_type!(
        alephcore::providers::ProviderDelta,
        aleph_slot_api::ai_delta::ProviderDelta,
        aleph_slot_api::ProviderDelta
    );
    same_type!(
        alephcore::providers::delta::NoopSink,
        aleph_slot_api::ai_delta::NoopSink,
        aleph_slot_api::NoopSink
    );
    same_type!(
        alephcore::providers::NoopSink,
        aleph_slot_api::ai_delta::NoopSink,
        aleph_slot_api::NoopSink
    );
    same_type!(
        alephcore::providers::reasoning_replay::ThinkingOrigin,
        aleph_slot_api::ai_reasoning::ThinkingOrigin,
        aleph_slot_api::ThinkingOrigin
    );
    same_type!(
        alephcore::providers::reasoning_replay::ReasoningReplay,
        aleph_slot_api::ai_reasoning::ReasoningReplay,
        aleph_slot_api::ReasoningReplay
    );
    // RequestPayload.tools is the metadata owner, never execution ToolDefinition.
    same_type!(
        alephcore::tool_metadata::ToolDefinition,
        aleph_slot_api::metadata::ToolDefinition,
        aleph_slot_api::MetadataToolDefinition
    );
    same_type!(
        alephcore::ToolDefinition,
        aleph_slot_api::metadata::ToolDefinition,
        aleph_slot_api::MetadataToolDefinition
    );
}

#[test]
fn task7_borrowed_payload_round_trip_preserves_metadata_tool_identity() {
    // Unlike TypeId's 'static instantiation, this function accepts any caller
    // lifetime and moves a genuinely local payload through every old/new path.
    fn round_trip<'a>(
        old: alephcore::providers::adapter::RequestPayload<'a>,
    ) -> alephcore::providers::RequestPayload<'a> {
        let module: aleph_slot_api::ai::RequestPayload<'a> = old;
        let root: aleph_slot_api::RequestPayload<'a> = module;
        let converted_old: alephcore::providers::RequestPayload<'a> = root;
        converted_old
    }
    let messages = vec![alephcore::providers::message::UnifiedMessage::user(
        "local history",
    )];
    let system = String::from("local system");
    let blocks = vec![alephcore::thinker::prompt_builder::SystemPromptPart {
        content: "cached".into(),
        cache: true,
    }];
    let old_tool = alephcore::ToolDefinition::new(
        "lookup",
        "metadata",
        serde_json::json!({"type":"object"}),
        alephcore::ToolCategory::Builtin,
    )
    .with_strict(true);
    let api_tool: aleph_slot_api::metadata::ToolDefinition = old_tool;
    let converted_old_tool: alephcore::tool_metadata::ToolDefinition = api_tool;
    let tools = vec![converted_old_tool];
    let old = alephcore::providers::adapter::RequestPayload::new(&messages)
        .with_system(Some(&system))
        .with_system_blocks(Some(&blocks))
        .with_tools(Some(&tools))
        .with_think_level(Some(alephcore::agents::ThinkLevel::High));
    let returned = round_trip(old);
    assert!(std::ptr::eq(returned.messages, messages.as_slice()));
    assert!(std::ptr::eq(
        returned.system_prompt.unwrap(),
        system.as_str()
    ));
    assert!(std::ptr::eq(
        returned.system_blocks.unwrap(),
        blocks.as_slice()
    ));
    let borrowed_tools: &[aleph_slot_api::MetadataToolDefinition] = returned.tools.unwrap();
    assert!(std::ptr::eq(borrowed_tools, tools.as_slice()));
    assert!(borrowed_tools[0].strict);
    assert_eq!(returned.think_level, Some(aleph_slot_api::ThinkLevel::High));
    let old = alephcore::providers::ProviderResponse::text_only("response".into());
    let api: aleph_slot_api::ProviderResponse = old;
    let converted_old: alephcore::providers::adapter::ProviderResponse = api;
    assert_eq!(converted_old.text_content(), "response");
}

#[test]
fn task7_sink_identity_and_distinct_provider_trait_boundaries() {
    fn sink_round_trip(
        old: std::sync::Arc<dyn alephcore::providers::delta::DeltaSink>,
    ) -> std::sync::Arc<dyn alephcore::providers::DeltaSink> {
        let module: std::sync::Arc<dyn aleph_slot_api::ai_delta::DeltaSink> = old;
        let root: std::sync::Arc<dyn aleph_slot_api::DeltaSink> = module;
        let converted_old: std::sync::Arc<dyn alephcore::providers::DeltaSink> = root;
        converted_old
    }
    let _sink = sink_round_trip(std::sync::Arc::new(alephcore::providers::NoopSink));
    assert_eq!(
        TypeId::of::<dyn alephcore::providers::delta::DeltaSink>(),
        TypeId::of::<dyn aleph_slot_api::DeltaSink>()
    );
    assert_eq!(
        TypeId::of::<dyn alephcore::providers::DeltaSink>(),
        TypeId::of::<dyn aleph_slot_api::ai_delta::DeltaSink>()
    );
    assert_eq!(
        TypeId::of::<dyn aleph_slot_api::AiProvider>(),
        TypeId::of::<dyn aleph_slot_api::ai::AiProviderContract>()
    );
    assert_eq!(
        TypeId::of::<dyn aleph_slot_api::AiProviderContract>(),
        TypeId::of::<dyn aleph_slot_api::ai::AiProvider>()
    );
    assert_eq!(
        TypeId::of::<dyn alephcore::AiProvider>(),
        TypeId::of::<dyn alephcore::providers::AiProvider>()
    );
    assert_ne!(
        TypeId::of::<dyn alephcore::AiProvider>(),
        TypeId::of::<dyn aleph_slot_api::AiProviderContract>()
    );
}

// =========================================================================
// Task 8 C3: browser vocabulary identity and native-only table semantics.
// =========================================================================

#[test]
fn task8_browser_public_dto_identity_through_old_paths() {
    macro_rules! same_type {
        ($old:ty, $new:ty) => {{
            assert_eq!(TypeId::of::<$old>(), TypeId::of::<$new>());
            fn round_trip(old_value: $old) -> $old {
                let converted_new: $new = old_value;
                let converted_old: $old = converted_new;
                converted_old
            }
            let _ = round_trip;
        }};
    }
    macro_rules! dto {
        ($name:ident) => {
            same_type!(
                alephcore::browser::types::$name,
                aleph_slot_api::browser::types::$name
            );
        };
    }
    dto!(TabId);
    dto!(TabLine);
    dto!(ActionTarget);
    dto!(ScrollDirection);
    dto!(HistoryNav);
    dto!(WaitCondition);
    dto!(ScreenshotOpts);
    dto!(SnapshotOutput);
    dto!(PresentedSnapshot);
    dto!(ScreenshotOutput);
    dto!(ColorScheme);
    dto!(NetworkCondition);
    dto!(CdpNetworkConditions);
    dto!(Geolocation);
    dto!(EmulateOptions);
    dto!(SameSite);
    dto!(CookieOp);
    same_type!(
        alephcore::browser::tab_registry::TabLine,
        aleph_slot_api::browser::types::TabLine
    );
    same_type!(
        alephcore::browser::engine::Engine,
        aleph_slot_api::browser::engine::Engine
    );
    same_type!(
        alephcore::browser::backend::EffectVerification,
        aleph_slot_api::browser::effect::EffectVerification
    );
    same_type!(
        alephcore::browser::error::BrowserError,
        aleph_slot_api::browser::error::BrowserError
    );
    same_type!(
        alephcore::browser::BrowserError,
        aleph_slot_api::browser::error::BrowserError
    );
    same_type!(
        alephcore::browser::error::StaleReason,
        aleph_slot_api::browser::error::StaleReason
    );
    same_type!(
        alephcore::browser::page_state::StaleReason,
        aleph_slot_api::browser::error::StaleReason
    );
}

#[test]
fn task8_browser_route_and_recording_identity_through_public_core_signatures() {
    use aleph_slot_api::browser::{recording as recording_api, routes as routes_api};
    use alephcore::browser::backend::BrowserBackend;
    use alephcore::builtin_tools::browser_tools::{BrowserNetworkOutput, BrowserRecordOutput};
    use serde::de::DeserializeOwned;

    fn value_type<T: 'static>(_: &T) -> TypeId {
        TypeId::of::<T>()
    }

    // The old cdp_backend modules are crate-private. Infer their exported
    // argument types from the PUBLIC native trait, without calling a backend.
    fn native_input_identity<Old: DeserializeOwned + 'static, New: 'static, F>(
        _native_signature: impl FnOnce(Old) -> F,
        to_new: fn(Old) -> New,
        to_old: fn(New) -> Old,
        fixture: serde_json::Value,
    ) {
        assert_eq!(TypeId::of::<Old>(), TypeId::of::<New>());
        let old_value: Old = serde_json::from_value(fixture).unwrap();
        let converted_new: New = to_new(old_value);
        let converted_old: Old = to_old(converted_new);
        assert_eq!(value_type(&converted_old), TypeId::of::<New>());
    }

    // Only the signature closures are uncalled: no native backend, session,
    // registry, encoder, or browser is instantiated by this witness.
    fn native_inputs(backend: Option<&dyn BrowserBackend>) {
        native_input_identity(
            |old| {
                backend
                    .expect("signature only; never called")
                    .route_add("tab", old)
            },
            |old| {
                let routes_api::NewRouteRule {
                    url_contains,
                    method,
                    kind,
                    scope,
                    note,
                } = old;
                assert_eq!(value_type(&kind), TypeId::of::<routes_api::RouteKind>());
                let converted_kind: routes_api::RouteKind = kind;
                assert_eq!(value_type(&scope), TypeId::of::<routes_api::RouteScope>());
                let converted_scope: routes_api::RouteScope = scope;
                routes_api::NewRouteRule {
                    url_contains,
                    method,
                    kind: converted_kind,
                    scope: converted_scope,
                    note,
                }
            },
            |new: routes_api::NewRouteRule| new,
            serde_json::json!({
                "url_contains":"/api", "method":"GET", "kind":{"type":"abort"},
                "scope":"tab", "note":"C3 identity"
            }),
        );
        native_input_identity(
            |old| {
                backend
                    .expect("signature only; never called")
                    .route_clear("tab", old)
            },
            |old| {
                let converted_scope: routes_api::RouteScope = old;
                converted_scope
            },
            |new: routes_api::RouteScope| new,
            serde_json::json!("profile"),
        );
        native_input_identity(
            |old| {
                backend
                    .expect("signature only; never called")
                    .record_start("tab", old)
            },
            |old| {
                let recording_api::RecordStartOptions {
                    path,
                    fps_cap,
                    format,
                    quality,
                    ffmpeg,
                } = old;
                assert_eq!(
                    value_type(&format),
                    TypeId::of::<recording_api::RecordingFormat>()
                );
                let converted_format: recording_api::RecordingFormat = format;
                recording_api::RecordStartOptions {
                    path,
                    fps_cap,
                    format: converted_format,
                    quality,
                    ffmpeg,
                }
            },
            |new: recording_api::RecordStartOptions| new,
            serde_json::json!({"path":null,"fps_cap":10,"format":"webm","quality":80}),
        );
    }
    native_inputs(None);

    // Public Core output fields expose the old return identities. These
    // value round trips run without invoking a tool or a native backend.
    let mut network = BrowserNetworkOutput {
        success: true,
        requests: String::new(),
        message: Some("C3 identity".into()),
        rules: Some(vec![serde_json::from_value::<routes_api::RouteRuleInfo>(
            serde_json::json!({
                "id":"r1","url_contains":"/api","method":null,"kind_label":"abort",
                "scope":"profile","hits":3,"active":false,"note":null
            }),
        )
        .unwrap()]),
    };
    let old = network.rules.as_mut().unwrap().pop().unwrap();
    assert_eq!(value_type(&old), TypeId::of::<routes_api::RouteRuleInfo>());
    let new: routes_api::RouteRuleInfo = old;
    network.rules.as_mut().unwrap().push(new);
    let old = &network.rules.as_ref().unwrap()[0];
    assert_eq!(old.id, "r1");
    assert_eq!(
        value_type(&old.scope),
        TypeId::of::<routes_api::RouteScope>()
    );

    let mut recording = BrowserRecordOutput {
        success: true,
        message: Some("C3 identity".into()),
        recording: Some(
            serde_json::from_value::<recording_api::RecordingStatus>(serde_json::json!({
                "recording_id":"rec1","path":"/tmp/c3.webm","elapsed_ms":12,
                "captured_frames":2,"dropped_frames":0,"ffmpeg_source":"fixture only"
            }))
            .unwrap(),
        ),
        receipt: Some(
            serde_json::from_value::<recording_api::RecordingReceipt>(serde_json::json!({
                "recording_id":"rec1","path":"/tmp/c3.webm","duration_ms":12,
                "captured_frames":2,"encoded_frames":2,"dropped_frames":0,
                "encoder_exit":"0","size_bytes":32,"complete":true,"truncation_reason":null
            }))
            .unwrap(),
        ),
    };
    let old = recording.recording.take().unwrap();
    assert_eq!(
        value_type(&old),
        TypeId::of::<recording_api::RecordingStatus>()
    );
    let new: recording_api::RecordingStatus = old;
    recording.recording = Some(new);
    assert_eq!(recording.recording.as_ref().unwrap().recording_id, "rec1");
    let old = recording.receipt.take().unwrap();
    assert_eq!(
        value_type(&old),
        TypeId::of::<recording_api::RecordingReceipt>()
    );
    let new: recording_api::RecordingReceipt = old;
    recording.receipt = Some(new);
    assert_eq!(recording.receipt.as_ref().unwrap().encoded_frames, 2);
}

#[test]
fn task8_browser_alias_constants_and_pure_helpers_keep_old_paths() {
    use aleph_slot_api::browser::{text, types as new};
    use alephcore::browser::{engine::Engine, types as old};

    assert_eq!(TypeId::of::<old::TabId>(), TypeId::of::<String>());
    let old_value: old::TabId = String::from("tab-c3");
    let converted_new: new::TabId = old_value;
    let converted_old: old::TabId = converted_new;
    let plain: String = converted_old;
    assert_eq!(plain, "tab-c3");
    assert_eq!(old::SCROLL_STEP_PX, new::SCROLL_STEP_PX);
    assert_eq!(old::SCROLL_STEP_PX, 400);
    assert_eq!(old::REF_TOKEN, new::REF_TOKEN);
    assert_eq!(old::REF_TOKEN, "[ref=");
    assert_eq!(old::ScrollDirection::Up.wheel_delta(), (0, -400));
    assert_eq!(old::ScrollDirection::Right.wheel_delta(), (400, 0));
    assert_eq!(old::ColorScheme::Dark.as_mcp(), "dark");
    assert_eq!(
        old::NetworkCondition::Offline.as_playwright_state(),
        Some("offline")
    );
    assert_eq!(old::NetworkCondition::Online.as_mcp(), None);
    assert_eq!(old::NetworkCondition::Online.as_cdp(), None);
    let condition: new::CdpNetworkConditions = old::NetworkCondition::Offline.as_cdp().unwrap();
    assert!(condition.offline);
    assert_eq!(condition.download_bps, -1.0);
    assert_eq!(old::SameSite::Strict.as_cli(), "Strict");
    let screenshot = old::ScreenshotOpts::default();
    assert!(!screenshot.full_page);
    assert_eq!(screenshot.format, "png");
    let options = old::EmulateOptions::default();
    assert!(options.is_empty());
    assert!(options.validate().is_err());
    let options = old::EmulateOptions {
        color_scheme: Some(old::ColorScheme::Auto),
        ..options
    };
    assert!(options.validate().is_ok());
    for engine in Engine::ALL {
        assert_eq!(Engine::parse(engine.as_str()), Some(engine));
        assert_eq!(engine.data_subdir(), engine.as_str());
        assert!(!engine.data_dir_flag().is_empty());
    }
    assert_eq!(Engine::chromium_default(), Engine::Chromium);
    assert_eq!(
        alephcore::browser::backend::EffectVerification::Failed.as_wire(),
        "failed"
    );
    assert_eq!(
        alephcore::browser::backend::EffectVerification::Skipped(Engine::Obscura).as_wire(),
        "skipped(obscura)"
    );
    let page_text = "quote \" newline\nslash\\";
    assert_eq!(
        alephcore::browser::page_state::quote(page_text),
        text::quote(page_text)
    );
}

#[test]
fn task8_browser_canonical_port_and_native_backend_traits_are_distinct() {
    assert_ne!(
        TypeId::of::<dyn aleph_slot_api::browser::BrowserManagerContract>(),
        TypeId::of::<dyn alephcore::browser::backend::BrowserBackend>()
    );
}

#[test]
fn task8_browser_native_effect_extension_preserves_table_lookup_semantics() {
    // Required for the old associated-call spelling: this is a Core extension,
    // not an inherent method on the API verdict or a positive effect producer.
    use alephcore::browser::backend::{EffectVerification, EffectVerificationExt};
    use alephcore::browser::engine::{capabilities, Cap, Engine};

    for engine in Engine::ALL {
        let verdict = EffectVerification::for_engine(engine);
        match capabilities(engine).effect_probe {
            Cap::Supported => assert!(verdict.is_none()),
            _ => assert!(
                matches!(verdict, Some(EffectVerification::Skipped(actual)) if actual == engine)
            ),
        }
        assert!(!matches!(verdict, Some(EffectVerification::Verified)));
    }
}

// =========================================================================
// Task 9: append-only assignments and source-census regressions.
// =========================================================================

#[test]
fn task9_direct_assignments_cover_migrated_contract_families() {
    macro_rules! round_trip {
        ($old:ty, $new:ty) => {{
            fn assignment(old_value: $old) -> $old {
                let converted_new: $new = old_value;
                let converted_old: $old = converted_new;
                converted_old
            }
            let _ = assignment;
            assert_eq!(TypeId::of::<$old>(), TypeId::of::<$new>());
        }};
    }
    round_trip!(alephcore::error::AlephError, aleph_slot_api::AlephError);
    round_trip!(
        alephcore::error::AlephException,
        aleph_slot_api::AlephException
    );
    round_trip!(alephcore::error::ErrorClass, aleph_slot_api::ErrorClass);
    round_trip!(alephcore::error::Result<()>, aleph_slot_api::Result<()>);
    round_trip!(alephcore::tools::ToolError, aleph_slot_api::tool::ToolError);
    round_trip!(
        alephcore::tools::ToolDefinition,
        aleph_slot_api::tool::ToolDefinition
    );
    round_trip!(
        alephcore::tools::ToolCapabilityDescriptor,
        aleph_slot_api::descriptor::ToolCapabilityDescriptor
    );
    round_trip!(
        alephcore::session::events::ToolOutput,
        aleph_slot_api::tool::ToolOutput
    );
    round_trip!(
        alephcore::session::events::ToolOutputMetadata,
        aleph_slot_api::tool::ToolOutputMetadata
    );
    round_trip!(
        alephcore::session::events::ToolImage,
        aleph_slot_api::tool::ToolImage
    );
    round_trip!(
        alephcore::search::SearchOptions,
        aleph_slot_api::search::SearchOptions
    );
    round_trip!(
        alephcore::search::SearchCapabilities,
        aleph_slot_api::search::SearchCapabilities
    );
    round_trip!(
        alephcore::search::SearchResult,
        aleph_slot_api::search::SearchResult
    );
    round_trip!(
        std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
        std::sync::Arc<dyn aleph_slot_api::embedding::EmbeddingProvider>
    );
    round_trip!(
        std::sync::Arc<dyn alephcore::tools::handlers::ToolHandler>,
        std::sync::Arc<dyn aleph_slot_api::handler::ToolHandler>
    );
    round_trip!(
        alephcore::gateway::channel::InboundMessage,
        aleph_slot_api::channel::InboundMessage
    );
    round_trip!(
        alephcore::gateway::channel::OutboundMessage,
        aleph_slot_api::channel::OutboundMessage
    );
    round_trip!(
        alephcore::gateway::channel_approval::PendingApproval,
        aleph_slot_api::approval::PendingApproval
    );
    round_trip!(
        alephcore::gateway::cancellation::CancellationToken,
        aleph_slot_api::cancellation::CancellationToken
    );
    round_trip!(
        alephcore::extension::HookEvent,
        aleph_slot_api::hooks::HookEvent
    );
    round_trip!(
        alephcore::providers::message::UnifiedMessage,
        aleph_slot_api::ai_message::UnifiedMessage
    );
    round_trip!(
        alephcore::providers::adapter::ProviderResponse,
        aleph_slot_api::ai_response::ProviderResponse
    );
    round_trip!(
        alephcore::browser::error::BrowserError,
        aleph_slot_api::browser::error::BrowserError
    );
    round_trip!(
        alephcore::browser::types::SnapshotOutput,
        aleph_slot_api::browser::types::SnapshotOutput
    );
    // The accepted tests above also retain the intentionally distinct AI and
    // browser trait boundaries. No conversion between those traits is claimed.
}

#[cfg(feature = "test-helpers")]
mod task9_producer_guards {
    use alephcore::extension::hooks::producer_census::{
        scan_hook_fixture, scan_hook_producers, scan_production_hook_producers,
    };
    use alephcore::utils::source_scan::rust_sources_under_root;
    use std::path::{Path, PathBuf};

    const AUTHORITY: &str = "authority/hooks.rs";

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/modularization/producers")
            .join(name)
    }

    fn assert_bad(root: &Path, expected: &str) -> String {
        let error = scan_hook_fixture(root, Path::new(AUTHORITY))
            .expect_err("a bad source root must fail closed")
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
        error
    }

    #[test]
    fn production_census_is_nonempty_and_session_start_mutation_removes_all_evidence() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let report = scan_production_hook_producers().unwrap();
        assert_eq!(
            report.inventory.variants.len(),
            alephcore::extension::HookEvent::ALL.len()
        );
        assert_eq!(report.inventory.variants.len(), 24);
        assert!(report.files_scanned > 0);
        assert!(report.dispatchers.contains("fire_global_observer"));
        assert!(report.producers.values().all(|sites| !sites.is_empty()));
        for (event, sites) in &report.producers {
            println!(
                "Task9 production {event}: {} qualifying sites {sites:?}",
                sites.len()
            );
        }
        let authority =
            std::fs::read_to_string(root.join("crates/aleph-slot-api/src/hooks.rs")).unwrap();
        let corpus = rust_sources_under_root(root, Path::new("src")).unwrap();
        // Remove ALL literal evidence for this event, including redundant sites.
        // A single site's removal need not fail when other sites still qualify.
        let mutated: Vec<_> = corpus
            .into_iter()
            .map(|(path, raw)| {
                (
                    path,
                    raw.replace("HookEvent::SessionStart", "HookEvent::RemovedSessionStart"),
                )
            })
            .collect();
        let error = scan_hook_producers(root, &authority, &mutated)
            .unwrap_err()
            .to_string();
        assert!(error.contains("SessionStart"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
    }

    #[test]
    fn clean_fixture_uses_real_authority_and_dispatch_fixpoint() {
        let root = fixture("clean");
        let report = scan_hook_fixture(&root, Path::new(AUTHORITY)).unwrap();
        assert_eq!(report.inventory.variants.len(), 3);
        assert_eq!(report.producers.len(), 3);
        assert!(report.dispatchers.contains("relay"));
        assert!(report.dispatchers.contains("fire_startup"));
        assert_eq!(report.producers["SessionStart"].len(), 1);
        assert_eq!(report.producers["BeforeToolCall"].len(), 2);
        assert_eq!(report.producers["AfterCompaction"].len(), 1);
        assert!(report
            .inventory
            .aliases
            .contains(&("PostCompact".into(), "AfterCompaction".into())));
    }

    #[test]
    fn each_fixture_event_fails_after_all_its_qualifying_evidence_is_removed() {
        let root = fixture("clean");
        let raw = std::fs::read_to_string(root.join(AUTHORITY)).unwrap();
        let corpus = rust_sources_under_root(&root, Path::new("src")).unwrap();
        let clean = scan_hook_producers(&root, &raw, &corpus).unwrap();
        for event in &clean.inventory.variants {
            let mutated: Vec<_> = corpus
                .iter()
                .map(|(path, text)| {
                    (
                        path.clone(),
                        text.replace(&format!("HookEvent::{event}"), "HookEvent::RemovedEvidence"),
                    )
                })
                .collect();
            let error = scan_hook_producers(&root, &raw, &mutated)
                .unwrap_err()
                .to_string();
            assert!(error.contains(event), "{error}");
            assert!(error.contains(&root.display().to_string()), "{error}");
        }
        // Deleting just one of the two qualifying tool calls retains coverage.
        let redundant: Vec<_> = corpus.iter().map(|(path, text)| {
            (path.clone(), text.replace("fn first_tool() {\n    executor.execute_observers(HookEvent::BeforeToolCall, &ctx);\n}", "fn first_tool() {}"))
        }).collect();
        assert_eq!(
            scan_hook_producers(&root, &raw, &redundant)
                .unwrap()
                .producers["BeforeToolCall"]
                .len(),
            1
        );
    }

    #[test]
    fn mutations_fail_for_missing_event_authority_alias_and_dispatch_reachability() {
        for (name, expected) in [
            ("missing_producer", "SessionStart"),
            ("missing_enum", "HookEvent enum"),
            ("empty_enum", "empty HookEvent"),
            ("string_comment_enum", "HookEvent enum"),
            ("added_variant", "NeverFired"),
            ("alias_nonfired", "StartupAlias -> SilentTarget"),
            ("unreachable_wrapper", "SessionStart"),
            ("extension_only", "SessionStart"),
        ] {
            assert_bad(&fixture(name), expected);
        }
    }

    #[test]
    fn clean_bad_clean_and_bad_root_clean_root_are_isolated_in_one_process() {
        let clean = fixture("clean");
        let bad = fixture("missing_producer");
        let first = scan_hook_fixture(&clean, Path::new(AUTHORITY)).unwrap();
        let bad_first = assert_bad(&bad, "SessionStart");
        assert_eq!(
            first,
            scan_hook_fixture(&clean, Path::new(AUTHORITY)).unwrap()
        );
        assert_eq!(bad_first, assert_bad(&bad, "SessionStart"));
        assert_eq!(
            first,
            scan_hook_fixture(&clean, Path::new(AUTHORITY)).unwrap()
        );
        for name in ["empty_enum", "missing_enum", "extension_only"] {
            assert_bad(
                &fixture(name),
                if name == "extension_only" {
                    "SessionStart"
                } else {
                    "HookEvent"
                },
            );
            assert_eq!(
                first,
                scan_hook_fixture(&clean, Path::new(AUTHORITY)).unwrap()
            );
        }
    }

    #[test]
    fn absent_unreadable_empty_corpus_and_bad_policy_paths_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        assert_bad(temp.path(), "authority/hooks.rs");
        std::fs::create_dir_all(temp.path().join("authority/hooks.rs")).unwrap();
        assert_bad(temp.path(), "authority/hooks.rs"); // directory is not readable text
        let clean = fixture("clean");
        let raw = std::fs::read_to_string(clean.join(AUTHORITY)).unwrap();
        assert!(scan_hook_producers(&clean, &raw, &[])
            .unwrap_err()
            .to_string()
            .contains("empty corpus"));
        for path in ["../escape.rs", "/src/runtime.rs", "runtime.rs"] {
            let error = scan_hook_producers(&clean, &raw, &[(path.into(), "fn x() {}".into())])
                .unwrap_err()
                .to_string();
            assert!(error.contains("corpus path"), "{error}");
        }
        assert!(rust_sources_under_root(temp.path(), Path::new("src")).is_err());
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        assert!(rust_sources_under_root(temp.path(), Path::new("src")).is_err());
    }
}
