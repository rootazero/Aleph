//! Pure contract authority for the Aleph slot API: error, text, tool,
//! search/embedding, and (Task 4) channel / approval / cancellation /
//! webhook contracts, plus the pure AI provider contract (Task 7).
//!
//! Crate-level invariants (R1, R3, R7, R10):
//!
//! * **No I/O, no platform, no LLM.** This crate is importable from contexts
//!   that cannot reach for `alephcore` (the WASM panel, ffi shims, public
//!   SDK stubs). It is also the side that `alephcore` depends on, not the
//!   other way around.
//! * **No tokio RUNTIME, no `log`.** Task 7 retains `tracing` only for the
//!   existing response diagnostics; no subscriber or runtime is installed here.
//!   Task 4 moved the channel
//!   contract here, whose shared state is tokio `broadcast` + tokio
//!   `RwLock`, so `tokio` is enabled with the `sync` feature only (no
//!   rt / net / fs / time / io). The runtime itself stays out of the
//!   dependency tree.
//! * **No hidden state.** No global, no thread-local, no `OnceCell`. Every
//!   constructor is deterministic and every test in `tests/error_compat.rs`
//!   and `tests/tool_compat.rs` pins a literal value.
//! * **No `alephcore` edge.** The dependency direction is one-way:
//!   `alephcore → aleph-slot-api`. The reverse would reintroduce every
//!   layering violation Task 1 / Task 2 exist to remove.
//!
//! ## Module layout
//!
//! * [`error`] — `AlephError`, `ErrorClass`, `AlephException`, `Result<T>`,
//!   plus all the `From` impls that bridge external error types. **This is
//!   the only error authority in the workspace**; `alephcore::error` is a
//!   re-export shim, not a second definition.
//! * [`text`] — UTF-8 safe helpers. `truncate_text` is the one moved out of
//!   `alephcore::utils::text_format`; the other four truncate variants stay
//!   in core because they encode different contracts (bytes vs chars, hard
//!   vs soft cap, marker vs none) and the survey that motivated them
//!   explicitly refused to fold them. See that file for the table.
//!
//! ### Tool contract (Task 2)
//!
//! * [`tool`] — `ToolError`, `RefusedBy`, the loop-side `ToolDefinition` and
//!   its `ToolDefinitionMetadata`, the wire `ToolOutput` / `ToolOutputMetadata`
//!   / `ToolImage` shape, the `McpServerFilter` type alias, and the
//!   `to_metadata_form` compatibility projection. The `ToolService` async_trait
//!   stays in `alephcore` (it depends on `tokio_util::CancellationToken`
//!   and the harness/permission/registry sides that the API never sees).
//! * [`descriptor`] — the capability registry's single source of truth:
//!   `ToolKind`, `ReplayPolicy`, `ImplementationContract`,
//!   `ReplayContractFingerprint` (versioned SHA-256 over a canonical
//!   preimage), `ToolCallIdentity`, the descriptor-side `ToolSource` (3
//!   variants), the `ReplayPolicyLookup` / `ToolDescriptorLookup` traits,
//!   `DescriptorError`, `ToolCapabilityDescriptor`, plus the
//!   `canonicalize_json` helper the fingerprint
//!   preimage is built from (`#[doc(hidden)] pub`, re-exported
//!   crate-private in Core). The `compute_replay_contract_fingerprint` /
//!   `canonical_replay_contract_preimage` / `push_len_prefixed` helpers
//!   stay private to this crate, exactly as they were private in Core
//!   before the move.
//! * [`concurrency`] — `ConcurrencyClaim`, `ExclusiveScope` (Global /
//!   Paths / Nodes / Sessions), the `paths` / `nodes` / `sessions` /
//!   `global` constructors, the `claims_conflict` truth table (Paths/Nodes
//!   are non-conflicting; Sessions conservatively conflicts with every
//!   other scope kind; the `normalize_path` helper folds both `/` and
//!   `\\` on every host), `batch_parallelizable`, and
//!   `partition_parallel_groups`.
//! * [`metadata`] — the UI / catalog layer: `ToolCategory`,
//!   `ToolSafetyLevel`, the metadata-side `ToolSource` (6 variants — kept
//!   distinct from the descriptor's 3-variant `ToolSource` per R8), the
//!   metadata `ToolDefinition`, `ToolPriority`, `ToolSourceType`,
//!   `ChannelType`, `UnifiedTool`
//!   (private fields, getters + a declaration-only `set_active` that
//!   does NOT activate the registry or grant anything), the
//!   `populate_safety_profile` algorithm, the `extract_source` helper,
//!   and the `project_metadata_source_to_descriptor` six-arm projection
//!   (unknown sources are rejected — never defaulted to `Builtin`).
//!   `ConflictInfo` / `ConflictResolution` stay in `alephcore`
//!   (`tool_metadata::types::conflict`) — they are registry-facing shapes,
//!   not part of the pure contract.
//! * [`error_kind`] — `ToolErrorKind` (15-variant classification), the
//!   `label` / `is_transient` / `admits_ladder` inherent methods, the
//!   `classify_error_str` / `classify_tool_error` helpers, and the
//!   `CROSS_BATCH_REFUSED_CAUSE` byte literal. The classifier is
//!   conservative and pattern-based: refusal / cancelled / repeated never
//!   admit the ladder; HTTP 401/429/timeout phrasing inside any variant
//!   loses to the variant's own head.
//! * [`handler`] — the `ToolHandler` async_trait with all FIVE methods
//!   (`invoke`, `definition`, `concurrency_claim`,
//!   `bind_visible_servers`, `fences_output`) and the public
//!   `McpServerFilter` type alias. Builtins and the MCP bridge implement
//!   this trait; the harness depends on the trait, not on any concrete
//!   handler.
//!
//! ### Search / embedding contract (Task 3)
//!
//! * [`search`] — `Recency`, `SearchOptions` (with the per-provider
//!   parameter mappers and `MAX_SEARCH_RESULTS`), `SearchResult`,
//!   `SearchCapabilities`, and the `SearchProvider` async_trait. The
//!   nine provider HTTP implementations, the registry/factory/handle,
//!   and the capability census stay in `alephcore::search`;
//!   `alephcore::search::{options, provider, result}` are re-export
//!   shims pointing here.
//! * [`embedding`] — the `EmbeddingProvider` async_trait only.
//!   `RemoteEmbeddingProvider`, URL/SSRF validation, the
//!   `create_provider` factory, `truncate_and_normalize`, and the
//!   test-only mock stay in `alephcore::memory::embedding_provider`.
//!
//! ### Channel / approval / cancellation / webhook contract (Task 4)
//!
//! * [`channel`] — the `Channel` trait with all defaults (including the
//!   `unsupported_feature` honesty split between declared and undeclared
//!   capabilities), the ten-variant `ChannelError` vocabulary, the id
//!   newtypes, `InboundMessage` / `OutboundMessage` / `ChannelCapabilities`
//!   / `ChannelState` / `ChannelHealth` / `InboundMessageSender`,
//!   `NativeStreamHandler`, `ChannelFactory` (whose `create_with_id` default
//!   still ignores the id), `ChannelConfig`, and `CB_MESSAGE_ID_PREFIX`.
//!   `paradigm_for_channel_type` stays in `alephcore` (it names the Core
//!   `InteractionParadigm`), as does the `pub(crate)` outbound chunk-length
//!   derivation the Core chunkers share.
//! * [`cancellation`] — `CancellationToken` (std `Arc`/`AtomicBool` with
//!   `SeqCst`, tokio `broadcast` capacity 16, re-emit on late subscribe).
//! * [`approval`] — the approval vocabulary: `ApprovalDecisionType` with
//!   `clamped_for` / `to_outcome_within`, the serde-backfill `full_set`,
//!   the tagged `ApprovalRequest` / `CommandApprovalRequest` wire shape,
//!   `ApprovalOutcome` / `ApprovalResponse`, the `GrantScope` wire token,
//!   and the channel-side delivery surface (`ChannelApprovalCapability`,
//!   `RenderedApproval`, `PendingApproval`, `ApprovalAction`,
//!   `AuthorizationResult`, `ChannelApprovalExt`). The Core approval gate,
//!   the grant store, and the live decision-set derivations
//!   (`once_only` / `session_max` / `with_persistent` / `for_confirm_gate`)
//!   stay in `alephcore`.
//! * [`webhook`] — the `WebhookHandler` trait and `WEBHOOK_ROUTE_PREFIX`
//!   only; the mount table, the axum receiver, and HMAC verification stay
//!   in `alephcore`. The `http::HeaderMap` / `bytes::Bytes` in the
//!   signature are the exact types axum 0.8 re-exports, pinned by the
//!   identity tests in `alephcore`'s `tests/modularization_contracts.rs`.
//!
//! ## Versioning
//!
//! The `Display` strings, constructor bodies, kind label strings, and
//! fingerprint wire format pinned here are load-bearing across the
//! workspace. Adding a variant is fine; changing an existing
//! `#[error("...")]` attribute, a constructor signature, a kind label, or
//! a fingerprint preimage is a deliberate API break and belongs in its
//! own proposal.

