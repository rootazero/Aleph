using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Graphics;
using Windows.System;
using Aleph.ViewModels;

namespace Aleph.Windows;

/// <summary>
/// Multi-turn conversation window.
///
/// Features:
/// - Message history display
/// - Streaming response indicator
/// - Keyboard shortcuts (Enter to send, Tab to select, Arrow keys to navigate, ESC to close)
/// - Slash commands (/ and //)
/// - Auto-scroll to latest message
/// - Mica backdrop for modern glass effect
/// - Right-click context menu for Clear action
/// - macOS-style layout: Input area only initially, messages appear after first message
/// - Auto-adaptive height: starts minimal, expands with content
/// </summary>
public sealed partial class ConversationWindow : Window
{
    private const int FixedWidth = 700;
    private const int MinHeight = 100;     // Only input box visible
    private const int MaxHeight = 800;     // Maximum window height
    private const int CommandItemHeight = 60;  // Height per command/topic item

    private Microsoft.UI.Windowing.AppWindow _appWindow;

    public ConversationViewModel ViewModel { get; }

    public ConversationWindow()
    {
        InitializeComponent();
        Title = "Aleph Conversation";

        // Get AppWindow reference for dynamic resizing
        _appWindow = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));

        // Set initial window size (minimal - only input box)
        _appWindow.Resize(new SizeInt32(FixedWidth, MinHeight));

        // Initialize ViewModel
        ViewModel = new ConversationViewModel();
        ViewModel.PropertyChanged += ViewModel_PropertyChanged;
        ViewModel.StreamingTextReceived += OnStreamingTextReceived;
        ViewModel.StreamingCompleted += OnStreamingCompleted;
        ViewModel.CloseRequested += OnCloseRequested;

        // Bind to messages collection changes for height updates
        ViewModel.Messages.CollectionChanged += Messages_CollectionChanged;
        ViewModel.FilteredCommands.CollectionChanged += (s, e) => UpdateWindowHeight();
        ViewModel.FilteredTopics.CollectionChanged += (s, e) => UpdateWindowHeight();
    }

    private void ViewModel_PropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(ConversationViewModel.IsStreaming):
                StreamingIndicator.Visibility = ViewModel.IsStreaming ? Visibility.Visible : Visibility.Collapsed;
                break;
            case nameof(ConversationViewModel.DisplayState):
            case nameof(ConversationViewModel.HasMessages):
                UpdateWindowHeight();
                break;
        }
    }

    private void Messages_CollectionChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
    {
        // Auto-scroll to bottom when new message is added
        ScrollToBottom();
        // Update window height when messages change
        UpdateWindowHeight();
    }

    /// <summary>
    /// Update window height based on content state.
    /// - Initial/Empty: MinHeight (only input box)
    /// - CommandList/TopicList: Based on item count
    /// - Conversation with messages: MaxHeight
    /// </summary>
    private void UpdateWindowHeight()
    {
        int targetHeight = MinHeight;

        if (ViewModel.IsShowingCommands)
        {
            // Calculate height based on command count
            var itemCount = ViewModel.FilteredCommands.Count;
            targetHeight = Math.Min(MinHeight + (itemCount * CommandItemHeight), MaxHeight);
        }
        else if (ViewModel.IsShowingTopics)
        {
            // Calculate height based on topic count
            var itemCount = ViewModel.FilteredTopics.Count;
            targetHeight = Math.Min(MinHeight + (itemCount * CommandItemHeight), MaxHeight);
        }
        else if (ViewModel.HasMessages)
        {
            // When conversation has messages, use max height
            targetHeight = MaxHeight;
        }

        // Resize window
        _appWindow.Resize(new SizeInt32(FixedWidth, targetHeight));
    }

    private void OnStreamingTextReceived(string text)
    {
        // Update UI during streaming if needed
        ScrollToBottom();
    }

    private void OnStreamingCompleted()
    {
        ScrollToBottom();
    }

    private void OnCloseRequested()
    {
        // Close the window when ESC is pressed (and not in list mode)
        Close();
    }

    private void ScrollToBottom()
    {
        // Dispatch to ensure layout is updated
        DispatcherQueue.TryEnqueue(() =>
        {
            MessagesScrollViewer.ChangeView(null, MessagesScrollViewer.ScrollableHeight, null);
        });
    }

    private void InputTextBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        switch (e.Key)
        {
            case VirtualKey.Enter when !IsShiftPressed():
                e.Handled = true;
                if (ViewModel.IsShowingCommands || ViewModel.IsShowingTopics)
                {
                    // Select current item when Enter is pressed in list mode
                    ViewModel.HandleTab();
                }
                else
                {
                    _ = SendMessageAsync();
                }
                break;

            case VirtualKey.Tab:
                e.Handled = true;
                ViewModel.HandleTab();
                break;

            case VirtualKey.Up:
                if (ViewModel.IsShowingCommands || ViewModel.IsShowingTopics)
                {
                    e.Handled = true;
                    ViewModel.MoveSelectionUp();
                }
                break;

            case VirtualKey.Down:
                if (ViewModel.IsShowingCommands || ViewModel.IsShowingTopics)
                {
                    e.Handled = true;
                    ViewModel.MoveSelectionDown();
                }
                break;

            case VirtualKey.Escape:
                e.Handled = true;
                ViewModel.HandleEscape();
                break;
        }
    }

    private static bool IsShiftPressed()
    {
        var state = Microsoft.UI.Input.InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Shift);
        return state.HasFlag(global::Windows.UI.Core.CoreVirtualKeyStates.Down);
    }

    private async void SendButton_Click(object sender, RoutedEventArgs e)
    {
        await SendMessageAsync();
    }

    private async Task SendMessageAsync()
    {
        if (ViewModel.SendMessageCommand.CanExecute(null))
        {
            await ViewModel.SendMessageAsync();
            InputTextBox.Focus(FocusState.Programmatic);
        }
    }

    private void StopButton_Click(object sender, RoutedEventArgs e)
    {
        ViewModel.StopStreamingCommand.Execute(null);
    }

    private async void ClearButton_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new ContentDialog
        {
            Title = "Clear Conversation",
            Content = "Are you sure you want to clear the conversation history?",
            PrimaryButtonText = "Clear",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = this.Content.XamlRoot
        };

        var result = await dialog.ShowAsync();

        if (result == ContentDialogResult.Primary)
        {
            ViewModel.ClearConversationCommand.Execute(null);
        }
    }

    private void CommandListView_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is CommandItem)
        {
            ViewModel.HandleTab();
            InputTextBox.Focus(FocusState.Programmatic);
        }
    }

    private void TopicListView_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is TopicItem)
        {
            ViewModel.HandleTab();
            InputTextBox.Focus(FocusState.Programmatic);
        }
    }
}
