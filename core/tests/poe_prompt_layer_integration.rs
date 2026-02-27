//! Integration tests for PoePromptLayer within the full PromptPipeline.
//!
//! Verifies that PoePromptLayer correctly injects POE success criteria
//! into the assembled system prompt when used inside PromptPipeline::default_layers().

use std::path::PathBuf;

use alephcore::poe::prompt_context::PoePromptContext;
use alephcore::poe::types::{SuccessManifest, ValidationRule};
use alephcore::thinker::prompt_builder::PromptConfig;
use alephcore::thinker::prompt_layer::{AssemblyPath, LayerInput};
use alephcore::thinker::prompt_pipeline::PromptPipeline;

#[test]
fn test_poe_layer_present_in_default_pipeline() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools: Vec<alephcore::agent_loop::ToolInfo> = vec![];

    // With POE context, the assembled prompt should include <poe_success_criteria>
    let manifest = SuccessManifest::new("t1", "Build the feature")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("src/feature.rs"),
        });
    let poe = PoePromptContext::new().with_manifest(manifest);
    let input = LayerInput::basic(&config, &tools).with_poe(&poe);
    let output = pipeline.execute(AssemblyPath::Basic, &input);

    assert!(
        output.contains("<poe_success_criteria>"),
        "Default pipeline should include POE criteria block when POE context is set"
    );
    assert!(
        output.contains("Build the feature"),
        "Pipeline output should contain the POE objective"
    );
    assert!(
        output.contains("File must exist: `src/feature.rs`"),
        "Pipeline output should contain the formatted hard constraint"
    );
}

#[test]
fn test_poe_layer_absent_without_context() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools: Vec<alephcore::agent_loop::ToolInfo> = vec![];

    // Without POE context, no <poe_success_criteria> block
    let input = LayerInput::basic(&config, &tools);
    let output = pipeline.execute(AssemblyPath::Basic, &input);

    assert!(
        !output.contains("<poe_success_criteria>"),
        "Pipeline output should NOT contain POE block when no POE context"
    );
}

#[test]
fn test_poe_layer_ordering_after_tools_before_security() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools: Vec<alephcore::agent_loop::ToolInfo> = vec![];

    let manifest = SuccessManifest::new("t1", "Test ordering")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "cargo".into(),
            args: vec!["test".into()],
            timeout_ms: 60_000,
        });
    let poe = PoePromptContext::new().with_manifest(manifest);
    let input = LayerInput::basic(&config, &tools).with_poe(&poe);
    let output = pipeline.execute(AssemblyPath::Basic, &input);

    // The POE block should appear in the output
    let poe_pos = output.find("<poe_success_criteria>");
    assert!(poe_pos.is_some(), "POE block should be present");

    // If security content exists, POE should come before it
    if let Some(security_pos) = output.find("SECURITY") {
        let poe_pos = poe_pos.unwrap();
        assert!(
            poe_pos < security_pos,
            "POE block (pos {}) should appear before security content (pos {})",
            poe_pos,
            security_pos,
        );
    }
}

#[test]
fn test_poe_layer_with_all_fields() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools: Vec<alephcore::agent_loop::ToolInfo> = vec![];

    let manifest = SuccessManifest::new("full-test", "Complete feature with all constraints")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("src/lib.rs"),
        })
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "cargo".into(),
            args: vec!["test".into()],
            timeout_ms: 60_000,
        });

    let poe = PoePromptContext::new()
        .with_manifest(manifest)
        .with_hint("Focus on the auth module first".into())
        .with_progress("2/5 constraints met, best score: 0.6".into());

    let input = LayerInput::basic(&config, &tools).with_poe(&poe);
    let output = pipeline.execute(AssemblyPath::Basic, &input);

    assert!(output.contains("Complete feature with all constraints"));
    assert!(output.contains("File must exist: `src/lib.rs`"));
    assert!(output.contains("cargo test"));
    assert!(output.contains("Focus on the auth module first"));
    assert!(output.contains("2/5 constraints met"));
    assert!(output.contains("</poe_success_criteria>"));
}

#[test]
fn test_poe_layer_works_across_all_assembly_paths() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools: Vec<alephcore::agent_loop::ToolInfo> = vec![];

    let poe = PoePromptContext::new()
        .with_hint("Cross-path test hint".into());

    let input = LayerInput::basic(&config, &tools).with_poe(&poe);

    // PoePromptLayer participates in all 5 paths
    for path in [
        AssemblyPath::Basic,
        AssemblyPath::Hydration,
        AssemblyPath::Soul,
        AssemblyPath::Context,
        AssemblyPath::Cached,
    ] {
        let output = pipeline.execute(path, &input);
        assert!(
            output.contains("Cross-path test hint"),
            "POE content should be present in {:?} path",
            path,
        );
    }
}
