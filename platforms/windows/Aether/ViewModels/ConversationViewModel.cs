using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Aether.ViewModels;

/// <summary>
/// ViewModel for ConversationWindow.
///
/// Manages:
/// - Conversation history
/// - Message input
/// - Streaming responses
/// - Multi-turn state
/// </summary>
public partial class ConversationViewModel : ObservableObject
{
    #region Observable Properties

    [ObservableProperty]
    private string _inputText = "";

    [ObservableProperty]
    private bool _isProcessing;

    [ObservableProperty]
    private bool _isStreaming;

    [ObservableProperty]
    private string _currentStreamingText = "";

    [ObservableProperty]
    private string _statusText = "Ready";

    [ObservableProperty]
    private string _conversationTitle = "New Conversation";

    [ObservableProperty]
    private bool _canSend = true;

    #endregion

    /// <summary>
    /// Collection of messages in the conversation.
    /// </summary>
    public ObservableCollection<ConversationMessage> Messages { get; } = new();

    /// <summary>
    /// Event raised when streaming text is received.
    /// </summary>
    public event Action<string>? StreamingTextReceived;

    /// <summary>
    /// Event raised when streaming completes.
    /// </summary>
    public event Action? StreamingCompleted;

    public ConversationViewModel()
    {
        // Add welcome message
        Messages.Add(new ConversationMessage
        {
            Role = MessageRole.System,
            Content = "Welcome to Aether. How can I help you today?",
            Timestamp = DateTime.Now
        });
    }

    #region Commands

    [RelayCommand(CanExecute = nameof(CanSendMessage))]
    public async Task SendMessageAsync()
    {
        if (string.IsNullOrWhiteSpace(InputText)) return;

        var userMessage = InputText.Trim();
        InputText = "";

        // Add user message
        Messages.Add(new ConversationMessage
        {
            Role = MessageRole.User,
            Content = userMessage,
            Timestamp = DateTime.Now
        });

        // Start processing
        IsProcessing = true;
        CanSend = false;
        StatusText = "Thinking...";

        try
        {
            // Add assistant message placeholder
            var assistantMessage = new ConversationMessage
            {
                Role = MessageRole.Assistant,
                Content = "",
                Timestamp = DateTime.Now,
                IsStreaming = true
            };
            Messages.Add(assistantMessage);

            // TODO: Call Rust core to get response
            // For now, simulate streaming response
            await SimulateStreamingResponseAsync(assistantMessage);

            assistantMessage.IsStreaming = false;
            StatusText = "Ready";
        }
        catch (Exception ex)
        {
            // Add error message
            Messages.Add(new ConversationMessage
            {
                Role = MessageRole.System,
                Content = $"Error: {ex.Message}",
                Timestamp = DateTime.Now,
                IsError = true
            });
            StatusText = "Error occurred";
        }
        finally
        {
            IsProcessing = false;
            CanSend = true;
        }
    }

    private bool CanSendMessage() => CanSend && !string.IsNullOrWhiteSpace(InputText);

    [RelayCommand]
    public void ClearConversation()
    {
        Messages.Clear();
        ConversationTitle = "New Conversation";
        Messages.Add(new ConversationMessage
        {
            Role = MessageRole.System,
            Content = "Conversation cleared. How can I help you?",
            Timestamp = DateTime.Now
        });
    }

    [RelayCommand]
    public void StopStreaming()
    {
        // TODO: Cancel the current streaming request
        IsStreaming = false;
        StatusText = "Stopped";
    }

    [RelayCommand]
    public void CopyLastResponse()
    {
        var lastAssistant = Messages.LastOrDefault(m => m.Role == MessageRole.Assistant);
        if (lastAssistant != null)
        {
            // TODO: Copy to clipboard via ClipboardService
            System.Diagnostics.Debug.WriteLine($"[Conversation] Copy: {lastAssistant.Content}");
        }
    }

    [RelayCommand]
    public void RegenerateResponse()
    {
        // TODO: Regenerate the last response
        System.Diagnostics.Debug.WriteLine("[Conversation] Regenerate requested");
    }

    #endregion

    #region Streaming

    /// <summary>
    /// Append text during streaming (called from Rust callback).
    /// </summary>
    public void AppendStreamingText(string text)
    {
        CurrentStreamingText += text;

        var lastMessage = Messages.LastOrDefault(m => m.Role == MessageRole.Assistant && m.IsStreaming);
        if (lastMessage != null)
        {
            lastMessage.Content += text;
        }

        StreamingTextReceived?.Invoke(text);
    }

    /// <summary>
    /// Complete the current streaming response.
    /// </summary>
    public void CompleteStreaming()
    {
        IsStreaming = false;
        CurrentStreamingText = "";

        var lastMessage = Messages.LastOrDefault(m => m.Role == MessageRole.Assistant && m.IsStreaming);
        if (lastMessage != null)
        {
            lastMessage.IsStreaming = false;
        }

        StreamingCompleted?.Invoke();
    }

    private async Task SimulateStreamingResponseAsync(ConversationMessage message)
    {
        IsStreaming = true;
        StatusText = "Streaming...";

        // Simulate streaming response
        var response = "I'm Aether, your AI assistant running on Windows. This is a simulated response to demonstrate the streaming functionality. In production, this would be connected to the Rust core which handles AI provider communication.";

        foreach (var word in response.Split(' '))
        {
            await Task.Delay(50);
            message.Content += (message.Content.Length > 0 ? " " : "") + word;
            StreamingTextReceived?.Invoke(word + " ");
        }

        IsStreaming = false;
        StreamingCompleted?.Invoke();
    }

    #endregion
}

/// <summary>
/// Represents a message in the conversation.
/// </summary>
public partial class ConversationMessage : ObservableObject
{
    [ObservableProperty]
    private MessageRole _role;

    [ObservableProperty]
    private string _content = "";

    [ObservableProperty]
    private DateTime _timestamp;

    [ObservableProperty]
    private bool _isStreaming;

    [ObservableProperty]
    private bool _isError;

    /// <summary>
    /// Get the display name for the role.
    /// </summary>
    public string RoleDisplay => Role switch
    {
        MessageRole.User => "You",
        MessageRole.Assistant => "Aether",
        MessageRole.System => "System",
        _ => "Unknown"
    };

    /// <summary>
    /// Get the timestamp display string.
    /// </summary>
    public string TimestampDisplay => Timestamp.ToString("HH:mm");
}

/// <summary>
/// Message roles in a conversation.
/// </summary>
public enum MessageRole
{
    User,
    Assistant,
    System
}
