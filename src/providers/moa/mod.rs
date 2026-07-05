//! MoA (Mixture of Agents) virtual-provider facade.
//!
//! Ported from hermes-agent's MoAClient: the agent loop is unaware of MoA;
//! advisors consult on a flattened view of the live conversation, and the
//! preset's aggregator is the acting model.
//! Spec: docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md

pub(crate) mod advisory_view;
pub(crate) mod prompts;
