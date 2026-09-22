//! Internal submodules of the workflow_tool — wire DTOs and the per-phase
//! tally used by the `status` rendering.
//!
//! The public surface (the `WorkflowTool` struct + its `AlephTool` impl +
//! re-exported DTOs) lives in the parent module
//! [`crate::builtin_tools::workflow_tool`]. Everything in here is internal.

pub mod dto;
pub mod phase_tally;