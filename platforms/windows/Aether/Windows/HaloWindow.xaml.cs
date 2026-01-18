using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;
using WinRT.Interop;
using Aether.ViewModels;
using Aether.Models;

namespace Aether.Windows;

/// <summary>
/// Halo floating window - the core AI interaction overlay.
///
/// Key behaviors:
/// - Does not steal focus from other applications
/// - Transparent background with blur
/// - Always on top
/// - No taskbar entry
/// - Follows cursor position
/// - Displays AI response states
/// </summary>
public sealed partial class HaloWindow : Window
{
    #region Win32 Constants

    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_TOPMOST = 0x00000008;

    private const int GWL_STYLE = -16;
    private const int WS_CAPTION = 0x00C00000;
    private const int WS_SYSMENU = 0x00080000;

    private const uint SWP_NOMOVE = 0x0002;
    private const uint SWP_NOSIZE = 0x0001;
    private const uint SWP_NOACTIVATE = 0x0010;
    private const uint SWP_SHOWWINDOW = 0x0040;

    private static readonly IntPtr HWND_TOPMOST = new(-1);

    private const int SW_SHOWNOACTIVATE = 4;
    private const int SW_HIDE = 0;

    #endregion

    #region Win32 Imports

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter,
        int X, int Y, int cx, int cy, uint uFlags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    #endregion

    private readonly IntPtr _hwnd;
    private readonly AppWindow _appWindow;
    private HaloState _currentState = HaloState.Hidden;

    public bool IsVisible { get; private set; }

    public HaloWindow()
    {
        InitializeComponent();

        _hwnd = WindowNative.GetWindowHandle(this);
        var windowId = Win32Interop.GetWindowIdFromWindow(_hwnd);
        _appWindow = AppWindow.GetFromWindowId(windowId);

        ConfigureWindow();
    }

    private void ConfigureWindow()
    {
        // Remove title bar
        if (_appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
            presenter.SetBorderAndTitleBar(false, false);
        }

        // Set initial size
        _appWindow.Resize(new SizeInt32(220, 160));

        // Apply no-focus window styles
        int exStyle = GetWindowLong(_hwnd, GWL_EXSTYLE);
        exStyle |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        SetWindowLong(_hwnd, GWL_EXSTYLE, exStyle);

        // Remove caption
        int style = GetWindowLong(_hwnd, GWL_STYLE);
        style &= ~WS_CAPTION;
        style &= ~WS_SYSMENU;
        SetWindowLong(_hwnd, GWL_STYLE, style);

        // Apply topmost
        SetWindowPos(_hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    /// <summary>
    /// Show the Halo at specified screen coordinates.
    /// </summary>
    public void ShowAt(int x, int y)
    {
        _appWindow.Move(new PointInt32(x, y));
        ShowWindow(_hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(_hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);

        IsVisible = true;
        SetState(HaloState.Listening);
    }

    /// <summary>
    /// Hide the Halo window.
    /// </summary>
    public new void Hide()
    {
        ShowWindow(_hwnd, SW_HIDE);
        IsVisible = false;
        SetState(HaloState.Hidden);
    }

    /// <summary>
    /// Set the current Halo state.
    /// </summary>
    public void SetState(HaloState state)
    {
        if (_currentState == state) return;
        _currentState = state;

        // Update UI based on state
        switch (state)
        {
            case HaloState.Hidden:
                break;

            case HaloState.Listening:
                StatusText.Text = "Listening...";
                ThinkingRing.IsActive = false;
                StreamingScrollViewer.Visibility = Visibility.Collapsed;
                ErrorText.Visibility = Visibility.Collapsed;
                SetHaloColor("#FF6B4EFF", "#FF9B6BFF");
                break;

            case HaloState.Thinking:
                StatusText.Text = "Thinking...";
                ThinkingRing.IsActive = true;
                StreamingScrollViewer.Visibility = Visibility.Collapsed;
                SetHaloColor("#FF4ECAFF", "#FF6B9BFF");
                break;

            case HaloState.Processing:
                StatusText.Text = "Processing...";
                ThinkingRing.IsActive = true;
                break;

            case HaloState.Streaming:
                StatusText.Text = "";
                ThinkingRing.IsActive = false;
                StreamingScrollViewer.Visibility = Visibility.Visible;
                SetHaloColor("#FF4EFF6B", "#FF6BFFB9");
                break;

            case HaloState.Success:
                StatusText.Text = "Done";
                ThinkingRing.IsActive = false;
                SetHaloColor("#FF4EFF6B", "#FF6BFFB9");
                break;

            case HaloState.Error:
                StatusText.Text = "Error";
                ThinkingRing.IsActive = false;
                ErrorText.Visibility = Visibility.Visible;
                SetHaloColor("#FFFF4E4E", "#FFFF6B6B");
                break;
        }
    }

    /// <summary>
    /// Append streaming text.
    /// </summary>
    public void AppendStreamingText(string text)
    {
        StreamingText.Text += text;

        // Auto-scroll to bottom
        StreamingScrollViewer.ChangeView(null, StreamingScrollViewer.ScrollableHeight, null);
    }

    /// <summary>
    /// Clear streaming text.
    /// </summary>
    public void ClearStreamingText()
    {
        StreamingText.Text = "";
    }

    /// <summary>
    /// Set error message.
    /// </summary>
    public void SetError(string message)
    {
        ErrorText.Text = message;
        SetState(HaloState.Error);
    }

    private void SetHaloColor(string startColor, string endColor)
    {
        GradientStart.Color = ParseColor(startColor);
        GradientEnd.Color = ParseColor(endColor);
    }

    private static Windows.UI.Color ParseColor(string hex)
    {
        hex = hex.TrimStart('#');
        byte a = 255, r, g, b;

        if (hex.Length == 8)
        {
            a = Convert.ToByte(hex[..2], 16);
            r = Convert.ToByte(hex[2..4], 16);
            g = Convert.ToByte(hex[4..6], 16);
            b = Convert.ToByte(hex[6..8], 16);
        }
        else
        {
            r = Convert.ToByte(hex[..2], 16);
            g = Convert.ToByte(hex[2..4], 16);
            b = Convert.ToByte(hex[4..6], 16);
        }

        return Windows.UI.Color.FromArgb(a, r, g, b);
    }
}
