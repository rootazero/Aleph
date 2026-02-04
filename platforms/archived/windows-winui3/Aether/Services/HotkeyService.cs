using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Aleph.Services;

/// <summary>
/// Global hotkey service using low-level keyboard hook.
///
/// Detects:
/// - Double-tap Shift: Triggers Halo
/// - Win + Alt + /: Triggers Conversation window
///
/// Works regardless of which application has focus.
/// </summary>
public sealed class HotkeyService : IDisposable
{
    #region Win32 Constants

    private const int WH_KEYBOARD_LL = 13;
    private const int WM_KEYDOWN = 0x0100;
    private const int WM_KEYUP = 0x0101;
    private const int WM_SYSKEYDOWN = 0x0104;
    private const int WM_SYSKEYUP = 0x0105;

    // Virtual key codes
    private const int VK_SHIFT = 0x10;
    private const int VK_LSHIFT = 0xA0;
    private const int VK_RSHIFT = 0xA1;
    private const int VK_LWIN = 0x5B;
    private const int VK_RWIN = 0x5C;
    private const int VK_MENU = 0x12;    // Alt
    private const int VK_LMENU = 0xA4;   // Left Alt
    private const int VK_RMENU = 0xA5;   // Right Alt
    private const int VK_OEM_2 = 0xBF;   // / key
    private const int VK_ESCAPE = 0x1B;

    #endregion

    #region Win32 Imports

    private delegate IntPtr LowLevelKeyboardProc(int nCode, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr SetWindowsHookEx(int idHook, LowLevelKeyboardProc lpfn,
        IntPtr hMod, uint dwThreadId);

    [DllImport("user32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnhookWindowsHookEx(IntPtr hhk);

    [DllImport("user32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr CallNextHookEx(IntPtr hhk, int nCode,
        IntPtr wParam, IntPtr lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr GetModuleHandle(string lpModuleName);

    [DllImport("user32.dll")]
    private static extern short GetAsyncKeyState(int vKey);

    [StructLayout(LayoutKind.Sequential)]
    private struct KBDLLHOOKSTRUCT
    {
        public uint vkCode;
        public uint scanCode;
        public uint flags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    #endregion

    #region Events

    /// <summary>
    /// Fired when double-tap Shift is detected.
    /// </summary>
    public event Action? OnHaloHotkeyPressed;

    /// <summary>
    /// Fired when Win + Alt + / is pressed.
    /// </summary>
    public event Action? OnConversationHotkeyPressed;

    /// <summary>
    /// Fired when Escape is pressed.
    /// </summary>
    public event Action? OnEscapePressed;

    /// <summary>
    /// Fired for any key event (for debugging).
    /// </summary>
    public event Action<string>? OnKeyEvent;

    #endregion

    private IntPtr _hookId = IntPtr.Zero;
    private readonly LowLevelKeyboardProc _proc;

    // Double-tap Shift detection
    private DateTime _lastShiftUp = DateTime.MinValue;
    private const int DoubleClickThresholdMs = 400;

    // Track if shift is currently held
    private bool _shiftHeld = false;

    public HotkeyService()
    {
        _proc = HookCallback;
        _hookId = SetHook(_proc);

        if (_hookId == IntPtr.Zero)
        {
            var error = Marshal.GetLastWin32Error();
            throw new InvalidOperationException(
                $"Failed to install keyboard hook. Error code: {error}");
        }
    }

    private IntPtr SetHook(LowLevelKeyboardProc proc)
    {
        using var curProcess = Process.GetCurrentProcess();
        using var curModule = curProcess.MainModule!;
        return SetWindowsHookEx(WH_KEYBOARD_LL, proc,
            GetModuleHandle(curModule.ModuleName), 0);
    }

    private IntPtr HookCallback(int nCode, IntPtr wParam, IntPtr lParam)
    {
        if (nCode >= 0)
        {
            var hookStruct = Marshal.PtrToStructure<KBDLLHOOKSTRUCT>(lParam);
            int vkCode = (int)hookStruct.vkCode;
            bool isKeyDown = wParam == WM_KEYDOWN || wParam == WM_SYSKEYDOWN;
            bool isKeyUp = wParam == WM_KEYUP || wParam == WM_SYSKEYUP;

            // Debug logging
            OnKeyEvent?.Invoke($"VK: 0x{vkCode:X2}, Down: {isKeyDown}, Up: {isKeyUp}");

            // Handle Shift key for double-tap detection
            if (IsShiftKey(vkCode))
            {
                if (isKeyDown && !_shiftHeld)
                {
                    _shiftHeld = true;
                }
                else if (isKeyUp && _shiftHeld)
                {
                    _shiftHeld = false;
                    HandleShiftUp();
                }
            }

            // Handle Win + Alt + / for conversation window
            if (vkCode == VK_OEM_2 && isKeyDown)
            {
                if (IsWinKeyDown() && IsAltKeyDown())
                {
                    OnConversationHotkeyPressed?.Invoke();
                    OnKeyEvent?.Invoke(">>> Win + Alt + / detected!");
                }
            }

            // Handle Escape
            if (vkCode == VK_ESCAPE && isKeyDown)
            {
                OnEscapePressed?.Invoke();
            }
        }

        return CallNextHookEx(_hookId, nCode, wParam, lParam);
    }

    private static bool IsShiftKey(int vkCode)
    {
        return vkCode == VK_SHIFT || vkCode == VK_LSHIFT || vkCode == VK_RSHIFT;
    }

    private void HandleShiftUp()
    {
        var now = DateTime.Now;
        var elapsed = (now - _lastShiftUp).TotalMilliseconds;

        if (elapsed < DoubleClickThresholdMs && elapsed > 50) // 50ms debounce
        {
            // Double-tap detected!
            OnHaloHotkeyPressed?.Invoke();
            OnKeyEvent?.Invoke($">>> Double-tap Shift detected! (elapsed: {elapsed:F0}ms)");
            _lastShiftUp = DateTime.MinValue; // Reset to prevent triple-tap
        }
        else
        {
            _lastShiftUp = now;
        }
    }

    private static bool IsKeyDown(int vKey)
    {
        return (GetAsyncKeyState(vKey) & 0x8000) != 0;
    }

    private static bool IsWinKeyDown()
    {
        return IsKeyDown(VK_LWIN) || IsKeyDown(VK_RWIN);
    }

    private static bool IsAltKeyDown()
    {
        return IsKeyDown(VK_MENU) || IsKeyDown(VK_LMENU) || IsKeyDown(VK_RMENU);
    }

    public void Dispose()
    {
        if (_hookId != IntPtr.Zero)
        {
            UnhookWindowsHookEx(_hookId);
            _hookId = IntPtr.Zero;
        }
    }
}
