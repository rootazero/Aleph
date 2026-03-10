//! Blast radius risk assessment for POE tasks.
//!
//! Two-phase assessment following System 1 + System 2 pattern:
//! - System 1 (StaticSafetyScanner): Deterministic pattern matching
//! - System 2 (SemanticRiskAnalyzer): LLM-based contextual analysis

pub mod assessor;
pub mod semantic_analyzer;
pub mod static_scanner;
