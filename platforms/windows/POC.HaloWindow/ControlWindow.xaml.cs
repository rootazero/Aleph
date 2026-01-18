using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Windows.Graphics;

namespace POC.HaloWindow;

/// <summary>
/// Control window for testing Halo window behavior.
/// </summary>
public sealed partial class ControlWindow : Window
{
    public event Action<int, int>? ShowHaloRequested;
    public event Action? HideHaloRequested;

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out POINT lpPoint);

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT
    {
        public int X;
        public int Y;
    }

    public ControlWindow()
    {
        InitializeComponent();
        Title = "POC-1: Halo Window Test";

        // Set initial size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(500, 450));

        UpdateStatus("Ready. Click 'Show Halo' to test.");
    }

    private void ShowHaloButton_Click(object sender, RoutedEventArgs e)
    {
        if (GetCursorPos(out POINT point))
        {
            // Offset slightly so Halo appears near but not exactly at cursor
            ShowHaloRequested?.Invoke(point.X + 20, point.Y + 20);
            UpdateStatus($"Halo shown at ({point.X + 20}, {point.Y + 20})");
        }
        else
        {
            UpdateStatus("Failed to get cursor position");
        }
    }

    private void HideHaloButton_Click(object sender, RoutedEventArgs e)
    {
        HideHaloRequested?.Invoke();
        UpdateStatus("Halo hidden");
    }

    private void ShowAtCenterButton_Click(object sender, RoutedEventArgs e)
    {
        // Get primary screen dimensions
        var screenWidth = GetSystemMetrics(SM_CXSCREEN);
        var screenHeight = GetSystemMetrics(SM_CYSCREEN);

        var x = (screenWidth / 2) - 100; // Center horizontally
        var y = (screenHeight / 2) - 70; // Center vertically

        ShowHaloRequested?.Invoke(x, y);
        UpdateStatus($"Halo shown at screen center ({x}, {y})");
    }

    private void UpdateStatus(string message)
    {
        StatusText.Text = $"[{DateTime.Now:HH:mm:ss}] {message}";
    }

    private const int SM_CXSCREEN = 0;
    private const int SM_CYSCREEN = 1;

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int nIndex);
}
