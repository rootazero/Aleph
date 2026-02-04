namespace Aleph.Models;

/// <summary>
/// Halo window states - complete 21 state machine.
///
/// Matches macOS HaloState enum for feature parity.
/// </summary>
public enum HaloState
{
    // Core states
    Idle,
    Hidden,
    Appearing,
    Listening,
    Thinking,
    Processing,
    Streaming,
    Success,
    Error,
    Disappearing,

    // Multi-turn conversation states
    MultiTurnActive,
    MultiTurnThinking,
    MultiTurnStreaming,

    // Tool execution states
    ToolExecuting,
    ToolSuccess,
    ToolError,

    // Clarification states (Phantom Flow)
    ClarificationNeeded,
    ClarificationReceived,

    // Agent states
    AgentPlanning,
    AgentExecuting,
    AgentComplete
}

/// <summary>
/// State categories for grouping related states.
/// </summary>
public static class HaloStateExtensions
{
    /// <summary>
    /// Check if the state is a loading/processing state.
    /// </summary>
    public static bool IsLoading(this HaloState state) => state switch
    {
        HaloState.Thinking => true,
        HaloState.Processing => true,
        HaloState.MultiTurnThinking => true,
        HaloState.ToolExecuting => true,
        HaloState.AgentPlanning => true,
        HaloState.AgentExecuting => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is a streaming state.
    /// </summary>
    public static bool IsStreaming(this HaloState state) => state switch
    {
        HaloState.Streaming => true,
        HaloState.MultiTurnStreaming => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is an error state.
    /// </summary>
    public static bool IsError(this HaloState state) => state switch
    {
        HaloState.Error => true,
        HaloState.ToolError => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is a success state.
    /// </summary>
    public static bool IsSuccess(this HaloState state) => state switch
    {
        HaloState.Success => true,
        HaloState.ToolSuccess => true,
        HaloState.AgentComplete => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is visible (Halo should be shown).
    /// </summary>
    public static bool IsVisible(this HaloState state) => state switch
    {
        HaloState.Hidden => false,
        HaloState.Idle => false,
        _ => true
    };

    /// <summary>
    /// Check if the state is a multi-turn state.
    /// </summary>
    public static bool IsMultiTurn(this HaloState state) => state switch
    {
        HaloState.MultiTurnActive => true,
        HaloState.MultiTurnThinking => true,
        HaloState.MultiTurnStreaming => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is a tool-related state.
    /// </summary>
    public static bool IsTool(this HaloState state) => state switch
    {
        HaloState.ToolExecuting => true,
        HaloState.ToolSuccess => true,
        HaloState.ToolError => true,
        _ => false
    };

    /// <summary>
    /// Check if the state is an agent-related state.
    /// </summary>
    public static bool IsAgent(this HaloState state) => state switch
    {
        HaloState.AgentPlanning => true,
        HaloState.AgentExecuting => true,
        HaloState.AgentComplete => true,
        _ => false
    };

    /// <summary>
    /// Get the display text for a state.
    /// </summary>
    public static string GetDisplayText(this HaloState state) => state switch
    {
        HaloState.Idle => "",
        HaloState.Hidden => "",
        HaloState.Appearing => "Appearing...",
        HaloState.Listening => "Listening...",
        HaloState.Thinking => "Thinking...",
        HaloState.Processing => "Processing...",
        HaloState.Streaming => "",
        HaloState.Success => "Done",
        HaloState.Error => "Error",
        HaloState.Disappearing => "",
        HaloState.MultiTurnActive => "Ready",
        HaloState.MultiTurnThinking => "Thinking...",
        HaloState.MultiTurnStreaming => "",
        HaloState.ToolExecuting => "Executing tool...",
        HaloState.ToolSuccess => "Tool completed",
        HaloState.ToolError => "Tool failed",
        HaloState.ClarificationNeeded => "Need clarification",
        HaloState.ClarificationReceived => "Got it",
        HaloState.AgentPlanning => "Planning...",
        HaloState.AgentExecuting => "Executing...",
        HaloState.AgentComplete => "Complete",
        _ => ""
    };

    /// <summary>
    /// Get the primary color for a state (hex string).
    /// </summary>
    public static (string Start, string End) GetGradientColors(this HaloState state) => state switch
    {
        // Purple - default/listening
        HaloState.Idle or HaloState.Listening or HaloState.Appearing =>
            ("#FF6B4EFF", "#FF9B6BFF"),

        // Blue - thinking/processing
        HaloState.Thinking or HaloState.Processing or HaloState.MultiTurnThinking =>
            ("#FF4ECAFF", "#FF6B9BFF"),

        // Green - streaming/success
        HaloState.Streaming or HaloState.Success or HaloState.MultiTurnStreaming or
        HaloState.ToolSuccess or HaloState.AgentComplete =>
            ("#FF4EFF6B", "#FF6BFFB9"),

        // Red - error
        HaloState.Error or HaloState.ToolError =>
            ("#FFFF4E4E", "#FFFF6B6B"),

        // Orange - tool executing
        HaloState.ToolExecuting =>
            ("#FFFFB04E", "#FFFFCF6B"),

        // Cyan - clarification
        HaloState.ClarificationNeeded or HaloState.ClarificationReceived =>
            ("#FF4EFFFF", "#FF6BFFFF"),

        // Indigo - agent
        HaloState.AgentPlanning or HaloState.AgentExecuting =>
            ("#FF4E6BFF", "#FF6B9BFF"),

        // Default purple
        _ => ("#FF6B4EFF", "#FF9B6BFF")
    };
}
