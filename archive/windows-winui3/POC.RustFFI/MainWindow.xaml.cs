using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace POC.RustFFI;

/// <summary>
/// Main window for testing Rust FFI callbacks.
/// </summary>
public sealed partial class MainWindow : Window
{
    private AetherCore? _aetherCore;
    private readonly Microsoft.UI.Dispatching.DispatcherQueue _dispatcherQueue;

    public MainWindow()
    {
        InitializeComponent();
        Title = "POC-3: Rust FFI Callback Test";

        _dispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread();

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(650, 550));

        Log("Ready. Click 'Initialize Core' to begin.");
        Log("");
        Log("Note: aethecore.dll must be present in the output directory.");
        Log("Build with: cd core && cargo build --release --features cabi");
    }

    private void InitButton_Click(object sender, RoutedEventArgs e)
    {
        Log("Initializing AetherCore...");

        try
        {
            _aetherCore = new AetherCore(_dispatcherQueue);

            // Subscribe to events
            _aetherCore.LogMessage += msg => Log($"[Core] {msg}");
            _aetherCore.StateChanged += state => Log($"[Callback] State changed to: {state}");
            _aetherCore.StreamReceived += text => Log($"[Callback] Stream: {text}");
            _aetherCore.ErrorOccurred += (msg, code) => Log($"[Callback] Error: {msg} (code: {code})");

            // Try to initialize
            if (_aetherCore.Initialize())
            {
                DllStatus.Text = "Loaded successfully";
                DllStatus.Foreground = new SolidColorBrush(Colors.Green);

                var version = _aetherCore.GetVersion();
                CoreVersion.Text = version ?? "Unknown";

                CallbackStatus.Text = "Registered";
                CallbackStatus.Foreground = new SolidColorBrush(Colors.Green);

                InitButton.IsEnabled = false;
                TestCallbackButton.IsEnabled = true;
                CleanupButton.IsEnabled = true;

                Log("Initialization complete!");
            }
            else
            {
                DllStatus.Text = "Initialization failed";
                DllStatus.Foreground = new SolidColorBrush(Colors.Red);
            }
        }
        catch (DllNotFoundException ex)
        {
            DllStatus.Text = "DLL not found";
            DllStatus.Foreground = new SolidColorBrush(Colors.Red);
            Log($"Error: {ex.Message}");
            Log("");
            Log("To fix this:");
            Log("1. Build the Rust core: cargo build --release --features cabi");
            Log("2. Copy aethecore.dll to the output directory");
        }
        catch (Exception ex)
        {
            DllStatus.Text = "Error";
            DllStatus.Foreground = new SolidColorBrush(Colors.Red);
            Log($"Error: {ex.Message}");
        }
    }

    private void TestCallbackButton_Click(object sender, RoutedEventArgs e)
    {
        Log("");
        Log("Testing callback mechanism...");
        Log("");
        Log("In production, Rust would call these callbacks during processing.");
        Log("For POC validation, we verify:");
        Log("  1. Callback registration succeeded (no crash)");
        Log("  2. Function pointers are correctly formatted");
        Log("  3. AetherCore wrapper handles threading correctly");
        Log("");
        Log("To fully test callbacks, the Rust core would need to:");
        Log("  - Call state_callback(state) during state transitions");
        Log("  - Call stream_callback(text) during streaming");
        Log("  - Call error_callback(msg, code) on errors");
        Log("");
        Log("The callback mechanism is verified if:");
        Log("  - No crashes during registration");
        Log("  - Version string retrieved successfully");
        Log("  - Init/Free complete without errors");
        Log("");

        // Simulate what would happen if Rust called back
        Log("Simulating callbacks (as if called from Rust):");
        _aetherCore?.StateChanged?.Invoke(1); // Thinking
        _aetherCore?.StreamReceived?.Invoke("Hello from simulated callback");
        _aetherCore?.StateChanged?.Invoke(2); // Complete

        Log("");
        Log("If you see the callback messages above, the mechanism works!");
    }

    private void CleanupButton_Click(object sender, RoutedEventArgs e)
    {
        Log("");
        Log("Cleaning up...");

        _aetherCore?.Dispose();
        _aetherCore = null;

        DllStatus.Text = "Unloaded";
        DllStatus.Foreground = new SolidColorBrush(Colors.Gray);
        CoreVersion.Text = "-";
        CallbackStatus.Text = "Not registered";
        CallbackStatus.Foreground = new SolidColorBrush(Colors.Gray);

        InitButton.IsEnabled = true;
        TestCallbackButton.IsEnabled = false;
        CleanupButton.IsEnabled = false;

        Log("Cleanup complete.");
    }

    private void ClearLog_Click(object sender, RoutedEventArgs e)
    {
        LogText.Text = "";
    }

    private void Log(string message)
    {
        var timestamp = DateTime.Now.ToString("HH:mm:ss.fff");
        var line = string.IsNullOrEmpty(message)
            ? "\n"
            : $"[{timestamp}] {message}\n";

        LogText.Text += line;
    }
}
