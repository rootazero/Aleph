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

    /// <summary>Fired when processing completes.</summary>
    public event Action<string>? Completed;

    /// <summary>Fired when an error occurs.</summary>
    public event Action<string, int>? ErrorOccurred;

    /// <summary>Fired when a tool is executed (name, status, result).</summary>
    public event Action<string, int, string>? ToolExecuted;

    /// <summary>Fired when memory is stored.</summary>
    public event Action? MemoryStored;

    /// <summary>Fired for log messages (debug).</summary>
    public event Action<string>? LogMessage;

    #endregion

    #region Properties

    /// <summary>Gets whether the core is initialized.</summary>
    public bool IsInitialized => _initialized;

    /// <summary>Gets the version of the Aether core library.</summary>
    public string? Version => GetVersion();

    /// <summary>Gets whether the current operation is cancelled.</summary>
    public bool IsCancelled => NativeMethods.aether_is_cancelled() != 0;

    #endregion

    public AetherCore(DispatcherQueue dispatcherQueue)
    {
        _dispatcherQueue = dispatcherQueue;
        _instance = this;
    }

    #region Initialization

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
                // Use default config path
                var defaultPath = GetDefaultConfigPath();
                var pathBytes = Encoding.UTF8.GetBytes(defaultPath + '\0');
                fixed (byte* pathPtr = pathBytes)
                {
                    result = NativeMethods.aether_init(pathPtr);
                }
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
                var errorMsg = GetErrorMessage(result);
                Log($"Initialization failed: {errorMsg}");
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

    private static string GetDefaultConfigPath()
    {
        var appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        return Path.Combine(appData, "Aether", "config.toml");
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

    #endregion

    #region Processing

    /// <summary>
    /// Process user input.
    /// </summary>
    /// <param name="input">User input text.</param>
    /// <param name="appContext">Optional application context (process name).</param>
    /// <param name="windowTitle">Optional active window title.</param>
    /// <param name="topicId">Optional topic ID for multi-turn conversation.</param>
    /// <param name="stream">Whether to stream the response.</param>
    /// <returns>True if processing started successfully.</returns>
    public unsafe bool Process(string input, string? appContext = null, string? windowTitle = null, string? topicId = null, bool stream = true)
    {
        if (!_initialized)
        {
            Log("Cannot process: not initialized");
            return false;
        }

        try
        {
            var inputBytes = Encoding.UTF8.GetBytes(input + '\0');
            byte[]? contextBytes = appContext != null ? Encoding.UTF8.GetBytes(appContext + '\0') : null;
            byte[]? titleBytes = windowTitle != null ? Encoding.UTF8.GetBytes(windowTitle + '\0') : null;
            byte[]? topicBytes = topicId != null ? Encoding.UTF8.GetBytes(topicId + '\0') : null;

            fixed (byte* inputPtr = inputBytes)
            fixed (byte* contextPtr = contextBytes)
            fixed (byte* titlePtr = titleBytes)
            fixed (byte* topicPtr = topicBytes)
            {
                int result = NativeMethods.aether_process(inputPtr, contextPtr, titlePtr, topicPtr, stream ? 1 : 0);
                if (result != 0)
                {
                    var errorMsg = GetErrorMessage(result);
                    Log($"Process failed: {errorMsg}");
                    return false;
                }
                return true;
            }
        }
        catch (Exception ex)
        {
            Log($"Process error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Cancel the current processing operation.
    /// </summary>
    public void Cancel()
    {
        if (!_initialized) return;
        int result = NativeMethods.aether_cancel();
        if (result != 0)
        {
            Log($"Cancel failed: {GetErrorMessage(result)}");
        }
    }

    #endregion

    #region Configuration

    /// <summary>
    /// Load configuration as JSON string.
    /// </summary>
    public unsafe string? LoadConfigJson()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_load_config(&jsonPtr, &len);
            if (result != 0)
            {
                Log($"LoadConfig failed: {GetErrorMessage(result)}");
                return null;
            }

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)jsonPtr, (int)len);
            }
            finally
            {
                if (jsonPtr != null)
                    NativeMethods.aether_free_string(jsonPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"LoadConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Reload configuration from disk.
    /// </summary>
    public void ReloadConfig()
    {
        if (!_initialized) return;
        int result = NativeMethods.aether_reload_config();
        if (result != 0)
        {
            Log($"ReloadConfig failed: {GetErrorMessage(result)}");
        }
    }

    /// <summary>
    /// Get the default provider name.
    /// </summary>
    public unsafe string? GetDefaultProvider()
    {
        if (!_initialized) return null;

        try
        {
            byte* providerPtr = null;
            int result = NativeMethods.aether_get_default_provider(&providerPtr);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)providerPtr);
            }
            finally
            {
                if (providerPtr != null)
                    NativeMethods.aether_free_string(providerPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"GetDefaultProvider error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Set the default provider.
    /// </summary>
    public unsafe bool SetDefaultProvider(string providerName)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(providerName + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_set_default_provider(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"SetDefaultProvider error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Update a provider configuration.
    /// </summary>
    public unsafe bool UpdateProvider(string name, string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var nameBytes = Encoding.UTF8.GetBytes(name + '\0');
            var configBytes = Encoding.UTF8.GetBytes(configJson + '\0');

            fixed (byte* namePtr = nameBytes)
            fixed (byte* configPtr = configBytes)
            {
                int result = NativeMethods.aether_update_provider(namePtr, configPtr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateProvider error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Delete a provider.
    /// </summary>
    public unsafe bool DeleteProvider(string name)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(name + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_delete_provider(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"DeleteProvider error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Test a provider connection.
    /// </summary>
    public unsafe (bool Success, string Message) TestProviderConnection(string providerName, string configJson)
    {
        if (!_initialized) return (false, "Not initialized");

        try
        {
            var nameBytes = Encoding.UTF8.GetBytes(providerName + '\0');
            var configBytes = Encoding.UTF8.GetBytes(configJson + '\0');
            int success = 0;
            byte* messagePtr = null;

            fixed (byte* namePtr = nameBytes)
            fixed (byte* configPtr = configBytes)
            {
                int result = NativeMethods.aether_test_provider_connection(namePtr, configPtr, &success, &messagePtr);
                if (result != 0)
                {
                    return (false, GetErrorMessage(result));
                }

                try
                {
                    var message = Marshal.PtrToStringUTF8((IntPtr)messagePtr) ?? "";
                    return (success != 0, message);
                }
                finally
                {
                    if (messagePtr != null)
                        NativeMethods.aether_free_string(messagePtr);
                }
            }
        }
        catch (Exception ex)
        {
            return (false, ex.Message);
        }
    }

    #endregion

    #region Memory

    /// <summary>
    /// Search memories.
    /// </summary>
    public unsafe string? SearchMemory(string query, int limit = 10)
    {
        if (!_initialized) return null;

        try
        {
            var queryBytes = Encoding.UTF8.GetBytes(query + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* queryPtr = queryBytes)
            {
                int result = NativeMethods.aether_search_memory(queryPtr, limit, &jsonPtr, &len);
                if (result != 0) return null;

                try
                {
                    return Marshal.PtrToStringUTF8((IntPtr)jsonPtr, (int)len);
                }
                finally
                {
                    if (jsonPtr != null)
                        NativeMethods.aether_free_string(jsonPtr);
                }
            }
        }
        catch (Exception ex)
        {
            Log($"SearchMemory error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Clear all memories.
    /// </summary>
    public bool ClearMemory()
    {
        if (!_initialized) return false;
        int result = NativeMethods.aether_clear_memory();
        return result == 0;
    }

    /// <summary>
    /// Get memory statistics.
    /// </summary>
    public unsafe string? GetMemoryStats()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_memory_stats(&jsonPtr, &len);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)jsonPtr, (int)len);
            }
            finally
            {
                if (jsonPtr != null)
                    NativeMethods.aether_free_string(jsonPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"GetMemoryStats error: {ex.Message}");
            return null;
        }
    }

    #endregion

    #region Tools

    /// <summary>
    /// List available tools.
    /// </summary>
    public unsafe string? ListTools()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_tools(&jsonPtr, &len);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)jsonPtr, (int)len);
            }
            finally
            {
                if (jsonPtr != null)
                    NativeMethods.aether_free_string(jsonPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"ListTools error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get root commands for command completion.
    /// </summary>
    public unsafe string? GetRootCommands()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_root_commands(&jsonPtr, &len);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)jsonPtr, (int)len);
            }
            finally
            {
                if (jsonPtr != null)
                    NativeMethods.aether_free_string(jsonPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"GetRootCommands error: {ex.Message}");
            return null;
        }
    }

    #endregion

    #region Logging

    /// <summary>
    /// Set the log level.
    /// </summary>
    /// <param name="level">0=Error, 1=Warn, 2=Info, 3=Debug, 4=Trace</param>
    public void SetLogLevel(int level)
    {
        NativeMethods.aether_set_log_level(level);
    }

    /// <summary>
    /// Get the log directory path.
    /// </summary>
    public unsafe string? GetLogDirectory()
    {
        try
        {
            byte* pathPtr = null;
            int result = NativeMethods.aether_get_log_directory(&pathPtr);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)pathPtr);
            }
            finally
            {
                if (pathPtr != null)
                    NativeMethods.aether_free_string(pathPtr);
            }
        }
        catch (Exception ex)
        {
            Log($"GetLogDirectory error: {ex.Message}");
            return null;
        }
    }

    #endregion

    #region Callback Registration

    private unsafe void RegisterCallbacks()
    {
        Log("Registering callbacks...");

        NativeMethods.aether_register_state_callback(&OnStateChangeCallback);
        NativeMethods.aether_register_stream_callback(&OnStreamCallback);
        NativeMethods.aether_register_complete_callback(&OnCompleteCallback);
        NativeMethods.aether_register_error_callback(&OnErrorCallback);
        NativeMethods.aether_register_tool_callback(&OnToolCallback);
        NativeMethods.aether_register_memory_stored_callback(&OnMemoryStoredCallback);

        Log("Callbacks registered");
    }

    #endregion

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
    private static unsafe void OnCompleteCallback(byte* response)
    {
        var str = response != null
            ? Marshal.PtrToStringUTF8((IntPtr)response)
            : "";
        _instance?.DispatchCompleted(str);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnErrorCallback(byte* message, int code)
    {
        var msg = message != null
            ? Marshal.PtrToStringUTF8((IntPtr)message)
            : "Unknown error";
        _instance?.DispatchError(msg, code);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnToolCallback(byte* toolName, int status, byte* result)
    {
        var name = toolName != null
            ? Marshal.PtrToStringUTF8((IntPtr)toolName)
            : "Unknown";
        var res = result != null
            ? Marshal.PtrToStringUTF8((IntPtr)result)
            : "";
        _instance?.DispatchToolExecuted(name, status, res);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnMemoryStoredCallback()
    {
        _instance?.DispatchMemoryStored();
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

    private void DispatchCompleted(string? response)
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            Log($"Completed");
            Completed?.Invoke(response ?? "");
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

    private void DispatchToolExecuted(string? toolName, int status, string? result)
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            Log($"Tool: {toolName}, status: {status}");
            ToolExecuted?.Invoke(toolName ?? "", status, result ?? "");
        });
    }

    private void DispatchMemoryStored()
    {
        _dispatcherQueue.TryEnqueue(() =>
        {
            Log("Memory stored");
            MemoryStored?.Invoke();
        });
    }

    #endregion

    #region Error Handling

    private static string GetErrorMessage(int code)
    {
        return code switch
        {
            0 => "Success",
            -1 => "Invalid argument (null pointer)",
            -2 => "Invalid UTF-8 encoding",
            -3 => "Core not initialized",
            -4 => "Already initialized",
            -5 => "Configuration error",
            -6 => "Provider error",
            -7 => "Memory error",
            -8 => "Operation cancelled",
            -99 => "Unknown error",
            _ => $"Error code: {code}"
        };
    }

    /// <summary>
    /// Get the last error message from the core.
    /// </summary>
    public unsafe string? GetLastError()
    {
        try
        {
            byte* messagePtr = null;
            int result = NativeMethods.aether_get_last_error(&messagePtr);
            if (result != 0) return null;

            try
            {
                return Marshal.PtrToStringUTF8((IntPtr)messagePtr);
            }
            finally
            {
                if (messagePtr != null)
                    NativeMethods.aether_free_string(messagePtr);
            }
        }
        catch
        {
            return null;
        }
    }

    #endregion

    #region Logging Helper

    private void Log(string message)
    {
        System.Diagnostics.Debug.WriteLine($"[AetherCore] {message}");
        _dispatcherQueue.TryEnqueue(() =>
        {
            LogMessage?.Invoke(message);
        });
    }

    #endregion

    #region Disposal

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        if (_initialized)
        {
            try
            {
                NativeMethods.aether_clear_callbacks();
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

    #endregion
}

/// <summary>
/// Exception thrown when an Aether core operation fails.
/// </summary>
public class AetherException : Exception
{
    /// <summary>Gets the error code from the native library.</summary>
    public int ErrorCode { get; }

    public AetherException(int errorCode, string message) : base(message)
    {
        ErrorCode = errorCode;
    }
}