#![forbid(unsafe_code)]
// Pure-data crate — refuse the noisy clippy categories that would otherwise
// fire on the long Display-formatting match and the preimage builder.
#![allow(clippy::too_many_lines)]

pub mod ai;
pub mod ai_delta;
pub mod ai_message;
pub mod ai_reasoning;
pub mod ai_response;
pub mod approval;
pub mod browser;
pub mod cancellation;
pub mod channel;
pub mod concurrency;
pub mod descriptor;
pub mod embedding;
pub mod error;
pub mod error_kind;
pub mod handler;
pub mod hooks;
pub mod metadata;
pub mod search;
pub mod slot;
pub mod text;
pub mod tool;
pub mod webhook;

// Public error surface (Task 1) — every `alephcore::error::*` call site
// resolves here. Re-exporting at the crate root keeps the public SDK
// happy with single-name imports.
pub use aleph_protocol as protocol;
pub use error::{AlephError, AlephException, ErrorClass, Result};
pub use text::truncate_text;

// Public tool-contract surface (Task 2). Module paths are the disambiguator
// between the three distinct `ToolSource` types (descriptor 3-variant,
// metadata 6-variant) and the two distinct `ToolDefinition` types (loop-side
// 5-field, metadata 6-field). Re-exports here are convenience aliases for
// the most common call patterns; the canonical home is the module.
pub use concurrency::{
    batch_parallelizable, claims_conflict, normalize_path, partition_parallel_groups,
    ConcurrencyClaim, ExclusiveScope,
};
pub use descriptor::{
    canonicalize_json, DescriptorError, ImplementationContract, ReplayContractFingerprint,
    ReplayPolicy, ReplayPolicyLookup, ToolCallIdentity, ToolCapabilityDescriptor,
    ToolDescriptorLookup, ToolKind, ToolSource as DescriptorToolSource,
};
pub use error_kind::{
    classify_error_str, classify_tool_error, ToolErrorKind, CROSS_BATCH_REFUSED_CAUSE,
};
pub use handler::{McpServerFilter, ToolHandler};
pub use metadata::{
    extract_source, project_metadata_source_to_descriptor, ChannelType, ToolCategory,
    ToolDefinition as MetadataToolDefinition, ToolPriority, ToolSafetyLevel,
    ToolSource as MetadataToolSource, ToolSourceType, UnifiedTool,
};
pub use tool::{
    to_metadata_form, RefusedBy, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolImage,
    ToolOutput, ToolOutputMetadata,
};

