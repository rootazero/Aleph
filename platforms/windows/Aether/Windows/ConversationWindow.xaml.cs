using Microsoft.UI.Xaml;
using Windows.Graphics;

namespace Aether.Windows;

/// <summary>
/// Multi-turn conversation window - placeholder for Phase 3.
/// </summary>
public sealed partial class ConversationWindow : Window
{
    public ConversationWindow()
    {
        InitializeComponent();
        Title = "Aether Conversation";

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(600, 700));
    }

    private void CloseButton_Click(object sender, RoutedEventArgs e)
    {
        Close();
    }
}
