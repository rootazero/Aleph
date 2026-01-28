using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace POC.Hotkey;

/// <summary>
/// Main window for testing global hotkey detection.
/// </summary>
public sealed partial class MainWindow : Window
{
    private readonly HotkeyService _hotkeyService;
    private int _haloCount = 0;
    private int _conversationCount = 0;
    private readonly Microsoft.UI.Dispatching.DispatcherQueue _dispatcherQueue;

    public MainWindow(HotkeyService hotkeyService)
    {
        InitializeComponent();
        Title = "POC-2: Global Hotkey Test";

        _hotkeyService = hotkeyService;
        _dispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread();

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(600, 500));

        // Subscribe to hotkey events
        _hotkeyService.OnHaloHotkeyPressed += OnHaloHotkey;
        _hotkeyService.OnConversationHotkeyPressed += OnConversationHotkey;
        _hotkeyService.OnEscapePressed += OnEscapeHotkey;
        _hotkeyService.OnKeyEvent += OnKeyEvent;

        LogEvent("Hotkey service initialized. Ready for testing!");
    }

    private void OnHaloHotkey()
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            _haloCount++;
            HaloCountText.Text = _haloCount.ToString();
            HaloStatus.Text = $"Double-tap Shift: Triggered! ({_haloCount})";
            HaloBadge.Background = new SolidColorBrush(Colors.Green);

            LogEvent($"[HALO] Double-tap Shift detected! (Total: {_haloCount})");

            // Reset visual after delay
            _ = ResetBadgeAsync(HaloBadge, HaloStatus, "Double-tap Shift: Waiting...");
        });
    }

    private void OnConversationHotkey()
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            _conversationCount++;
            ConversationCountText.Text = _conversationCount.ToString();
            ConversationStatus.Text = $"Win+Alt+/: Triggered! ({_conversationCount})";
            ConversationBadge.Background = new SolidColorBrush(Colors.Green);

            LogEvent($"[CONVERSATION] Win + Alt + / detected! (Total: {_conversationCount})");

            // Reset visual after delay
            _ = ResetBadgeAsync(ConversationBadge, ConversationStatus, "Win+Alt+/: Waiting...");
        });
    }

    private void OnEscapeHotkey()
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            LogEvent("[ESCAPE] Escape key pressed");
        });
    }

    private void OnKeyEvent(string message)
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            if (ShowAllEvents.IsChecked == true || message.StartsWith(">>>"))
            {
                LogEvent(message);
            }
        });
    }

    private async Task ResetBadgeAsync(Border badge, TextBlock status, string defaultText)
    {
        await Task.Delay(2000);
        _dispatcherQueue.TryEnqueue(() =>
        {
            badge.Background = (Brush)Application.Current.Resources["SystemFillColorCriticalBackgroundBrush"];
            status.Text = defaultText;
        });
    }

    private void LogEvent(string message)
    {
        var timestamp = DateTime.Now.ToString("HH:mm:ss.fff");
        var logLine = $"[{timestamp}] {message}\n";

        EventLog.Text = logLine + EventLog.Text;

        // Keep log size manageable
        if (EventLog.Text.Length > 10000)
        {
            EventLog.Text = EventLog.Text[..8000];
        }
    }

    private void ClearLog_Click(object sender, RoutedEventArgs e)
    {
        EventLog.Text = "";
        LogEvent("Log cleared.");
    }
}