// Public search/embedding contract surface (Task 3). The canonical home is
// the module; these are convenience aliases for the common call patterns.
pub use embedding::EmbeddingProvider;
pub use search::{
    Recency, SearchCapabilities, SearchOptions, SearchProvider, SearchResult, MAX_SEARCH_RESULTS,
};

// Public channel/approval/cancellation/webhook contract surface (Task 4).
// The canonical home is the module; these are convenience aliases for the
// common call patterns. `alephcore::gateway::{channel, cancellation,
// channel_approval, webhook_receiver}`, `alephcore::exec::{socket,
// allowed_decisions, approval::types}` and
// `alephcore::sandbox::exec_approval::{gate, grants}` re-export from
// here — one definition, one `TypeId`.
pub use approval::{
    full_set, ApprovalAction, ApprovalDecisionType, ApprovalOutcome, ApprovalRequest,
    ApprovalResponse, AuthorizationResult, ChannelApprovalCapability, ChannelApprovalExt,
    CommandApprovalRequest, GrantScope, PendingApproval, RenderedApproval,
};
pub use cancellation::CancellationToken;
pub use channel::{
    Attachment, CallbackQuery, Channel, ChannelCapabilities, ChannelConfig, ChannelError,
    ChannelFactory, ChannelHealth, ChannelId, ChannelInfo, ChannelResult, ChannelState,
    ChannelStatus, ConversationId, ConversationKind, ConversationPage, ConversationRef,
    HealthStatus, InboundMessage, InboundMessageSender, InlineButton, InlineKeyboard,
    InlineKeyboardRow, MessageId, MessageMeta, NativeStreamHandler, OutboundMessage, PairingData,
    SendResult, StreamProtocol, UserId, CB_MESSAGE_ID_PREFIX,
};
pub use webhook::{WebhookHandler, WEBHOOK_ROUTE_PREFIX};

