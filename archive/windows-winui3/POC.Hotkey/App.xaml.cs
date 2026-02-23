using Microsoft.UI.Xaml;

namespace POC.Hotkey;

/// <summary>
/// POC Application - Tests global hotkey with low-level keyboard hook
/// </summary>
public partial class App : Application
{
    private MainWindow? _mainWindow;
    private HotkeyService? _hotkeyService;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Initialize hotkey service
        _hotkeyService = new HotkeyService();

        // Create main window
        _mainWindow = new MainWindow(_hotkeyService);
        _mainWindow.Closed += (s, e) =>
        {
            _hotkeyService?.Dispose();
        };

        _mainWindow.Activate();
    }
}
