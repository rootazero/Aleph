using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Graphics;
using Windows.System;
using Aether.ViewModels;

namespace Aether.Windows;

/// <summary>
/// Multi-turn conversation window.
///
/// Features:
/// - Message history display
/// - Streaming response indicator
/// - Keyboard shortcuts (Enter to send)
/// - Auto-scroll to latest message
/// </summary>
public sealed partial class ConversationWindow : Window
{
    public ConversationViewModel ViewModel { get; }

    public ConversationWindow()
    {
        InitializeComponent();
        Title = "Aether Conversation";

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(650, 750));

        // Initialize ViewModel
        ViewModel = new ConversationViewModel();
        ViewModel.PropertyChanged += ViewModel_PropertyChanged;
        ViewModel.StreamingTextReceived += OnStreamingTextReceived;
        ViewModel.StreamingCompleted += OnStreamingCompleted;

        // Bind to messages collection changes
        ViewModel.Messages.CollectionChanged += Messages_CollectionChanged;
    }

    private void ViewModel_PropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(ConversationViewModel.StatusText):
                StatusText.Text = ViewModel.StatusText;
                break;
            case nameof(ConversationViewModel.ConversationTitle):
                TitleText.Text = ViewModel.ConversationTitle;
                break;
            case nameof(ConversationViewModel.IsStreaming):
                StreamingIndicator.Visibility = ViewModel.IsStreaming ? Visibility.Visible : Visibility.Collapsed;
                break;
        }
    }

    private void Messages_CollectionChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
    {
        // Auto-scroll to bottom when new message is added
        ScrollToBottom();
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
        if (e.Key == VirtualKey.Enter && !IsShiftPressed())
        {
            e.Handled = true;
            _ = SendMessageAsync();
        }
    }

    private static bool IsShiftPressed()
    {
        var state = Microsoft.UI.Input.InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Shift);
        return state.HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
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

    private void SettingsButton_Click(object sender, RoutedEventArgs e)
    {
        // Open settings window
        var settingsWindow = new SettingsWindow();
        settingsWindow.Activate();
    }
}
