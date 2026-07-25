//! Typed target for the right-side contextual inspector (the workspace pane).
//!
//! The pane began life as a bare tool-detail mirror: its selection was an
//! `Option<(run_id, tool_id)>` tuple that could only ever point at a tool
//! call — so every other clickable key point in the transcript (a run's
//! cost/tokens, its reasoning, its plan) had nowhere to open a richer view,
//! and the pane just re-rendered the same tool card the chat already showed.
//!
//! [`InspectorTarget`] generalizes that selection: any chat key point routes
//! the pane to a dedicated surface by handing [`crate::state::layout::WorkspaceState::inspect`]
//! the matching variant. The router in [`crate::components::inspector`] maps
//! each variant to one small surface component.
//!
//! Classification is a **static variant map**, never a content sniff — the
//! panel does not decide *which* surface to open by inspecting a payload (that
//! would drift toward LLM-reasoning, R7). The click site (or, later, an
//! explicit LLM tool-output field) supplies the variant; the panel stays a
//! pure renderer of a selection signal (R4).
//!
//! Adding a surface = one variant here + one arm in the router + one component.

/// What the contextual inspector pane is currently showing.
///
/// The data-carrying fields are the *identity* of the thing being inspected
/// (a run, a tool call), never its rendered content — surface components read
/// the already-projected `ChatState` / `WorkspaceState` signals (or issue one
/// JSON-RPC) to render, so the panel remains pure I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorTarget {
    /// A single tool call's full (uncapped) args / result / diff. The original
    /// behaviour — live-followed while streaming, pinned on click.
    Tool { run_id: String, tool_id: String },
    /// A completed run's cost / token / model-fallback breakdown.
    RunMeta { run_id: String },
    /// A run's reasoning (think→act) transcript.
    Reasoning { run_id: String },
    /// A run's task plan / scratchpad detail.
    Plan { run_id: String },
}
