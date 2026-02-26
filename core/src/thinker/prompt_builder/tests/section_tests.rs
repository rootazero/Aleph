//! Tests that call append_* methods

use super::super::*;

#[test]
fn test_append_protocol_tokens_with_silent_reply() {
    use crate::thinker::context::EnvironmentContract;
    use crate::thinker::interaction::{Capability, InteractionConstraints, InteractionParadigm};

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    let contract = EnvironmentContract {
        paradigm: InteractionParadigm::Background,
        active_capabilities: vec![Capability::SilentReply],
        constraints: InteractionConstraints::default(),
        security_notes: vec![],
    };

    builder.append_protocol_tokens(&mut prompt, &contract);

    assert!(prompt.contains("ALEPH_HEARTBEAT_OK"));
    assert!(prompt.contains("ALEPH_SILENT_COMPLETE"));
    assert!(prompt.contains("Response Protocol Tokens"));
}

#[test]
fn test_append_protocol_tokens_without_silent_reply() {
    use crate::thinker::context::EnvironmentContract;
    use crate::thinker::interaction::{InteractionConstraints, InteractionParadigm};

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    let contract = EnvironmentContract {
        paradigm: InteractionParadigm::CLI,
        active_capabilities: vec![],
        constraints: InteractionConstraints::default(),
        security_notes: vec![],
    };

    builder.append_protocol_tokens(&mut prompt, &contract);

    assert!(!prompt.contains("ALEPH_HEARTBEAT_OK"));
}

#[test]
fn test_append_operational_guidelines_background() {
    use crate::thinker::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::Background);

    assert!(prompt.contains("System Operational Awareness"));
    assert!(prompt.contains("Diagnostic Capabilities"));
    assert!(prompt.contains("NEVER Do Autonomously"));
}

#[test]
fn test_append_operational_guidelines_cli() {
    use crate::thinker::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::CLI);

    // CLI should also get operational guidelines
    assert!(prompt.contains("System Operational Awareness"));
}

#[test]
fn test_append_operational_guidelines_messaging_skipped() {
    use crate::thinker::interaction::InteractionParadigm;

    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_operational_guidelines(&mut prompt, InteractionParadigm::Messaging);

    // Messaging should NOT get operational guidelines (save tokens)
    assert!(!prompt.contains("System Operational Awareness"));
}

#[test]
fn test_append_safety_constitution() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();
    builder.append_safety_constitution(&mut prompt);
    assert!(prompt.contains("## Safety Principles"));
    assert!(prompt.contains("Autonomy Boundaries"));
    assert!(prompt.contains("Oversight Priority"));
    assert!(prompt.contains("Transparency"));
    assert!(prompt.contains("Data Handling"));
    assert!(prompt.contains("NO independent goals"));
}

#[test]
fn test_append_memory_guidance() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();
    builder.append_memory_guidance(&mut prompt);
    assert!(prompt.contains("## Memory Protocol"));
    assert!(prompt.contains("Before Answering"));
    assert!(prompt.contains("memory_search"));
    assert!(prompt.contains("After Learning"));
    assert!(prompt.contains("Memory Hygiene"));
}

#[test]
fn test_append_citation_standards() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();

    builder.append_citation_standards(&mut prompt);

    assert!(prompt.contains("## Citation Standards"));
    assert!(prompt.contains("[Source: <path>#<id>]"));
    assert!(prompt.contains("citation is mandatory"));
}

#[test]
fn test_append_channel_behavior_telegram_group() {
    use crate::thinker::channel_behavior::{ChannelBehaviorGuide, ChannelVariant};
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();
    let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Telegram { is_group: true });
    builder.append_channel_behavior(&mut prompt, &guide);
    assert!(prompt.contains("## Channel: Telegram Group"));
    assert!(prompt.contains("Group Chat Rules"));
}

#[test]
fn test_append_channel_behavior_terminal() {
    use crate::thinker::channel_behavior::{ChannelBehaviorGuide, ChannelVariant};
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();
    let guide = ChannelBehaviorGuide::for_channel(ChannelVariant::Terminal);
    builder.append_channel_behavior(&mut prompt, &guide);
    assert!(prompt.contains("## Channel: Terminal"));
    assert!(!prompt.contains("Group Chat Rules"));
}

#[test]
fn test_append_soul_continuity() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let mut prompt = String::new();
    builder.append_soul_continuity(&mut prompt);
    assert!(prompt.contains("## Soul Continuity"));
    assert!(prompt.contains("gradual"));
    assert!(prompt.contains("anti-patterns"));
    assert!(prompt.contains("expertise"));
    assert!(prompt.contains("identity files"));
}
