namespace Aether.Models;

/// <summary>
/// Halo window states.
///
/// Phase 1 implements 6 core states.
/// Full 21 states will be implemented in Phase 2.
/// </summary>
public enum HaloState
{
    // Core states (Phase 1)
    Hidden,
    Listening,
    Thinking,
    Processing,
    Streaming,
    Success,
    Error,

    // Extended states (Phase 2)
    // Idle,
    // Appearing,
    // Disappearing,
    // MultiTurnActive,
    // MultiTurnThinking,
    // MultiTurnStreaming,
    // ToolExecuting,
    // ToolSuccess,
    // ToolError,
    // ClarificationNeeded,
    // ClarificationReceived,
    // AgentPlanning,
    // AgentExecuting,
    // AgentComplete
}
