using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Aether.Models;

namespace Aether.ViewModels;

/// <summary>
/// ViewModel for HaloWindow.
///
/// Manages:
/// - Complete 21-state Halo state machine
/// - Streaming text with cursor animation
/// - Tool execution status
/// - Error handling
/// - Multi-turn conversation
/// </summary>
public partial class HaloViewModel : ObservableObject
{
    #region Observable Properties - State

    [ObservableProperty]
    private HaloState _state = HaloState.Hidden;

    [ObservableProperty]
    private string _statusText = "";

    [ObservableProperty]
    private string _streamingText = "";

    [ObservableProperty]
    private string _errorMessage = "";

    #endregion

    #region Observable Properties - UI State

    [ObservableProperty]
    private bool _isThinking;

    [ObservableProperty]
    private bool _isStreaming;

    [ObservableProperty]
    private bool _hasError;

    [ObservableProperty]
    private bool _isToolExecuting;

    [ObservableProperty]
    private bool _isMultiTurn;

    [ObservableProperty]
    private bool _isAgentMode;

    [ObservableProperty]
    private bool _needsClarification;

    #endregion

    #region Observable Properties - Tool Execution

    [ObservableProperty]
    private string _toolName = "";

    [ObservableProperty]
    private string _toolStatus = "";

    [ObservableProperty]
    private string _toolResult = "";

    [ObservableProperty]
    private bool _toolHasError;

    #endregion

    #region Observable Properties - Gradient Colors

    [ObservableProperty]
    private string _gradientStartColor = "#FF6B4EFF";

    [ObservableProperty]
    private string _gradientEndColor = "#FF9B6BFF";

    #endregion

    public HaloViewModel()
    {
        PropertyChanged += (s, e) =>
        {
            if (e.PropertyName == nameof(State))
            {
                UpdateStateProperties();
            }
        };
    }

    /// <summary>
    /// Transition to a new state.
    /// </summary>
    public void TransitionTo(HaloState newState)
    {
        if (State == newState) return;

        var oldState = State;
        State = newState;

        System.Diagnostics.Debug.WriteLine($"[HaloVM] State: {oldState} -> {newState}");
    }

    #region Commands - Basic Flow

    /// <summary>
    /// Start listening for input.
    /// </summary>
    [RelayCommand]
    public void StartListening()
    {
        ClearStreamingText();
        ClearError();
        ClearTool();
        TransitionTo(HaloState.Listening);
    }

    /// <summary>
    /// Start thinking/processing.
    /// </summary>
    [RelayCommand]
    public void StartThinking()
    {
        TransitionTo(HaloState.Thinking);
    }

    /// <summary>
    /// Start streaming response.
    /// </summary>
    [RelayCommand]
    public void StartStreaming()
    {
        ClearStreamingText();
        TransitionTo(HaloState.Streaming);
    }

    /// <summary>
    /// Mark as complete.
    /// </summary>
    [RelayCommand]
    public void Complete()
    {
        TransitionTo(HaloState.Success);
    }

    /// <summary>
    /// Hide the Halo.
    /// </summary>
    [RelayCommand]
    public void Hide()
    {
        TransitionTo(HaloState.Hidden);
    }

    #endregion

    #region Commands - Multi-Turn

    /// <summary>
    /// Enter multi-turn conversation mode.
    /// </summary>
    [RelayCommand]
    public void StartMultiTurn()
    {
        TransitionTo(HaloState.MultiTurnActive);
    }

    /// <summary>
    /// Start thinking in multi-turn mode.
    /// </summary>
    [RelayCommand]
    public void MultiTurnThink()
    {
        TransitionTo(HaloState.MultiTurnThinking);
    }

    /// <summary>
    /// Start streaming in multi-turn mode.
    /// </summary>
    [RelayCommand]
    public void MultiTurnStream()
    {
        ClearStreamingText();
        TransitionTo(HaloState.MultiTurnStreaming);
    }

    #endregion

    #region Commands - Tool Execution

    /// <summary>
    /// Start tool execution.
    /// </summary>
    public void StartToolExecution(string toolName)
    {
        ToolName = toolName;
        ToolStatus = "Executing";
        ToolResult = "";
        ToolHasError = false;
        TransitionTo(HaloState.ToolExecuting);
    }

    /// <summary>
    /// Mark tool as completed successfully.
    /// </summary>
    public void ToolComplete(string? result = null)
    {
        ToolStatus = "Completed";
        ToolResult = result ?? "";
        ToolHasError = false;
        TransitionTo(HaloState.ToolSuccess);
    }

    /// <summary>
    /// Mark tool as failed.
    /// </summary>
    public void ToolFailed(string error)
    {
        ToolStatus = "Failed";
        ToolResult = error;
        ToolHasError = true;
        TransitionTo(HaloState.ToolError);
    }

    #endregion

    #region Commands - Clarification

    /// <summary>
    /// Request clarification from user.
    /// </summary>
    [RelayCommand]
    public void RequestClarification()
    {
        TransitionTo(HaloState.ClarificationNeeded);
    }

    /// <summary>
    /// Clarification received.
    /// </summary>
    [RelayCommand]
    public void ClarificationReceived()
    {
        TransitionTo(HaloState.ClarificationReceived);
    }

    #endregion

    #region Commands - Agent Mode

    /// <summary>
    /// Start agent planning.
    /// </summary>
    [RelayCommand]
    public void StartAgentPlanning()
    {
        TransitionTo(HaloState.AgentPlanning);
    }

    /// <summary>
    /// Start agent execution.
    /// </summary>
    [RelayCommand]
    public void StartAgentExecuting()
    {
        TransitionTo(HaloState.AgentExecuting);
    }

    /// <summary>
    /// Agent completed.
    /// </summary>
    [RelayCommand]
    public void AgentComplete()
    {
        TransitionTo(HaloState.AgentComplete);
    }

    #endregion

    #region Text Management

    /// <summary>
    /// Append text during streaming.
    /// </summary>
    public void AppendText(string text)
    {
        StreamingText += text;
    }

    /// <summary>
    /// Set error state.
    /// </summary>
    public void SetError(string message)
    {
        ErrorMessage = message;
        TransitionTo(HaloState.Error);
    }

    /// <summary>
    /// Clear streaming text.
    /// </summary>
    public void ClearStreamingText()
    {
        StreamingText = "";
    }

    /// <summary>
    /// Clear error.
    /// </summary>
    public void ClearError()
    {
        ErrorMessage = "";
        HasError = false;
    }

    /// <summary>
    /// Clear tool state.
    /// </summary>
    public void ClearTool()
    {
        ToolName = "";
        ToolStatus = "";
        ToolResult = "";
        ToolHasError = false;
    }

    #endregion

    #region State Property Updates

    private void UpdateStateProperties()
    {
        // Reset all flags first
        IsThinking = false;
        IsStreaming = false;
        HasError = false;
        IsToolExecuting = false;
        IsMultiTurn = false;
        IsAgentMode = false;
        NeedsClarification = false;

        // Update status text and flags based on state
        StatusText = State.GetDisplayText();
        var colors = State.GetGradientColors();
        GradientStartColor = colors.Start;
        GradientEndColor = colors.End;

        // Set specific flags
        IsThinking = State.IsLoading();
        IsStreaming = State.IsStreaming();
        HasError = State.IsError();
        IsToolExecuting = State.IsTool();
        IsMultiTurn = State.IsMultiTurn();
        IsAgentMode = State.IsAgent();
        NeedsClarification = State is HaloState.ClarificationNeeded or HaloState.ClarificationReceived;
    }

    #endregion
}
