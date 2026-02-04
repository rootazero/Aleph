using System.Runtime.InteropServices;

namespace Aleph.Services;

/// <summary>
/// Cursor and caret position tracking service.
///
/// Features:
/// - Get current mouse cursor position
/// - Get text caret (insertion point) position from focused window
/// - Monitor for cursor position changes
///
/// Used for:
/// - Positioning Halo window at cursor location
/// - Positioning Halo near text caret in text editors
/// </summary>
public sealed class CursorService : IDisposable
{
    #region Win32 Structures

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT
    {
        public int X;
        public int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct GUITHREADINFO
    {
        public int cbSize;
        public int flags;
        public IntPtr hwndActive;
        public IntPtr hwndFocus;
        public IntPtr hwndCapture;
        public IntPtr hwndMenuOwner;
        public IntPtr hwndMoveSize;
        public IntPtr hwndCaret;
        public RECT rcCaret;
    }

    #endregion

    #region Win32 Imports

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out POINT lpPoint);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO lpgui);

    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ClientToScreen(IntPtr hWnd, ref POINT lpPoint);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr MonitorFromPoint(POINT pt, uint dwFlags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFO lpmi);

    [StructLayout(LayoutKind.Sequential)]
    private struct MONITORINFO
    {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    private const uint MONITOR_DEFAULTTONEAREST = 2;

    #endregion

    private bool _disposed;

    /// <summary>
    /// Get the current mouse cursor position in screen coordinates.
    /// </summary>
    public (int X, int Y) GetCursorPosition()
    {
        if (GetCursorPos(out POINT point))
        {
            return (point.X, point.Y);
        }
        return (0, 0);
    }

    /// <summary>
    /// Get the text caret (insertion point) position in screen coordinates.
    /// Returns null if no caret is available or cannot be determined.
    /// </summary>
    public (int X, int Y)? GetCaretPosition()
    {
        try
        {
            // Get the foreground window
            var hwndForeground = GetForegroundWindow();
            if (hwndForeground == IntPtr.Zero)
                return null;

            // Get the thread ID of the foreground window
            var threadId = GetWindowThreadProcessId(hwndForeground, out _);

            // Get GUI thread info
            var guiInfo = new GUITHREADINFO
            {
                cbSize = Marshal.SizeOf<GUITHREADINFO>()
            };

            if (!GetGUIThreadInfo(threadId, ref guiInfo))
                return null;

            // Check if there's a caret
            if (guiInfo.hwndCaret == IntPtr.Zero)
                return null;

            // Convert caret position to screen coordinates
            var caretPoint = new POINT
            {
                X = guiInfo.rcCaret.Left,
                Y = guiInfo.rcCaret.Bottom // Use bottom for below-caret positioning
            };

            if (ClientToScreen(guiInfo.hwndCaret, ref caretPoint))
            {
                return (caretPoint.X, caretPoint.Y);
            }

            return null;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// Get the best position for showing the Halo window.
    /// Prefers caret position, falls back to cursor position.
    /// </summary>
    /// <param name="windowWidth">Width of the Halo window</param>
    /// <param name="windowHeight">Height of the Halo window</param>
    /// <returns>Screen coordinates for the Halo window</returns>
    public (int X, int Y) GetHaloPosition(int windowWidth = 280, int windowHeight = 180)
    {
        // Try caret position first
        var caretPos = GetCaretPosition();
        if (caretPos.HasValue)
        {
            return AdjustForScreenBounds(caretPos.Value.X, caretPos.Value.Y + 8, windowWidth, windowHeight);
        }

        // Fall back to cursor position
        var cursorPos = GetCursorPosition();
        return AdjustForScreenBounds(cursorPos.X + 16, cursorPos.Y + 16, windowWidth, windowHeight);
    }

    /// <summary>
    /// Adjust position to ensure window stays within screen bounds.
    /// </summary>
    private (int X, int Y) AdjustForScreenBounds(int x, int y, int width, int height)
    {
        var point = new POINT { X = x, Y = y };
        var monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);

        if (monitor == IntPtr.Zero)
            return (x, y);

        var monitorInfo = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        if (!GetMonitorInfo(monitor, ref monitorInfo))
            return (x, y);

        var workArea = monitorInfo.rcWork;

        // Adjust X if window would go off right edge
        if (x + width > workArea.Right)
        {
            x = workArea.Right - width - 8;
        }

        // Adjust X if window would go off left edge
        if (x < workArea.Left)
        {
            x = workArea.Left + 8;
        }

        // Adjust Y if window would go off bottom edge
        if (y + height > workArea.Bottom)
        {
            y = workArea.Bottom - height - 8;
        }

        // Adjust Y if window would go off top edge
        if (y < workArea.Top)
        {
            y = workArea.Top + 8;
        }

        return (x, y);
    }

    /// <summary>
    /// Get information about the monitor at the current cursor position.
    /// </summary>
    public MonitorInfo GetCurrentMonitor()
    {
        var pos = GetCursorPosition();
        var point = new POINT { X = pos.X, Y = pos.Y };
        var monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);

        if (monitor == IntPtr.Zero)
        {
            return new MonitorInfo { WorkArea = new System.Drawing.Rectangle(0, 0, 1920, 1080) };
        }

        var monitorInfo = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        if (!GetMonitorInfo(monitor, ref monitorInfo))
        {
            return new MonitorInfo { WorkArea = new System.Drawing.Rectangle(0, 0, 1920, 1080) };
        }

        return new MonitorInfo
        {
            WorkArea = new System.Drawing.Rectangle(
                monitorInfo.rcWork.Left,
                monitorInfo.rcWork.Top,
                monitorInfo.rcWork.Right - monitorInfo.rcWork.Left,
                monitorInfo.rcWork.Bottom - monitorInfo.rcWork.Top
            ),
            FullArea = new System.Drawing.Rectangle(
                monitorInfo.rcMonitor.Left,
                monitorInfo.rcMonitor.Top,
                monitorInfo.rcMonitor.Right - monitorInfo.rcMonitor.Left,
                monitorInfo.rcMonitor.Bottom - monitorInfo.rcMonitor.Top
            )
        };
    }

    /// <summary>
    /// Get the handle of the currently focused window.
    /// </summary>
    public IntPtr GetFocusedWindowHandle()
    {
        var hwndForeground = GetForegroundWindow();
        if (hwndForeground == IntPtr.Zero)
            return IntPtr.Zero;

        var threadId = GetWindowThreadProcessId(hwndForeground, out _);
        var guiInfo = new GUITHREADINFO
        {
            cbSize = Marshal.SizeOf<GUITHREADINFO>()
        };

        if (GetGUIThreadInfo(threadId, ref guiInfo) && guiInfo.hwndFocus != IntPtr.Zero)
        {
            return guiInfo.hwndFocus;
        }

        return hwndForeground;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        GC.SuppressFinalize(this);
    }
}

/// <summary>
/// Monitor information.
/// </summary>
public struct MonitorInfo
{
    /// <summary>
    /// Work area (excluding taskbar).
    /// </summary>
    public System.Drawing.Rectangle WorkArea { get; set; }

    /// <summary>
    /// Full monitor area.
    /// </summary>
    public System.Drawing.Rectangle FullArea { get; set; }
}
