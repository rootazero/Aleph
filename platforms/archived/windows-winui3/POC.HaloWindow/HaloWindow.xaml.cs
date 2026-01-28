using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Windows.Graphics;
using WinRT.Interop;

namespace POC.HaloWindow;

/// <summary>
/// Halo floating window - POC for no-focus transparent overlay
///
/// Key requirements:
/// 1. Does not steal focus from other applications
/// 2. Transparent background
/// 3. Always on top
/// 4. No taskbar entry
/// 5. Can be positioned at cursor location
/// </summary>
public sealed partial class HaloWindow : Window
{
    #region Win32 Constants

    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_TOPMOST = 0x00000008;
    private const int WS_EX_LAYERED = 0x00080000;
    private const int WS_EX_TRANSPARENT = 0x00000020;

    private const int GWL_STYLE = -16;
    private const int WS_CAPTION = 0x00C00000;
    private const int WS_SYSMENU = 0x00080000;

    private const uint SWP_NOMOVE = 0x0002;
    private const uint SWP_NOSIZE = 0x0001;
    private const uint SWP_NOACTIVATE = 0x0010;
    private const uint SWP_SHOWWINDOW = 0x0040;

    private static readonly IntPtr HWND_TOPMOST = new(-1);

    #endregion

    #region Win32 Imports

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(
        IntPtr hWnd,
        IntPtr hWndInsertAfter,
        int X, int Y, int cx, int cy,
        uint uFlags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    private const int SW_SHOWNOACTIVATE = 4;
    private const int SW_HIDE = 0;

    #endregion

    private readonly IntPtr _hwnd;
    private readonly AppWindow _appWindow;

    public HaloWindow()
    {
        InitializeComponent();

        // Get native window handle
        _hwnd = WindowNative.GetWindowHandle(this);
        var windowId = Win32Interop.GetWindowIdFromWindow(_hwnd);
        _appWindow = AppWindow.GetFromWindowId(windowId);

        ConfigureWindow();
    }

    private void ConfigureWindow()
    {
        // 1. Remove title bar
        if (_appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
            presenter.SetBorderAndTitleBar(false, false);
        }

        // 2. Set window size
        _appWindow.Resize(new SizeInt32(200, 140));

        // 3. Apply extended window styles for no-focus behavior
        int exStyle = GetWindowLong(_hwnd, GWL_EXSTYLE);

        // WS_EX_NOACTIVATE: Prevents window from being activated
        // WS_EX_TOOLWINDOW: Hides from taskbar
        // WS_EX_TOPMOST: Always on top
        exStyle |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;

        SetWindowLong(_hwnd, GWL_EXSTYLE, exStyle);

        // 4. Remove WS_CAPTION and WS_SYSMENU for borderless window
        int style = GetWindowLong(_hwnd, GWL_STYLE);
        style &= ~WS_CAPTION;
        style &= ~WS_SYSMENU;
        SetWindowLong(_hwnd, GWL_STYLE, style);

        // 5. Ensure topmost is applied
        SetWindowPos(_hwnd, HWND_TOPMOST, 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);

        // Note: Transparent background is set via XAML
        // The window itself needs to support transparency via composition
    }

    /// <summary>
    /// Show the Halo window at specified screen coordinates.
    /// CRITICAL: Uses ShowWindow with SW_SHOWNOACTIVATE to prevent focus stealing.
    /// </summary>
    public void ShowAt(int x, int y)
    {
        // Move window to position
        _appWindow.Move(new PointInt32(x, y));

        // Show without activating - this is the key to not stealing focus!
        ShowWindow(_hwnd, SW_SHOWNOACTIVATE);

        // Ensure it stays topmost
        SetWindowPos(_hwnd, HWND_TOPMOST, x, y, 0, 0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);

        UpdateStatus("Showing at ({0}, {1})", x, y);
    }

    /// <summary>
    /// Hide the Halo window.
    /// </summary>
    public void Hide()
    {
        ShowWindow(_hwnd, SW_HIDE);
    }

    /// <summary>
    /// Update the status text (for POC testing).
    /// </summary>
    public void UpdateStatus(string format, params object[] args)
    {
        StatusText.Text = string.Format(format, args);
    }
}
