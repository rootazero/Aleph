// Aether Windows Application
// Main entry point - manages lifecycle, services, and windows

using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Aether.Services;
using Aether.Interop;
using Aether.Windows;
using Aether.Models;

namespace Aether;

/// <summary>
/// Aether Windows application entry point.
///
/// Responsibilities:
/// - Single instance enforcement
/// - Service initialization (hotkey, tray, core, cursor, clipboard, screen capture, auto-update)
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
    private CursorService? _cursorService;
    private ClipboardService? _clipboardService;
    private ScreenCaptureService? _screenCaptureService;
    private AutoUpdateService? _autoUpdateService;

    public TrayIconService TrayIcon => _trayIconService
        ?? throw new InvalidOperationException("TrayIconService not initialized");

    public HotkeyService Hotkeys => _hotkeyService
        ?? throw new InvalidOperationException("HotkeyService not initialized");

    public AetherCore Core => _aetherCore
        ?? throw new InvalidOperationException("AetherCore not initialized");

    public CursorService Cursor => _cursorService
        ?? throw new InvalidOperationException("CursorService not initialized");

    public ClipboardService Clipboard => _clipboardService
        ?? throw new InvalidOperationException("ClipboardService not initialized");

    public ScreenCaptureService ScreenCapture => _screenCaptureService
        ?? throw new InvalidOperationException("ScreenCaptureService not initialized");

    public AutoUpdateService AutoUpdate => _autoUpdateService
        ?? throw new InvalidOperationException("AutoUpdateService not initialized");

    #endregion

    #region Windows

    private HaloWindow? _haloWindow;
    private SettingsWindow? _settingsWindow;
    private ConversationWindow? _conversationWindow;

    public HaloWindow? HaloWindow => _haloWindow;

    #endregion

    public App()
    {
        _instance = this;

        // Set application language based on system preference
        InitializeLanguage();

        InitializeComponent();
        UnhandledException += OnUnhandledException;
    }

    private static void InitializeLanguage()
    {
        try
        {
            // Get system language from user preferences
            var systemLanguages = global::Windows.System.UserProfile.GlobalizationPreferences.Languages;
            if (systemLanguages.Count > 0)
            {
                var primaryLanguage = systemLanguages[0];
                // Set application language to match system
                global::Windows.Globalization.ApplicationLanguages.PrimaryLanguageOverride = primaryLanguage;
                System.Diagnostics.Debug.WriteLine($"[App] Language set to: {primaryLanguage}");
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[App] Language detection failed: {ex.Message}");
        }
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

        // Wire up core callbacks
        WireUpCoreCallbacks();

        // Show tray icon
        _trayIconService?.Show();

        // Check for updates on startup (background)
        _ = CheckForUpdatesAsync();

        // Cleanup old update files
        _autoUpdateService?.CleanupOldUpdates();

        System.Diagnostics.Debug.WriteLine("Aether started successfully");
    }

    private void InitializeServices()
    {
        try
        {
            // 1. Initialize cursor service (needed for Halo positioning)
            _cursorService = new CursorService();

            // 2. Initialize clipboard service
            _clipboardService = new ClipboardService();

            // 3. Initialize screen capture service
            _screenCaptureService = new ScreenCaptureService();

            // 4. Initialize Rust core
            _aetherCore = new AetherCore(_dispatcherQueue!);
            var configPath = GetConfigPath();
            if (!_aetherCore.Initialize(configPath))
            {
                System.Diagnostics.Debug.WriteLine("Warning: Aether core initialization failed (DLL may be missing)");
            }

            // 5. Initialize hotkey service
            _hotkeyService = new HotkeyService();

            // 6. Initialize tray icon service
            _trayIconService = new TrayIconService();
            _trayIconService.SettingsRequested += ShowSettings;
            _trayIconService.QuitRequested += Quit;

            // 7. Initialize auto-update service
            _autoUpdateService = new AutoUpdateService();
            _autoUpdateService.UpdateAvailable += OnUpdateAvailable;
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

    private void WireUpCoreCallbacks()
    {
        if (_aetherCore == null || _haloWindow == null) return;

        // Stream text callback
        _aetherCore.StreamReceived += (text) =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                if (_haloWindow.ViewModel.State != HaloState.Streaming &&
                    _haloWindow.ViewModel.State != HaloState.MultiTurnStreaming)
                {
                    _haloWindow.SetState(HaloState.Streaming);
                }
                _haloWindow.AppendStreamingText(text);
            });
        };

        // Stream complete callback
        _aetherCore.Completed += (response) =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                _haloWindow.SetState(HaloState.Success);
            });
        };

        // Error callback
        _aetherCore.ErrorOccurred += (error, code) =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                _haloWindow.SetError(error);
            });
        };

        // Tool execution callback (status: 0=started, 1=completed, 2=failed)
        _aetherCore.ToolExecuted += (toolName, status, result) =>
        {
            _dispatcherQueue?.TryEnqueue(() =>
            {
                switch (status)
                {
                    case 0: // Started
                        _haloWindow.StartToolExecution(toolName);
                        break;
                    case 1: // Completed
                        _haloWindow.CompleteToolExecution(result);
                        break;
                    case 2: // Failed
                        _haloWindow.FailToolExecution(result);
                        break;
                }
            });
        };
    }

    #region Window Management

    public void ShowHaloAtCursor()
    {
        if (_haloWindow == null || _cursorService == null) return;

        var (x, y) = _cursorService.GetHaloPosition(280, 180);
        _haloWindow.ShowAt(x, y);
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

    #region AI Interaction

    /// <summary>
    /// Send user input to AI and display response in Halo.
    /// </summary>
    public Task ProcessUserInputAsync(string input)
    {
        if (_haloWindow == null || _aetherCore == null) return Task.CompletedTask;

        // Show thinking state
        _haloWindow.SetState(HaloState.Thinking);
        _haloWindow.ClearStreamingText();

        try
        {
            // Send to Rust core (which will trigger callbacks)
            _aetherCore.Process(input, stream: true);
        }
        catch (Exception ex)
        {
            _haloWindow.SetError(ex.Message);
        }

        return Task.CompletedTask;
    }

    /// <summary>
    /// Process selected text (transmutation flow).
    /// </summary>
    public async Task ProcessSelectedTextAsync()
    {
        if (_clipboardService == null) return;

        // Simulate copy (Ctrl+C) to get selected text
        var content = await _clipboardService.SimulateCopyAsync();
        if (content?.HasText == true && !string.IsNullOrWhiteSpace(content.Text))
        {
            ShowHaloAtCursor();
            await ProcessUserInputAsync(content.Text);
        }
    }

    /// <summary>
    /// Capture screen and send to AI for vision analysis.
    /// </summary>
    public async Task ProcessScreenCaptureAsync()
    {
        if (_screenCaptureService == null || _aetherCore == null || _haloWindow == null) return;

        _haloWindow.SetState(HaloState.Processing);

        var imageBytes = await _screenCaptureService.CaptureActiveWindowAsync();
        if (imageBytes != null)
        {
            // Resize for efficiency
            var resized = _screenCaptureService.ResizeImage(imageBytes, 1280, 720);
            if (resized != null)
            {
                // TODO: Send to vision API via Rust core
                // await _aetherCore.SendImageAsync(resized, "What's on this screen?");
            }
        }
    }

    #endregion

    #region Auto Update

    private async Task CheckForUpdatesAsync()
    {
        if (_autoUpdateService == null) return;

        // Wait a bit before checking (don't slow down startup)
        await Task.Delay(5000);

        var update = await _autoUpdateService.CheckForUpdatesAsync();
        if (update != null)
        {
            System.Diagnostics.Debug.WriteLine($"[AutoUpdate] New version available: {update.Version}");
        }
    }

    private void OnUpdateAvailable(UpdateInfo update)
    {
        // TODO: Show update notification to user
        System.Diagnostics.Debug.WriteLine($"[AutoUpdate] Update available: {update.Version} (current: {update.CurrentVersion})");
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

    #endregion

    #region Lifecycle

    public void Quit()
    {
        // Cleanup services
        _hotkeyService?.Dispose();
        _trayIconService?.Dispose();
        _aetherCore?.Dispose();
        _cursorService?.Dispose();
        _screenCaptureService?.Dispose();
        _autoUpdateService?.Dispose();

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
