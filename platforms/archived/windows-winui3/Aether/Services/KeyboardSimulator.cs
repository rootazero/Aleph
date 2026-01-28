using System.Runtime.InteropServices;

namespace Aether.Services;

/// <summary>
/// Keyboard input simulation using Windows SendInput API.
///
/// Used for:
/// - Simulating Cut (Ctrl+X), Copy (Ctrl+C), Paste (Ctrl+V)
/// - Typing text character by character (typewriter effect)
/// - Sending key combinations
/// </summary>
public static class KeyboardSimulator
{
    #region Win32 Structures

    [StructLayout(LayoutKind.Sequential)]
    private struct INPUT
    {
        public uint type;
        public InputUnion U;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct InputUnion
    {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
        [FieldOffset(0)] public HARDWAREINPUT hi;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KEYBDINPUT
    {
        public ushort wVk;
        public ushort wScan;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MOUSEINPUT
    {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HARDWAREINPUT
    {
        public uint uMsg;
        public ushort wParamL;
        public ushort wParamH;
    }

    #endregion

    #region Win32 Constants

    private const uint INPUT_KEYBOARD = 1;
    private const uint KEYEVENTF_KEYDOWN = 0x0000;
    private const uint KEYEVENTF_KEYUP = 0x0002;
    private const uint KEYEVENTF_UNICODE = 0x0004;

    #endregion

    #region Win32 Imports

    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

    #endregion

    /// <summary>
    /// Send a single key press (down + up).
    /// </summary>
    public static void SendKey(VirtualKey key)
    {
        var inputs = new INPUT[2];

        // Key down
        inputs[0] = CreateKeyInput((ushort)key, false);

        // Key up
        inputs[1] = CreateKeyInput((ushort)key, true);

        SendInput(2, inputs, Marshal.SizeOf<INPUT>());
    }

    /// <summary>
    /// Send a key combination (e.g., Ctrl+C, Ctrl+Shift+V).
    /// </summary>
    public static void SendKeyCombo(params VirtualKey[] keys)
    {
        if (keys.Length == 0) return;

        var inputs = new INPUT[keys.Length * 2];
        int idx = 0;

        // Press all keys in order
        foreach (var key in keys)
        {
            inputs[idx++] = CreateKeyInput((ushort)key, false);
        }

        // Release all keys in reverse order
        for (int i = keys.Length - 1; i >= 0; i--)
        {
            inputs[idx++] = CreateKeyInput((ushort)keys[i], true);
        }

        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf<INPUT>());
    }

    /// <summary>
    /// Type text character by character using Unicode input.
    /// </summary>
    public static void TypeText(string text)
    {
        if (string.IsNullOrEmpty(text)) return;

        var inputs = new INPUT[text.Length * 2];
        int idx = 0;

        foreach (char c in text)
        {
            // Key down (Unicode)
            inputs[idx++] = CreateUnicodeInput(c, false);

            // Key up (Unicode)
            inputs[idx++] = CreateUnicodeInput(c, true);
        }

        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf<INPUT>());
    }

    /// <summary>
    /// Type text with delay between characters (typewriter effect).
    /// </summary>
    public static async Task TypeTextWithDelayAsync(string text, int delayMs = 20)
    {
        if (string.IsNullOrEmpty(text)) return;

        foreach (char c in text)
        {
            TypeChar(c);
            await Task.Delay(delayMs);
        }
    }

    /// <summary>
    /// Type a single character.
    /// </summary>
    public static void TypeChar(char c)
    {
        var inputs = new INPUT[2];
        inputs[0] = CreateUnicodeInput(c, false);
        inputs[1] = CreateUnicodeInput(c, true);
        SendInput(2, inputs, Marshal.SizeOf<INPUT>());
    }

    /// <summary>
    /// Press and hold a key.
    /// </summary>
    public static void KeyDown(VirtualKey key)
    {
        var input = CreateKeyInput((ushort)key, false);
        SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    /// <summary>
    /// Release a key.
    /// </summary>
    public static void KeyUp(VirtualKey key)
    {
        var input = CreateKeyInput((ushort)key, true);
        SendInput(1, new[] { input }, Marshal.SizeOf<INPUT>());
    }

    #region Helper Methods

    private static INPUT CreateKeyInput(ushort vkCode, bool keyUp)
    {
        return new INPUT
        {
            type = INPUT_KEYBOARD,
            U = new InputUnion
            {
                ki = new KEYBDINPUT
                {
                    wVk = vkCode,
                    wScan = 0,
                    dwFlags = keyUp ? KEYEVENTF_KEYUP : KEYEVENTF_KEYDOWN,
                    time = 0,
                    dwExtraInfo = IntPtr.Zero
                }
            }
        };
    }

    private static INPUT CreateUnicodeInput(char c, bool keyUp)
    {
        return new INPUT
        {
            type = INPUT_KEYBOARD,
            U = new InputUnion
            {
                ki = new KEYBDINPUT
                {
                    wVk = 0,
                    wScan = c,
                    dwFlags = KEYEVENTF_UNICODE | (keyUp ? KEYEVENTF_KEYUP : KEYEVENTF_KEYDOWN),
                    time = 0,
                    dwExtraInfo = IntPtr.Zero
                }
            }
        };
    }

    #endregion
}

/// <summary>
/// Extended virtual key codes.
/// </summary>
public enum VirtualKey : ushort
{
    // Modifier keys
    Shift = 0x10,
    Control = 0x11,
    Alt = 0x12,
    LWin = 0x5B,
    RWin = 0x5C,

    // Common keys
    Back = 0x08,
    Tab = 0x09,
    Return = 0x0D,
    Escape = 0x1B,
    Space = 0x20,

    // Arrow keys
    Left = 0x25,
    Up = 0x26,
    Right = 0x27,
    Down = 0x28,

    // Edit keys
    Insert = 0x2D,
    Delete = 0x2E,
    Home = 0x24,
    End = 0x23,
    PageUp = 0x21,
    PageDown = 0x22,

    // Letter keys (A-Z = 0x41-0x5A)
    A = 0x41, B = 0x42, C = 0x43, D = 0x44, E = 0x45,
    F = 0x46, G = 0x47, H = 0x48, I = 0x49, J = 0x4A,
    K = 0x4B, L = 0x4C, M = 0x4D, N = 0x4E, O = 0x4F,
    P = 0x50, Q = 0x51, R = 0x52, S = 0x53, T = 0x54,
    U = 0x55, V = 0x56, W = 0x57, X = 0x58, Y = 0x59, Z = 0x5A,

    // Number keys (0-9 = 0x30-0x39)
    D0 = 0x30, D1 = 0x31, D2 = 0x32, D3 = 0x33, D4 = 0x34,
    D5 = 0x35, D6 = 0x36, D7 = 0x37, D8 = 0x38, D9 = 0x39,

    // Function keys
    F1 = 0x70, F2 = 0x71, F3 = 0x72, F4 = 0x73,
    F5 = 0x74, F6 = 0x75, F7 = 0x76, F8 = 0x77,
    F9 = 0x78, F10 = 0x79, F11 = 0x7A, F12 = 0x7B,
}
