using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Aether.Models;

namespace Aether.ViewModels;

/// <summary>
/// ViewModel for HaloWindow.
///
/// Manages:
/// - Halo state machine (6 core states in Phase 1)
/// - Streaming text
/// - Error handling
/// </summary>
public partial class HaloViewModel : ObservableObject
{
    #region Observable Properties

    [ObservableProperty]
    private HaloState _state = HaloState.Hidden;

    [ObservableProperty]
    private string _statusText = "";

    [ObservableProperty]
    private string _streamingText = "";

    [ObservableProperty]
    private string _errorMessage = "";

    [ObservableProperty]
    private bool _isThinking = false;

    [ObservableProperty]
    private bool _isStreaming = false;

    [ObservableProperty]
    private bool _hasError = false;

    #endregion

    public HaloViewModel()
    {
        // Subscribe to state changes
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

    /// <summary>
    /// Start listening for input.
    /// </summary>
    [RelayCommand]
    public void StartListening()
    {
        ClearStreamingText();
        ClearError();
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
    /// Append text during streaming.
    /// </summary>
    public void AppendText(string text)
    {
        StreamingText += text;
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
    /// Set error state.
    /// </summary>
    public void SetError(string message)
    {
        ErrorMessage = message;
        TransitionTo(HaloState.Error);
    }

    /// <summary>
    /// Hide the Halo.
    /// </summary>
    [RelayCommand]
    public void Hide()
    {
        TransitionTo(HaloState.Hidden);
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

    private void UpdateStateProperties()
    {
        switch (State)
        {
            case HaloState.Hidden:
                StatusText = "";
                IsThinking = false;
                IsStreaming = false;
                HasError = false;
                break;

            case HaloState.Listening:
                StatusText = "Listening...";
                IsThinking = false;
                IsStreaming = false;
                HasError = false;
                break;

            case HaloState.Thinking:
                StatusText = "Thinking...";
                IsThinking = true;
                IsStreaming = false;
                HasError = false;
                break;

            case HaloState.Processing:
                StatusText = "Processing...";
                IsThinking = true;
                IsStreaming = false;
                HasError = false;
                break;

            case HaloState.Streaming:
                StatusText = "";
                IsThinking = false;
                IsStreaming = true;
                HasError = false;
                break;

            case HaloState.Success:
                StatusText = "Done";
                IsThinking = false;
                IsStreaming = false;
                HasError = false;
                break;

            case HaloState.Error:
                StatusText = "Error";
                IsThinking = false;
                IsStreaming = false;
                HasError = true;
                break;
        }
    }
}
