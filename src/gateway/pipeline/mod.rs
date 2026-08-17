//! Message pipeline data types.
//!
//! Defines the structured message types that flow toward execution:
//! `InboundContext → MergedMessage → EnrichedMessage`.
//!
//! Note: the earlier `MessagePipeline` orchestrator (debounce → media download
//! → media understanding) and its `DebounceBuffer` were removed as dead code —
//! rapid-fire merging is handled by [`crate::gateway::coalescer`], and these
//! data types are consumed directly by the session scheduler.