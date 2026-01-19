using System.Runtime.InteropServices;
using System.Runtime.CompilerServices;
using System.Text;
using Microsoft.UI.Dispatching;

namespace Aether.Interop;

#region Initialization Progress Interface

/// <summary>
/// Interface for receiving initialization progress updates.
/// </summary>
public interface IInitProgressHandler
{
    /// <summary>Called when a phase starts.</summary>
    void OnPhaseStarted(string phase, uint current, uint total);

    /// <summary>Called for progress updates within a phase.</summary>
    void OnPhaseProgress(string phase, double progress, string message);

    /// <summary>Called when a phase completes.</summary>
    void OnPhaseCompleted(string phase);

    /// <summary>Called for download progress.</summary>
    void OnDownloadProgress(string item, ulong downloaded, ulong total);

    /// <summary>Called when an error occurs.</summary>
    void OnError(string phase, string message, bool isRetryable);
}

#endregion

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
    private static IInitProgressHandler? _initHandler;
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
        // Use ~/.config/aether/ for cross-platform consistency
        var userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return Path.Combine(userProfile, ".config", "aether", "config.toml");
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

    #region MCP Server Management

    /// <summary>
    /// List all MCP servers as JSON.
    /// </summary>
    public unsafe string? ListMcpServers()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_mcp_servers(&jsonPtr, &len);
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
            Log($"ListMcpServers error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Add an MCP server.
    /// </summary>
    public unsafe bool AddMcpServer(string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(configJson + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_add_mcp_server(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"AddMcpServer error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Update an MCP server.
    /// </summary>
    public unsafe bool UpdateMcpServer(string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(configJson + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_update_mcp_server(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateMcpServer error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Delete an MCP server.
    /// </summary>
    public unsafe bool DeleteMcpServer(string serverId)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(serverId + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_delete_mcp_server(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"DeleteMcpServer error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Get MCP server status as JSON.
    /// </summary>
    public unsafe string? GetMcpServerStatus(string serverId)
    {
        if (!_initialized) return null;

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(serverId + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* idPtr = idBytes)
            {
                int result = NativeMethods.aether_get_mcp_server_status(idPtr, &jsonPtr, &len);
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
            Log($"GetMcpServerStatus error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Export MCP config as JSON.
    /// </summary>
    public unsafe string? ExportMcpConfig()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_export_mcp_config(&jsonPtr, &len);
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
            Log($"ExportMcpConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Import MCP config from JSON.
    /// </summary>
    public unsafe bool ImportMcpConfig(string json)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(json + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_import_mcp_config(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"ImportMcpConfig error: {ex.Message}");
            return false;
        }
    }

    #endregion

    #region Skills Management

    /// <summary>
    /// List all installed skills as JSON.
    /// </summary>
    public unsafe string? ListSkills()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_skills(&jsonPtr, &len);
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
            Log($"ListSkills error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Install a skill from URL.
    /// </summary>
    public unsafe string? InstallSkill(string url)
    {
        if (!_initialized) return null;

        try
        {
            var urlBytes = Encoding.UTF8.GetBytes(url + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* urlPtr = urlBytes)
            {
                int result = NativeMethods.aether_install_skill(urlPtr, &jsonPtr, &len);
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
            Log($"InstallSkill error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Install skills from ZIP file.
    /// </summary>
    public unsafe string? InstallSkillsFromZip(string zipPath)
    {
        if (!_initialized) return null;

        try
        {
            var pathBytes = Encoding.UTF8.GetBytes(zipPath + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* pathPtr = pathBytes)
            {
                int result = NativeMethods.aether_install_skills_from_zip(pathPtr, &jsonPtr, &len);
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
            Log($"InstallSkillsFromZip error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Delete a skill.
    /// </summary>
    public unsafe bool DeleteSkill(string skillId)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(skillId + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_delete_skill(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"DeleteSkill error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Get skills directory path.
    /// </summary>
    public unsafe string? GetSkillsDirectory()
    {
        try
        {
            byte* pathPtr = null;
            int result = NativeMethods.aether_get_skills_dir(&pathPtr);
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
            Log($"GetSkillsDirectory error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Refresh skills registry.
    /// </summary>
    public bool RefreshSkills()
    {
        if (!_initialized) return false;
        int result = NativeMethods.aether_refresh_skills();
        return result == 0;
    }

    #endregion

    #region Generation Provider Management

    /// <summary>
    /// List all generation providers as JSON.
    /// </summary>
    public unsafe string? ListGenerationProviders()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_generation_providers(&jsonPtr, &len);
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
            Log($"ListGenerationProviders error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get generation provider configuration as JSON.
    /// </summary>
    public unsafe string? GetGenerationProviderConfig(string providerId)
    {
        if (!_initialized) return null;

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* idPtr = idBytes)
            {
                int result = NativeMethods.aether_get_generation_provider_config(idPtr, &jsonPtr, &len);
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
            Log($"GetGenerationProviderConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update generation provider configuration.
    /// </summary>
    public unsafe bool UpdateGenerationProvider(string providerId, string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            var configBytes = Encoding.UTF8.GetBytes(configJson + '\0');

            fixed (byte* idPtr = idBytes)
            fixed (byte* configPtr = configBytes)
            {
                int result = NativeMethods.aether_update_generation_provider(idPtr, configPtr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateGenerationProvider error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Test generation provider connection.
    /// </summary>
    public unsafe (bool Success, string Message) TestGenerationProvider(string providerId, string apiKey)
    {
        if (!_initialized) return (false, "Not initialized");

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            var keyBytes = Encoding.UTF8.GetBytes(apiKey + '\0');
            int success = 0;
            byte* messagePtr = null;

            fixed (byte* idPtr = idBytes)
            fixed (byte* keyPtr = keyBytes)
            {
                int result = NativeMethods.aether_test_generation_provider(idPtr, keyPtr, &success, &messagePtr);
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

    #region Routing Configuration

    /// <summary>
    /// Get routing configuration as JSON.
    /// </summary>
    public unsafe string? GetRoutingConfig()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_routing_config(&jsonPtr, &len);
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
            Log($"GetRoutingConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update routing configuration.
    /// </summary>
    public unsafe bool UpdateRoutingConfig(string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(configJson + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_update_routing_config(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateRoutingConfig error: {ex.Message}");
            return false;
        }
    }

    #endregion

    #region Behavior Configuration

    /// <summary>
    /// Get behavior configuration as JSON.
    /// </summary>
    public unsafe string? GetBehaviorConfig()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_behavior_config(&jsonPtr, &len);
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
            Log($"GetBehaviorConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update behavior configuration.
    /// </summary>
    public unsafe bool UpdateBehaviorConfig(string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(configJson + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_update_behavior_config(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateBehaviorConfig error: {ex.Message}");
            return false;
        }
    }

    #endregion

    #region Search Provider Management

    /// <summary>
    /// List all search providers as JSON.
    /// </summary>
    public unsafe string? ListSearchProviders()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_search_providers(&jsonPtr, &len);
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
            Log($"ListSearchProviders error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get search provider configuration as JSON.
    /// </summary>
    public unsafe string? GetSearchProviderConfig(string providerId)
    {
        if (!_initialized) return null;

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            byte* jsonPtr = null;
            nuint len = 0;

            fixed (byte* idPtr = idBytes)
            {
                int result = NativeMethods.aether_get_search_provider_config(idPtr, &jsonPtr, &len);
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
            Log($"GetSearchProviderConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update search provider configuration.
    /// </summary>
    public unsafe bool UpdateSearchProvider(string providerId, string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            var configBytes = Encoding.UTF8.GetBytes(configJson + '\0');

            fixed (byte* idPtr = idBytes)
            fixed (byte* configPtr = configBytes)
            {
                int result = NativeMethods.aether_update_search_provider(idPtr, configPtr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateSearchProvider error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Test search provider connection.
    /// </summary>
    public unsafe (bool Success, string Message) TestSearchProvider(string providerId, string apiKey)
    {
        if (!_initialized) return (false, "Not initialized");

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(providerId + '\0');
            var keyBytes = Encoding.UTF8.GetBytes(apiKey + '\0');
            int success = 0;
            byte* messagePtr = null;

            fixed (byte* idPtr = idBytes)
            fixed (byte* keyPtr = keyBytes)
            {
                int result = NativeMethods.aether_test_search_provider(idPtr, keyPtr, &success, &messagePtr);
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

    #region Cowork Configuration

    /// <summary>
    /// Get cowork configuration as JSON.
    /// </summary>
    public unsafe string? GetCoworkConfig()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_cowork_config(&jsonPtr, &len);
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
            Log($"GetCoworkConfig error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update cowork configuration.
    /// </summary>
    public unsafe bool UpdateCoworkConfig(string configJson)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(configJson + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_update_cowork_config(ptr);
                return result == 0;
            }
        }
        catch (Exception ex)
        {
            Log($"UpdateCoworkConfig error: {ex.Message}");
            return false;
        }
    }

    #endregion

    #region Policies (Read-Only)

    /// <summary>
    /// Get all policies as JSON (read-only, managed by admin).
    /// </summary>
    public unsafe string? GetPolicies()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_get_policies(&jsonPtr, &len);
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
            Log($"GetPolicies error: {ex.Message}");
            return null;
        }
    }

    #endregion

    #region Runtime Management

    /// <summary>
    /// List all runtimes as JSON.
    /// </summary>
    public unsafe string? ListRuntimes()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_list_runtimes(&jsonPtr, &len);
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
            Log($"ListRuntimes error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Check if a runtime is installed.
    /// </summary>
    public unsafe bool IsRuntimeInstalled(string runtimeId)
    {
        if (!_initialized) return false;

        try
        {
            var bytes = Encoding.UTF8.GetBytes(runtimeId + '\0');
            fixed (byte* ptr = bytes)
            {
                int result = NativeMethods.aether_is_runtime_installed(ptr);
                return result == 1;
            }
        }
        catch (Exception ex)
        {
            Log($"IsRuntimeInstalled error: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Install a runtime.
    /// </summary>
    public unsafe (bool Success, string Message) InstallRuntime(string runtimeId)
    {
        if (!_initialized) return (false, "Not initialized");

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(runtimeId + '\0');
            byte* messagePtr = null;

            fixed (byte* idPtr = idBytes)
            {
                int result = NativeMethods.aether_install_runtime(idPtr, &messagePtr);

                try
                {
                    var message = messagePtr != null
                        ? Marshal.PtrToStringUTF8((IntPtr)messagePtr) ?? ""
                        : "";
                    return (result == 0, result == 0 ? message : GetErrorMessage(result));
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

    /// <summary>
    /// Check for runtime updates.
    /// </summary>
    public unsafe string? CheckRuntimeUpdates()
    {
        if (!_initialized) return null;

        try
        {
            byte* jsonPtr = null;
            nuint len = 0;

            int result = NativeMethods.aether_check_runtime_updates(&jsonPtr, &len);
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
            Log($"CheckRuntimeUpdates error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Update a runtime.
    /// </summary>
    public unsafe (bool Success, string Message) UpdateRuntime(string runtimeId)
    {
        if (!_initialized) return (false, "Not initialized");

        try
        {
            var idBytes = Encoding.UTF8.GetBytes(runtimeId + '\0');
            byte* messagePtr = null;

            fixed (byte* idPtr = idBytes)
            {
                int result = NativeMethods.aether_update_runtime(idPtr, &messagePtr);

                try
                {
                    var message = messagePtr != null
                        ? Marshal.PtrToStringUTF8((IntPtr)messagePtr) ?? ""
                        : "";
                    return (result == 0, result == 0 ? message : GetErrorMessage(result));
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

    /// <summary>
    /// Set runtime auto-update preference.
    /// </summary>
    public bool SetRuntimeAutoUpdate(bool enabled)
    {
        if (!_initialized) return false;
        int result = NativeMethods.aether_set_runtime_auto_update(enabled ? 1 : 0);
        return result == 0;
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

    #region First-Time Initialization

    /// <summary>
    /// Check if first-time initialization is needed.
    /// </summary>
    public static bool NeedsFirstTimeInit()
    {
        try
        {
            return NativeMethods.aether_needs_first_time_init() == 1;
        }
        catch (DllNotFoundException)
        {
            return true;
        }
    }

    /// <summary>
    /// Check if embedding model exists.
    /// </summary>
    public static bool CheckEmbeddingModelExists()
    {
        try
        {
            return NativeMethods.aether_check_embedding_model_exists() == 1;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>
    /// Run first-time initialization with progress handler.
    /// This is blocking and should be called from a background thread.
    /// </summary>
    public static unsafe bool RunFirstTimeInit(IInitProgressHandler handler)
    {
        _initHandler = handler;

        try
        {
            NativeMethods.aether_register_init_callbacks(
                &OnInitPhaseStarted,
                &OnInitPhaseProgress,
                &OnInitPhaseCompleted,
                &OnInitDownloadProgress,
                &OnInitError
            );

            int result = NativeMethods.aether_run_first_time_init();
            return result == 0;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[AetherCore] Init error: {ex.Message}");
            return false;
        }
        finally
        {
            NativeMethods.aether_clear_init_callbacks();
            _initHandler = null;
        }
    }

    #region Init Callback Methods

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnInitPhaseStarted(byte* phase, uint current, uint total)
    {
        var phaseStr = phase != null ? Marshal.PtrToStringUTF8((IntPtr)phase) ?? "" : "";
        _instance?._dispatcherQueue.TryEnqueue(() =>
        {
            _initHandler?.OnPhaseStarted(phaseStr, current, total);
        });
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnInitPhaseProgress(byte* phase, double progress, byte* message)
    {
        var phaseStr = phase != null ? Marshal.PtrToStringUTF8((IntPtr)phase) ?? "" : "";
        var msgStr = message != null ? Marshal.PtrToStringUTF8((IntPtr)message) ?? "" : "";
        _instance?._dispatcherQueue.TryEnqueue(() =>
        {
            _initHandler?.OnPhaseProgress(phaseStr, progress, msgStr);
        });
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnInitPhaseCompleted(byte* phase)
    {
        var phaseStr = phase != null ? Marshal.PtrToStringUTF8((IntPtr)phase) ?? "" : "";
        _instance?._dispatcherQueue.TryEnqueue(() =>
        {
            _initHandler?.OnPhaseCompleted(phaseStr);
        });
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnInitDownloadProgress(byte* item, ulong downloaded, ulong total)
    {
        var itemStr = item != null ? Marshal.PtrToStringUTF8((IntPtr)item) ?? "" : "";
        _instance?._dispatcherQueue.TryEnqueue(() =>
        {
            _initHandler?.OnDownloadProgress(itemStr, downloaded, total);
        });
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnInitError(byte* phase, byte* message, int isRetryable)
    {
        var phaseStr = phase != null ? Marshal.PtrToStringUTF8((IntPtr)phase) ?? "" : "";
        var msgStr = message != null ? Marshal.PtrToStringUTF8((IntPtr)message) ?? "" : "";
        _instance?._dispatcherQueue.TryEnqueue(() =>
        {
            _initHandler?.OnError(phaseStr, msgStr, isRetryable != 0);
        });
    }

    #endregion

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