// Pure AI contract (Task 7). The Core AiProvider remains a distinct runtime
// trait; these aliases expose only the new std-Future + borrowed-sink port.
pub use ai::{
    normalize_think_level, AiProvider, AiProviderContract, RequestPayload, SystemPromptPart,
    ThinkLevel, ToolChoice,
};
pub use ai_delta::{DeltaSink, NoopSink, ProviderDelta};
pub use ai_message::{
    normalize_tool_pairs, transform_messages, value_as_model_text, CacheControl, ContentBlock,
    EphemeralTtl, UnifiedMessage,
};
pub use ai_reasoning::{ReasoningReplay, ThinkingOrigin};
pub use ai_response::{NativeToolCall, ProviderResponse, StopReason, TokenCost, TokenUsage};

// Public slot/typed-hook@1 contract surface (Task 6). The canonical home is
// the `slot` module; these are convenience aliases for the common call
// patterns. The new trait is a pure contract — the runtime dispatch path,
// permission gates, and Core `HookExecutor` stay in `alephcore` and own
// the distinct legacy types. `SlotId` is correlation only (R8): it does
// not parse privileged ID syntax, does not clamp priorities, and does not
// grant or deny authority. The only contract version in force is the
// protocol `HOOK_CONTRACT_VERSION`; this crate does not publish a
// `ContractVersion::CURRENT` alias until a second contract arrives.
pub use slot::{
    validate_hook_request, ContractVersion, Hook, HookContext, HookDescriptor, HookError, SlotId,
};

// Planned third-party re-exports for tests and downstream consumers that
// want a single import root for the wire / async plumbing already in the
// dependency tree. These are the SAME major versions used by the
// descriptor / webhook / channel modules — no Cargo dependency change,
// no version bump.
//
// * `async_trait::async_trait` is re-exported as the `async_trait` macro
//   at the crate root so consumer code can write `use aleph_slot_api::async_trait;`
//   and apply `#[async_trait]` directly. The bare `pub use async_trait;`
//   reexport of the module is intentionally NOT provided, because the
//   bare name would shadow the macro and prevent consumer use.
// * `serde_json::json` (the `json!` macro) and `serde_json::Value` are
//   re-exported as root names `json` and `Value` so consumer code can
//   write `use aleph_slot_api::{json, Value};` and use them directly.
//   The full `serde_json` crate reexport is also retained for consumers
//   that want the rest of the crate's surface.
pub use async_trait::async_trait;
pub use bytes;
pub use http;
pub use serde_json;
pub use serde_json::{json, Value};
pub use tokio::sync;
