// Aether Windows Application
// Main entry point - manages lifecycle, services, and windows

using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Aether.Services;
using Aether.Interop;
using Aether.Windows;

namespace Aether;

/// <summary>
/// Aether Windows application entry point.
///
/// Responsibilities:
/// - Single instance enforcement
/// - Service initialization (hotkey, tray, core)
/// - Window management (Halo, Settings, Conversation)
/// - Application lifecycle
/// </summary>
public partial class App : Application
{
    #region Singleton

    private static App? _instance;
    public static App Instance => _instance ?? throw new InvalidOperationException("App not initialized");

    public static DispatcherQueue DispatcherQueue => Instance._dispatcherQueue
        ?? throw new InvalidOperationException("DispatcherQueue not available");

    #endregion

    #region Services

    private DispatcherQueue? _dispatcherQueue;
    private TrayIconService? _trayIconService;
    private HotkeyService? _hotkeyService;
    private AetherCore? _aetherCore;

    public TrayIconService TrayIcon => _trayIconService
        ?? throw new InvalidOperationException("TrayIconService not initialized");

    public HotkeyService Hotkeys => _hotkeyService
        ?? throw new InvalidOperationException("HotkeyService not initialized");

    public AetherCore Core => _aetherCore
        ?? throw new InvalidOperationException("AetherCore not initialized");

    #endregion

    #region Windows

    private HaloWindow? _haloWindow;
    private SettingsWindow? _settingsWindow;
    private ConversationWindow? _conversationWindow;

    #endregion

    public App()
    {
        _instance = this;
        InitializeComponent();
        UnhandledException += OnUnhandledException;
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _dispatcherQueue = DispatcherQueue.GetForCurrentThread();

        // Initialize services
        InitializeServices();

        // Create windows (but don't show yet)
        CreateWindows();

        // Wire up hotkey handlers
        WireUpHotkeys();

        // Show tray icon
        _trayIconService?.Show();

        System.Diagnostics.Debug.WriteLine("Aether started successfully");
    }

    private void InitializeServices()
    {
        try
        {
            // 1. Initialize Rust core
            _aetherCore = new AetherCore(_dispatcherQueue!);
            var configPath = GetConfigPath();
            if (!_aetherCore.Initialize(configPath))
            {
                System.Diagnostics.Debug.WriteLine("Warning: Aether core initialization failed (DLL may be missing)");
            }

            // 2. Initialize hotkey service
            _hotkeyService = new HotkeyService();

            // 3. Initialize tray icon service
            _trayIconService = new TrayIconService();
            _trayIconService.SettingsRequested += ShowSettings;
            _trayIconService.QuitRequested += Quit;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Service initialization error: {ex.Message}");
        }
    }

    private void CreateWindows()
    {
        // Create Halo window (hidden initially)
        _haloWindow = new HaloWindow();

        // Settings and Conversation windows are created on demand
    }

    private void WireUpHotkeys()
    {
        if (_hotkeyService == null) return;

        // Double-tap Shift -> Show/Toggle Halo
        _hotkeyService.OnHaloHotkeyPressed += () =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                if (_haloWindow?.IsVisible == true)
                {
                    _haloWindow.Hide();
                }
                else
                {
                    ShowHaloAtCursor();
                }
            });
        };

        // Win + Alt + / -> Show Conversation window
        _hotkeyService.OnConversationHotkeyPressed += () =>
        {
            _dispatcherQueue?.TryEnqueue(ShowConversation);
        };

        // Escape -> Hide Halo
        _hotkeyService.OnEscapePressed += () =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                _haloWindow?.Hide();
            });
        };
    }

    #region Window Management

    public void ShowHaloAtCursor()
    {
        if (_haloWindow == null) return;

        var (x, y) = GetCursorPosition();
        _haloWindow.ShowAt(x + 20, y + 20);
    }

    public void HideHalo()
    {
        _haloWindow?.Hide();
    }

    public void ShowSettings()
    {
        if (_settingsWindow == null)
        {
            _settingsWindow = new SettingsWindow();
            _settingsWindow.Closed += (s, e) => _settingsWindow = null;
        }
        _settingsWindow.Activate();
    }

    public void ShowConversation()
    {
        if (_conversationWindow == null)
        {
            _conversationWindow = new ConversationWindow();
            _conversationWindow.Closed += (s, e) => _conversationWindow = null;
        }
        _conversationWindow.Activate();
    }

    #endregion

    #region Helpers

    private static string GetConfigPath()
    {
        var configDir = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Aether"
        );
        Directory.CreateDirectory(configDir);
        return Path.Combine(configDir, "config.toml");
    }

    private static (int x, int y) GetCursorPosition()
    {
        NativeMethods.GetCursorPos(out var point);
        return (point.X, point.Y);
    }

    #endregion

    #region Lifecycle

    public void Quit()
    {
        // Cleanup services
        _hotkeyService?.Dispose();
        _trayIconService?.Dispose();
        _aetherCore?.Dispose();

        // Close all windows
        _haloWindow?.Close();
        _settingsWindow?.Close();
        _conversationWindow?.Close();

        // Exit application
        Exit();
    }

    private void OnUnhandledException(object sender, Microsoft.UI.Xaml.UnhandledExceptionEventArgs e)
    {
        System.Diagnostics.Debug.WriteLine($"Unhandled exception: {e.Exception}");
        e.Handled = true; // Prevent crash, but log the error
    }

    #endregion
}

/// <summary>
/// Native methods for cursor position, etc.
/// </summary>
internal static partial class NativeMethods
{
    [System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
    public struct POINT
    {
        public int X;
        public int Y;
    }

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    [return: System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.Bool)]
    public static extern bool GetCursorPos(out POINT lpPoint);
}
