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
/// - Slash commands (/ and //)
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

    [ObservableProperty]
    private ContentDisplayState _displayState = ContentDisplayState.Empty;

    [ObservableProperty]
    private int _selectedCommandIndex = -1;

    [ObservableProperty]
    private int _selectedTopicIndex = -1;

    #endregion

    /// <summary>
    /// Collection of messages in the conversation.
    /// </summary>
    public ObservableCollection<ConversationMessage> Messages { get; } = new();

    /// <summary>
    /// Collection of available commands for slash menu.
    /// </summary>
    public ObservableCollection<CommandItem> FilteredCommands { get; } = new();

    /// <summary>
    /// Collection of available topics for double-slash menu.
    /// </summary>
    public ObservableCollection<TopicItem> FilteredTopics { get; } = new();

    /// <summary>
    /// All available commands.
    /// </summary>
    private readonly List<CommandItem> _allCommands = new()
    {
        new CommandItem { Name = "clear", Description = "Clear the conversation" },
        new CommandItem { Name = "copy", Description = "Copy the last response" },
        new CommandItem { Name = "regenerate", Description = "Regenerate the last response" },
        new CommandItem { Name = "help", Description = "Show available commands" },
        new CommandItem { Name = "settings", Description = "Open settings" },
    };

    /// <summary>
    /// All available topics/conversations.
    /// </summary>
    private readonly List<TopicItem> _allTopics = new()
    {
        new TopicItem { Title = "New Conversation", Description = "Start a fresh conversation" },
    };

    /// <summary>
    /// Event raised when streaming text is received.
    /// </summary>
    public event Action<string>? StreamingTextReceived;

    /// <summary>
    /// Event raised when streaming completes.
    /// </summary>
    public event Action? StreamingCompleted;

    /// <summary>
    /// Event raised when the window should be closed.
    /// </summary>
    public event Action? CloseRequested;

    /// <summary>
    /// Whether the messages list has any messages.
    /// </summary>
    public bool HasMessages => Messages.Count > 0;

    /// <summary>
    /// Whether the command list is showing.
    /// </summary>
    public bool IsShowingCommands => DisplayState == ContentDisplayState.CommandList;

    /// <summary>
    /// Whether the topic list is showing.
    /// </summary>
    public bool IsShowingTopics => DisplayState == ContentDisplayState.TopicList;

    /// <summary>
    /// Whether the conversation view is showing.
    /// </summary>
    public bool IsShowingConversation => DisplayState == ContentDisplayState.Conversation;

    public ConversationViewModel()
    {
        // No welcome message - start with empty state (macOS style)
        Messages.CollectionChanged += (s, e) =>
        {
            OnPropertyChanged(nameof(HasMessages));
            UpdateDisplayState();
        };
    }

    /// <summary>
    /// Called when InputText changes to update display state.
    /// </summary>
    partial void OnInputTextChanged(string value)
    {
        UpdateDisplayState();
    }

    /// <summary>
    /// Update the display state based on input text and messages.
    /// </summary>
    private void UpdateDisplayState()
    {
        var previousState = DisplayState;

        if (InputText.StartsWith("//"))
        {
            DisplayState = ContentDisplayState.TopicList;
            FilterTopics(InputText[2..]);
        }
        else if (InputText.StartsWith("/"))
        {
            DisplayState = ContentDisplayState.CommandList;
            FilterCommands(InputText[1..]);
        }
        else if (Messages.Count > 0)
        {
            DisplayState = ContentDisplayState.Conversation;
        }
        else
        {
            DisplayState = ContentDisplayState.Empty;
        }

        // Reset selection when state changes
        if (previousState != DisplayState)
        {
            SelectedCommandIndex = FilteredCommands.Count > 0 ? 0 : -1;
            SelectedTopicIndex = FilteredTopics.Count > 0 ? 0 : -1;
        }

        OnPropertyChanged(nameof(IsShowingCommands));
        OnPropertyChanged(nameof(IsShowingTopics));
        OnPropertyChanged(nameof(IsShowingConversation));
    }

    /// <summary>
    /// Filter commands based on search text.
    /// </summary>
    private void FilterCommands(string searchText)
    {
        FilteredCommands.Clear();
        var filtered = string.IsNullOrEmpty(searchText)
            ? _allCommands
            : _allCommands.Where(c => c.Name.Contains(searchText, StringComparison.OrdinalIgnoreCase)).ToList();

        foreach (var cmd in filtered)
        {
            FilteredCommands.Add(cmd);
        }

        if (SelectedCommandIndex >= FilteredCommands.Count)
        {
            SelectedCommandIndex = FilteredCommands.Count > 0 ? 0 : -1;
        }
    }

    /// <summary>
    /// Filter topics based on search text.
    /// </summary>
    private void FilterTopics(string searchText)
    {
        FilteredTopics.Clear();
        var filtered = string.IsNullOrEmpty(searchText)
            ? _allTopics
            : _allTopics.Where(t => t.Title.Contains(searchText, StringComparison.OrdinalIgnoreCase)).ToList();

        foreach (var topic in filtered)
        {
            FilteredTopics.Add(topic);
        }

        if (SelectedTopicIndex >= FilteredTopics.Count)
        {
            SelectedTopicIndex = FilteredTopics.Count > 0 ? 0 : -1;
        }
    }

    /// <summary>
    /// Move selection up in the current list.
    /// </summary>
    public void MoveSelectionUp()
    {
        if (IsShowingCommands && FilteredCommands.Count > 0)
        {
            SelectedCommandIndex = SelectedCommandIndex <= 0
                ? FilteredCommands.Count - 1
                : SelectedCommandIndex - 1;
        }
        else if (IsShowingTopics && FilteredTopics.Count > 0)
        {
            SelectedTopicIndex = SelectedTopicIndex <= 0
                ? FilteredTopics.Count - 1
                : SelectedTopicIndex - 1;
        }
    }

    /// <summary>
    /// Move selection down in the current list.
    /// </summary>
    public void MoveSelectionDown()
    {
        if (IsShowingCommands && FilteredCommands.Count > 0)
        {
            SelectedCommandIndex = SelectedCommandIndex >= FilteredCommands.Count - 1
                ? 0
                : SelectedCommandIndex + 1;
        }
        else if (IsShowingTopics && FilteredTopics.Count > 0)
        {
            SelectedTopicIndex = SelectedTopicIndex >= FilteredTopics.Count - 1
                ? 0
                : SelectedTopicIndex + 1;
        }
    }

    /// <summary>
    /// Handle Tab key to select current item.
    /// </summary>
    public void HandleTab()
    {
        if (IsShowingCommands && SelectedCommandIndex >= 0 && SelectedCommandIndex < FilteredCommands.Count)
        {
            var command = FilteredCommands[SelectedCommandIndex];
            ExecuteCommand(command.Name);
        }
        else if (IsShowingTopics && SelectedTopicIndex >= 0 && SelectedTopicIndex < FilteredTopics.Count)
        {
            var topic = FilteredTopics[SelectedTopicIndex];
            SelectTopic(topic);
        }
    }

    /// <summary>
    /// Handle Escape key with layered exit behavior.
    /// </summary>
    public void HandleEscape()
    {
        if (IsShowingCommands || IsShowingTopics)
        {
            // First ESC: Clear input and exit list mode
            InputText = "";
        }
        else
        {
            // Second ESC: Close the window
            CloseRequested?.Invoke();
        }
    }

    /// <summary>
    /// Execute a slash command.
    /// </summary>
    private void ExecuteCommand(string commandName)
    {
        InputText = "";

        switch (commandName.ToLowerInvariant())
        {
            case "clear":
                ClearConversation();
                break;
            case "copy":
                CopyLastResponse();
                break;
            case "regenerate":
                RegenerateResponse();
                break;
            case "help":
                ShowHelpMessage();
                break;
            case "settings":
                // TODO: Open settings window
                break;
        }
    }

    /// <summary>
    /// Select a topic/conversation.
    /// </summary>
    private void SelectTopic(TopicItem topic)
    {
        InputText = "";

        if (topic.Title == "New Conversation")
        {
            Messages.Clear();
        }
        // TODO: Load existing conversation by topic
    }

    /// <summary>
    /// Show help message in conversation.
    /// </summary>
    private void ShowHelpMessage()
    {
        var helpText = "Available commands:\n" +
                       "/clear - Clear the conversation\n" +
                       "/copy - Copy the last response\n" +
                       "/regenerate - Regenerate the last response\n" +
                       "/help - Show this help message\n" +
                       "/settings - Open settings\n\n" +
                       "Use // to access conversation history.";

        Messages.Add(new ConversationMessage
        {
            Role = MessageRole.System,
            Content = helpText,
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
        // No message after clear - macOS style empty state
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
    /// Get the first character of the role display (for avatar).
    /// </summary>
    public string RoleInitial => RoleDisplay.Length > 0 ? RoleDisplay[0].ToString() : "?";

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

/// <summary>
/// Represents a slash command item.
/// </summary>
public class CommandItem
{
    /// <summary>
    /// Command name (without the leading /).
    /// </summary>
    public string Name { get; set; } = "";

    /// <summary>
    /// Description of what the command does.
    /// </summary>
    public string Description { get; set; } = "";
}

/// <summary>
/// Represents a topic/conversation item.
/// </summary>
public class TopicItem
{
    /// <summary>
    /// Topic/conversation title.
    /// </summary>
    public string Title { get; set; } = "";

    /// <summary>
    /// Description or preview of the topic.
    /// </summary>
    public string Description { get; set; } = "";

    /// <summary>
    /// Unique identifier for the topic.
    /// </summary>
    public string Id { get; set; } = "";
}
