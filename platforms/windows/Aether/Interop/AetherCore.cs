using System.Runtime.InteropServices;
using System.Runtime.CompilerServices;
using System.Text;
using Microsoft.UI.Dispatching;

namespace Aether.Interop;

/// <summary>
/// High-level wrapper for Aether Rust core.
///
/// Handles:
/// - UTF-8 string marshaling
/// - Callback registration and thread dispatching
/// - Error handling
///
/// CRITICAL: Callbacks from Rust may fire on any thread.
/// We use DispatcherQueue to marshal calls to the UI thread.
/// </summary>
public sealed class AetherCore : IDisposable
{
    private static AetherCore? _instance;
    private readonly DispatcherQueue _dispatcherQueue;
    private bool _initialized = false;
    private bool _disposed = false;

    #region Events

    /// <summary>Fired when processing state changes.</summary>
    public event Action<int>? StateChanged;

    /// <summary>Fired when streaming text is received.</summary>
    public event Action<string>? StreamReceived;

    /// <summary>Fired when an error occurs.</summary>
    public event Action<string, int>? ErrorOccurred;

    /// <summary>Fired for log messages (debug).</summary>
    public event Action<string>? LogMessage;

    #endregion

    public bool IsInitialized => _initialized;

    public AetherCore(DispatcherQueue dispatcherQueue)
    {
        _dispatcherQueue = dispatcherQueue;
        _instance = this;
    }

    /// <summary>
    /// Initialize the Rust core with config path.
    /// </summary>
    public unsafe bool Initialize(string? configPath = null)
    {
        if (_initialized)
        {
            Log("Already initialized");
            return true;
        }

        try
        {
            // Register callbacks BEFORE init
            RegisterCallbacks();

            // Call aether_init
            int result;
            if (string.IsNullOrEmpty(configPath))
            {
                result = NativeMethods.aether_init(null);
            }
            else
            {
                var pathBytes = Encoding.UTF8.GetBytes(configPath + '\0');
                fixed (byte* pathPtr = pathBytes)
                {
                    result = NativeMethods.aether_init(pathPtr);
                }
            }

            if (result == 0)
            {
                _initialized = true;
                Log("Initialization successful");
                return true;
            }
            else
            {
                Log($"Initialization failed with code: {result}");
                return false;
            }
        }
        catch (DllNotFoundException ex)
        {
            Log($"DLL not found: {ex.Message}");
            Log("Make sure aethecore.dll is in the application directory");
            return false;
        }
        catch (Exception ex)
        {
            Log($"Initialization error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Get version string from Rust core.
    /// </summary>
    public unsafe string? GetVersion()
    {
        try
        {
            byte* versionPtr = NativeMethods.aether_version();
            if (versionPtr == null) return null;
            return Marshal.PtrToStringUTF8((IntPtr)versionPtr);
        }
        catch (Exception ex)
        {
            Log($"GetVersion error: {ex.Message}");
            return null;
        }
    }

    private unsafe void RegisterCallbacks()
    {
        Log("Registering callbacks...");

        NativeMethods.aether_register_state_callback(&OnStateChangeCallback);
        NativeMethods.aether_register_stream_callback(&OnStreamCallback);
        NativeMethods.aether_register_error_callback(&OnErrorCallback);

        Log("Callbacks registered");
    }

    #region Static Callback Methods

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnStateChangeCallback(int state)
    {
        _instance?.DispatchStateChange(state);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnStreamCallback(byte* text)
    {
        if (text == null) return;
        var str = Marshal.PtrToStringUTF8((IntPtr)text);
        _instance?.DispatchStreamReceived(str);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnErrorCallback(byte* message, int code)
    {
        var msg = message != null
            ? Marshal.PtrToStringUTF8((IntPtr)message)
            : "Unknown error";
        _instance?.DispatchError(msg, code);
    }

    #endregion

    #region Dispatch to UI Thread

    private void DispatchStateChange(int state)
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            Log($"State changed: {state}");
            StateChanged?.Invoke(state);
        });
    }

    private void DispatchStreamReceived(string? text)
    {
        if (text == null) return;
        _dispatcherQueue.TryEnqueue(() =>
        {
            StreamReceived?.Invoke(text);
        });
    }

    private void DispatchError(string? message, int code)
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            Log($"Error: {message} (code: {code})");
            ErrorOccurred?.Invoke(message ?? "Unknown", code);
        });
    }

    #endregion

    private void Log(string message)
    {
        System.Diagnostics.Debug.WriteLine($"[AetherCore] {message}");
        _dispatcherQueue.TryEnqueue(() =>
        {
            LogMessage?.Invoke(message);
        });
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        if (_initialized)
        {
            try
            {
                NativeMethods.aether_free();
                Log("Resources freed");
            }
            catch (Exception ex)
            {
                Log($"Cleanup error: {ex.Message}");
            }
        }

        _instance = null;
    }
}

/// <summary>
/// P/Invoke declarations for aethecore.dll
/// </summary>
internal static unsafe partial class NativeMethods
{
    private const string DllName = "aethecore";

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern int aether_init(byte* configPath);

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern int aether_free();

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern byte* aether_version();

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern void aether_register_state_callback(
        delegate* unmanaged[Cdecl]<int, void> callback);

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern void aether_register_stream_callback(
        delegate* unmanaged[Cdecl]<byte*, void> callback);

    [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
    public static extern void aether_register_error_callback(
        delegate* unmanaged[Cdecl]<byte*, int, void> callback);
}
