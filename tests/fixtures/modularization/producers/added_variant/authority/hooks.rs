#[derive(Debug)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    #[serde(alias = "SessionStart")]
    SessionStart,
    #[serde(alias = "NeverFired")]
    NeverFired,
    #[serde(alias = "PreToolUse", alias = "BeforeToolCall")]
    BeforeToolCall,
    #[serde(alias = "PostCompact", alias = "AfterCompaction")]
    AfterCompaction,
}
