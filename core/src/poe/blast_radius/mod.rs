//! Blast radius risk assessment for POE tasks.
//!
//! Two-phase assessment following System 1 + System 2 pattern:
//! - System 1 (StaticSafetyScanner): Deterministic pattern matching
//! - System 2 (SemanticRiskAnalyzer): LLM-based contextual analysis

pub mod static_scanner;
