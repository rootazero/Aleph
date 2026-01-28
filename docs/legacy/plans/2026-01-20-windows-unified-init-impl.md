# Windows Unified Initialization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement first-time initialization for Windows that matches macOS, downloading embedding models, creating directories, and installing runtimes on first launch.

**Architecture:** Add C ABI exports to `ffi_cabi.rs` that wrap the existing `init_unified` module, then create C# bindings and a ContentDialog-based progress UI.

**Tech Stack:** Rust (C ABI), C# (WinUI 3, ContentDialog), csbindgen (auto-generation)

---

## Task 1: Add C ABI Callback Types

**Files:**
- Modify: `core/src/ffi_cabi.rs:62-88` (after existing callback types)

**Step 1: Add initialization callback type definitions**

Add after line 88 (after `MemoryStoredCallback`):

```rust
// =============================================================================
// Initialization Callback Types
// =============================================================================

/// Callback for initialization phase started
/// @param phase Phase name (UTF-8, null-terminated)
/// @param current Current phase number (1-based)
/// @param total Total number of phases
pub type InitPhaseStartedCallback = extern "C" fn(phase: *const c_char, current: u32, total: u32);

/// Callback for initialization progress within a phase
/// @param phase Phase name
/// @param progress Progress 0.0 to 1.0
/// @param message Status message (UTF-8, null-terminated)
pub type InitPhaseProgressCallback = extern "C" fn(phase: *const c_char, progress: f64, message: *const c_char);

/// Callback for initialization phase completed
/// @param phase Phase name that completed
pub type InitPhaseCompletedCallback = extern "C" fn(phase: *const c_char);

/// Callback for download progress (e.g., embedding model)
/// @param item Item being downloaded
/// @param downloaded Bytes downloaded
/// @param total Total bytes (0 if unknown)
pub type InitDownloadProgressCallback = extern "C" fn(item: *const c_char, downloaded: u64, total: u64);

/// Callback for initialization error
/// @param phase Phase where error occurred
/// @param message Error message
/// @param is_retryable 1 if retry might succeed, 0 otherwise
pub type InitErrorCallback = extern "C" fn(phase: *const c_char, message: *const c_char, is_retryable: c_int);
```

**Step 2: Verify the code compiles**

Run: `cd core && cargo check --no-default-features --features cabi`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add core/src/ffi_cabi.rs
git commit -m "feat(ffi): add initialization callback types for Windows C ABI"
```

---

## Task 2: Add Initialization Callback Storage

**Files:**
- Modify: `core/src/ffi_cabi.rs:95-112` (Callbacks struct)

**Step 1: Add init callbacks to storage struct**

Replace the `Callbacks` struct (around line 95-112):

```rust
struct Callbacks {
    state: Option<StateChangeCallback>,
    stream: Option<StreamTextCallback>,
    complete: Option<CompleteCallback>,
    error: Option<ErrorCallback>,
    tool: Option<ToolCallback>,
    memory_stored: Option<MemoryStoredCallback>,
    // Initialization callbacks
    init_phase_started: Option<InitPhaseStartedCallback>,
    init_phase_progress: Option<InitPhaseProgressCallback>,
    init_phase_completed: Option<InitPhaseCompletedCallback>,
    init_download_progress: Option<InitDownloadProgressCallback>,
    init_error: Option<InitErrorCallback>,
}

static CALLBACKS: Mutex<Callbacks> = Mutex::new(Callbacks {
    state: None,
    stream: None,
    complete: None,
    error: None,
    tool: None,
    memory_stored: None,
    init_phase_started: None,
    init_phase_progress: None,
    init_phase_completed: None,
    init_download_progress: None,
    init_error: None,
});
```

**Step 2: Verify the code compiles**

Run: `cd core && cargo check --no-default-features --features cabi`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add core/src/ffi_cabi.rs
git commit -m "feat(ffi): add initialization callback storage"
```

---

## Task 3: Add Initialization FFI Functions

**Files:**
- Modify: `core/src/ffi_cabi.rs` (add new section before Tests)

**Step 1: Add the initialization functions section**

Add before the `// Tests` section (around line 2258):

```rust
// =============================================================================
// First-Time Initialization Functions
// =============================================================================

/// Check if first-time initialization is needed
///
/// Returns:
/// * `1` if initialization is needed
/// * `0` if already initialized
#[no_mangle]
pub extern "C" fn aether_needs_first_time_init() -> c_int {
    match crate::init_unified::needs_initialization() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            tracing::warn!("Error checking initialization: {}, defaulting to needed", e);
            1
        }
    }
}

/// Check if the embedding model is installed
///
/// Returns:
/// * `1` if model exists
/// * `0` if model does not exist
#[no_mangle]
pub extern "C" fn aether_check_embedding_model_exists() -> c_int {
    use crate::memory::EmbeddingModel;

    match EmbeddingModel::get_default_model_path() {
        Ok(cache_dir) => {
            let model_dir = cache_dir.join("models--BAAI--bge-small-zh-v1.5");
            if !model_dir.exists() {
                return 0;
            }

            let snapshots_dir = model_dir.join("snapshots");
            if !snapshots_dir.exists() {
                return 0;
            }

            if let Ok(entries) = std::fs::read_dir(&snapshots_dir) {
                for entry in entries.flatten() {
                    let snapshot_path = entry.path();
                    if snapshot_path.is_dir() {
                        let model_onnx = snapshot_path.join("model.onnx");
                        let tokenizer_json = snapshot_path.join("tokenizer.json");
                        if model_onnx.exists() && tokenizer_json.exists() {
                            return 1;
                        }
                    }
                }
            }
            0
        }
        Err(_) => 0,
    }
}

/// Register initialization progress callbacks
///
/// # Safety
/// All callback function pointers must be valid for the duration of initialization.
#[no_mangle]
pub unsafe extern "C" fn aether_register_init_callbacks(
    on_phase_started: InitPhaseStartedCallback,
    on_phase_progress: InitPhaseProgressCallback,
    on_phase_completed: InitPhaseCompletedCallback,
    on_download_progress: InitDownloadProgressCallback,
    on_error: InitErrorCallback,
) -> c_int {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.init_phase_started = Some(on_phase_started);
        cbs.init_phase_progress = Some(on_phase_progress);
        cbs.init_phase_completed = Some(on_phase_completed);
        cbs.init_download_progress = Some(on_download_progress);
        cbs.init_error = Some(on_error);
        AETHER_SUCCESS
    } else {
        AETHER_ERR_UNKNOWN
    }
}

/// Clear initialization callbacks
#[no_mangle]
pub extern "C" fn aether_clear_init_callbacks() {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.init_phase_started = None;
        cbs.init_phase_progress = None;
        cbs.init_phase_completed = None;
        cbs.init_download_progress = None;
        cbs.init_error = None;
    }
}

/// Adapter to bridge C ABI callbacks to InitProgressHandler trait
struct CAbiProgressHandler;

impl crate::init_unified::InitProgressHandler for CAbiProgressHandler {
    fn on_phase_started(&self, phase: String, current: u32, total: u32) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_started {
                if let Ok(phase_cstr) = CString::new(phase) {
                    cb(phase_cstr.as_ptr(), current, total);
                }
            }
        }
    }

    fn on_phase_progress(&self, phase: String, progress: f64, message: String) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_progress {
                if let (Ok(phase_cstr), Ok(msg_cstr)) = (CString::new(phase), CString::new(message)) {
                    cb(phase_cstr.as_ptr(), progress, msg_cstr.as_ptr());
                }
            }
        }
    }

    fn on_phase_completed(&self, phase: String) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_completed {
                if let Ok(phase_cstr) = CString::new(phase) {
                    cb(phase_cstr.as_ptr());
                }
            }
        }
    }

    fn on_download_progress(&self, item: String, downloaded: u64, total: u64) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_download_progress {
                if let Ok(item_cstr) = CString::new(item) {
                    cb(item_cstr.as_ptr(), downloaded, total);
                }
            }
        }
    }

    fn on_error(&self, phase: String, message: String, is_retryable: bool) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_error {
                if let (Ok(phase_cstr), Ok(msg_cstr)) = (CString::new(phase), CString::new(message)) {
                    cb(phase_cstr.as_ptr(), msg_cstr.as_ptr(), if is_retryable { 1 } else { 0 });
                }
            }
        }
    }
}

/// Run first-time initialization
///
/// This is a blocking function that runs the full 6-phase initialization.
/// Progress is reported via registered callbacks.
///
/// Returns:
/// * `0` on success
/// * Negative error code on failure
#[no_mangle]
pub extern "C" fn aether_run_first_time_init() -> c_int {
    use crate::init_unified::InitializationCoordinator;
    use std::sync::Arc;

    // Create tokio runtime for async operations
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("Failed to create tokio runtime: {}", e);
            return AETHER_ERR_UNKNOWN;
        }
    };

    let handler = Arc::new(CAbiProgressHandler);

    let result = rt.block_on(async {
        match InitializationCoordinator::new(Some(handler)) {
            Ok(coordinator) => coordinator.run().await,
            Err(e) => crate::init_unified::InitializationResult {
                success: false,
                completed_phases: vec![],
                error_phase: Some(e.phase),
                error_message: Some(e.message),
            },
        }
    });

    if result.success {
        AETHER_SUCCESS
    } else {
        tracing::error!(
            "Initialization failed at phase {:?}: {:?}",
            result.error_phase,
            result.error_message
        );
        AETHER_ERR_CONFIG
    }
}
```

**Step 2: Verify the code compiles**

Run: `cd core && cargo check --no-default-features --features cabi`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add core/src/ffi_cabi.rs
git commit -m "feat(ffi): add first-time initialization C ABI functions"
```

---

## Task 4: Build Windows DLL and Generate Bindings

**Files:**
- Build output: `target/release/aethecore.dll`
- Generated: `platforms/windows/Aether/Interop/NativeMethods.g.cs`

**Step 1: Build the Rust core with cabi feature**

Run: `cd core && cargo build --release --no-default-features --features cabi`
Expected: Build succeeds, produces `target/release/libaethecore.dll` (or `.so` on other platforms)

**Step 2: Verify csbindgen regenerates bindings**

The build.rs should automatically regenerate NativeMethods.g.cs. Check:
```bash
grep -n "aether_needs_first_time_init" platforms/windows/Aether/Interop/NativeMethods.g.cs
```
Expected: Find the new function declarations

**Step 3: Commit**

```bash
git add platforms/windows/Aether/Interop/NativeMethods.g.cs
git commit -m "build: regenerate Windows P/Invoke bindings with init functions"
```

---

## Task 5: Add IInitProgressHandler Interface to AetherCore.cs

**Files:**
- Modify: `platforms/windows/Aether/Interop/AetherCore.cs`

**Step 1: Add the interface definition**

Add after line 48 (after existing events, before `#region Properties`):

```csharp
#endregion

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

#region Properties
```

**Step 2: Verify C# syntax**

Run: `~/.uv/python3/bin/python scripts/verify_csharp_syntax.py platforms/windows/Aether/Interop/AetherCore.cs` (or manual review)
Expected: No syntax errors

**Step 3: Commit**

```bash
git add platforms/windows/Aether/Interop/AetherCore.cs
git commit -m "feat(windows): add IInitProgressHandler interface"
```

---

## Task 6: Add Initialization Methods to AetherCore.cs

**Files:**
- Modify: `platforms/windows/Aether/Interop/AetherCore.cs`

**Step 1: Add static callback storage for initialization**

Add after `_instance` declaration (around line 21):

```csharp
private static IInitProgressHandler? _initHandler;
```

**Step 2: Add the initialization methods**

Add new region before `#region Disposal`:

```csharp
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
        // DLL not found means we definitely need init (or can't proceed)
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
        // Register callbacks
        NativeMethods.aether_register_init_callbacks(
            &OnInitPhaseStarted,
            &OnInitPhaseProgress,
            &OnInitPhaseCompleted,
            &OnInitDownloadProgress,
            &OnInitError
        );

        // Run initialization (blocking)
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
        // Clear callbacks
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
```

**Step 3: Commit**

```bash
git add platforms/windows/Aether/Interop/AetherCore.cs
git commit -m "feat(windows): add first-time initialization methods to AetherCore"
```

---

## Task 7: Create InitializationDialog.xaml

**Files:**
- Create: `platforms/windows/Aether/Windows/InitializationDialog.xaml`

**Step 1: Create the XAML file**

```xml
<?xml version="1.0" encoding="utf-8"?>
<ContentDialog
    x:Class="Aether.Windows.InitializationDialog"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="Initializing Aether"
    PrimaryButtonText=""
    SecondaryButtonText=""
    CloseButtonText=""
    IsPrimaryButtonEnabled="False"
    IsSecondaryButtonEnabled="False"
    Closing="OnClosing">

    <StackPanel Width="400" Spacing="16" Padding="8">
        <!-- Current phase -->
        <TextBlock x:Name="PhaseText"
                   Text="Preparing..."
                   FontSize="16"
                   FontWeight="SemiBold"/>

        <!-- Overall progress -->
        <ProgressBar x:Name="OverallProgress"
                     Minimum="0"
                     Maximum="100"
                     Value="0"/>

        <!-- Detail message -->
        <TextBlock x:Name="DetailText"
                   Text=""
                   FontSize="12"
                   Opacity="0.7"
                   TextWrapping="Wrap"/>

        <!-- Download progress panel (hidden by default) -->
        <StackPanel x:Name="DownloadPanel" Visibility="Collapsed" Spacing="4">
            <TextBlock x:Name="DownloadText" FontSize="12"/>
            <ProgressBar x:Name="DownloadProgress" Minimum="0" Maximum="100"/>
        </StackPanel>

        <!-- Error panel (hidden by default) -->
        <StackPanel x:Name="ErrorPanel" Visibility="Collapsed" Spacing="8">
            <TextBlock x:Name="ErrorText"
                       Foreground="{ThemeResource SystemFillColorCriticalBrush}"
                       TextWrapping="Wrap"/>
            <StackPanel Orientation="Horizontal" Spacing="8">
                <Button x:Name="RetryButton"
                        Content="Retry"
                        Click="OnRetryClick"/>
                <Button x:Name="QuitButton"
                        Content="Quit"
                        Click="OnQuitClick"/>
            </StackPanel>
        </StackPanel>
    </StackPanel>
</ContentDialog>
```

**Step 2: Commit**

```bash
git add platforms/windows/Aether/Windows/InitializationDialog.xaml
git commit -m "feat(windows): create InitializationDialog XAML"
```

---

## Task 8: Create InitializationDialog.xaml.cs

**Files:**
- Create: `platforms/windows/Aether/Windows/InitializationDialog.xaml.cs`

**Step 1: Create the code-behind file**

```csharp
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.Interop;

namespace Aether.Windows;

/// <summary>
/// Dialog for first-time initialization progress.
/// </summary>
public sealed partial class InitializationDialog : ContentDialog, IInitProgressHandler
{
    private const int TotalPhases = 6;
    private int _currentPhase = 0;
    private bool _hasError = false;
    private bool _isRetryable = false;
    private bool _isCompleted = false;

    /// <summary>Event fired when user requests retry.</summary>
    public event Action? RetryRequested;

    /// <summary>Event fired when user requests quit.</summary>
    public event Action? QuitRequested;

    /// <summary>Event fired when initialization completes successfully.</summary>
    public event Action? InitCompleted;

    public InitializationDialog()
    {
        InitializeComponent();
    }

    #region IInitProgressHandler Implementation

    public void OnPhaseStarted(string phase, uint current, uint total)
    {
        _currentPhase = (int)current;
        _hasError = false;

        PhaseText.Text = GetPhaseDisplayName(phase);
        DetailText.Text = "";
        ErrorPanel.Visibility = Visibility.Collapsed;
        DownloadPanel.Visibility = Visibility.Collapsed;

        // Update overall progress
        double progress = ((current - 1) / (double)total) * 100;
        OverallProgress.Value = progress;
    }

    public void OnPhaseProgress(string phase, double progress, string message)
    {
        DetailText.Text = message;

        // Update overall progress with phase progress
        double phaseWeight = 100.0 / TotalPhases;
        double baseProgress = ((_currentPhase - 1) / (double)TotalPhases) * 100;
        OverallProgress.Value = baseProgress + (progress * phaseWeight);
    }

    public void OnPhaseCompleted(string phase)
    {
        // Update progress to reflect completed phase
        double progress = (_currentPhase / (double)TotalPhases) * 100;
        OverallProgress.Value = progress;

        // Check if all phases complete
        if (_currentPhase >= TotalPhases)
        {
            _isCompleted = true;
            PhaseText.Text = "Initialization Complete";
            DetailText.Text = "";
            InitCompleted?.Invoke();
        }
    }

    public void OnDownloadProgress(string item, ulong downloaded, ulong total)
    {
        DownloadPanel.Visibility = Visibility.Visible;

        if (total > 0)
        {
            double percent = (downloaded / (double)total) * 100;
            DownloadText.Text = $"Downloading {item}: {FormatBytes(downloaded)} / {FormatBytes(total)}";
            DownloadProgress.Value = percent;
            DownloadProgress.IsIndeterminate = false;
        }
        else
        {
            DownloadText.Text = $"Downloading {item}: {FormatBytes(downloaded)}";
            DownloadProgress.IsIndeterminate = true;
        }
    }

    public void OnError(string phase, string message, bool isRetryable)
    {
        _hasError = true;
        _isRetryable = isRetryable;

        ErrorPanel.Visibility = Visibility.Visible;
        ErrorText.Text = $"Error in {GetPhaseDisplayName(phase)}:\n{message}";
        RetryButton.Visibility = isRetryable ? Visibility.Visible : Visibility.Collapsed;
    }

    #endregion

    #region Event Handlers

    private void OnRetryClick(object sender, RoutedEventArgs e)
    {
        ErrorPanel.Visibility = Visibility.Collapsed;
        _hasError = false;
        RetryRequested?.Invoke();
    }

    private void OnQuitClick(object sender, RoutedEventArgs e)
    {
        QuitRequested?.Invoke();
    }

    private void OnClosing(ContentDialog sender, ContentDialogClosingEventArgs args)
    {
        // Prevent closing unless completed or user chose to quit
        if (!_isCompleted && !_hasError)
        {
            args.Cancel = true;
        }
    }

    #endregion

    #region Helpers

    private static string GetPhaseDisplayName(string phase)
    {
        return phase.ToLowerInvariant() switch
        {
            "directories" => "Creating directories...",
            "config" => "Generating configuration...",
            "embedding_model" or "embeddingmodel" => "Downloading embedding model...",
            "database" => "Initializing database...",
            "runtimes" => "Installing runtimes...",
            "skills" => "Setting up skills...",
            _ => phase
        };
    }

    private static string FormatBytes(ulong bytes)
    {
        string[] suffixes = { "B", "KB", "MB", "GB" };
        int i = 0;
        double size = bytes;
        while (size >= 1024 && i < suffixes.Length - 1)
        {
            size /= 1024;
            i++;
        }
        return $"{size:F1} {suffixes[i]}";
    }

    #endregion
}
```

**Step 2: Commit**

```bash
git add platforms/windows/Aether/Windows/InitializationDialog.xaml.cs
git commit -m "feat(windows): implement InitializationDialog code-behind"
```

---

## Task 9: Refactor App.xaml.cs Startup Flow

**Files:**
- Modify: `platforms/windows/Aether/App.xaml.cs`

**Step 1: Remove Directory.CreateDirectory from GetConfigPath**

Change `GetConfigPath()` (around line 406-416):

```csharp
private static string GetConfigPath()
{
    // Use ~/.aether/ for cross-platform consistency
    // Directory creation is handled by first-time initialization
    var configDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        ".config",
        "aether"
    );
    return Path.Combine(configDir, "config.toml");
}
```

**Step 2: Commit**

```bash
git add platforms/windows/Aether/App.xaml.cs
git commit -m "refactor(windows): remove Directory.CreateDirectory from GetConfigPath"
```

---

## Task 10: Add First-Time Init Check to App.xaml.cs

**Files:**
- Modify: `platforms/windows/Aether/App.xaml.cs`

**Step 1: Add init dialog field**

Add after line 72 (after `_conversationWindow` declaration):

```csharp
private InitializationDialog? _initDialog;
```

**Step 2: Refactor OnLaunched**

Replace `OnLaunched` method (around line 109-135):

```csharp
protected override async void OnLaunched(LaunchActivatedEventArgs args)
{
    _dispatcherQueue = DispatcherQueue.GetForCurrentThread();

    // Check if first-time initialization is needed
    if (AetherCore.NeedsFirstTimeInit())
    {
        await RunFirstTimeInitializationAsync();
    }
    else
    {
        ContinueStartup();
    }
}

private async Task RunFirstTimeInitializationAsync()
{
    // Create a minimal window to host the dialog
    var initWindow = new Microsoft.UI.Xaml.Window();
    initWindow.Content = new Grid();
    initWindow.Activate();

    _initDialog = new InitializationDialog();
    _initDialog.XamlRoot = initWindow.Content.XamlRoot;

    bool success = false;
    bool shouldRetry = true;

    _initDialog.InitCompleted += () =>
    {
        success = true;
        _initDialog.Hide();
    };

    _initDialog.QuitRequested += () =>
    {
        shouldRetry = false;
        _initDialog.Hide();
    };

    _initDialog.RetryRequested += async () =>
    {
        // Run init again on background thread
        await Task.Run(() => AetherCore.RunFirstTimeInit(_initDialog));
    };

    while (shouldRetry && !success)
    {
        // Show dialog
        var dialogTask = _initDialog.ShowAsync();

        // Run initialization on background thread
        await Task.Run(() => AetherCore.RunFirstTimeInit(_initDialog));

        // Wait for dialog to close (either success, quit, or retry)
        await dialogTask;

        if (success)
        {
            break;
        }
    }

    // Close init window
    initWindow.Close();

    if (success)
    {
        ContinueStartup();
    }
    else
    {
        // User chose to quit
        Exit();
    }
}

private void ContinueStartup()
{
    // Initialize services
    InitializeServices();

    // Create windows (but don't show yet)
    CreateWindows();

    // Wire up hotkey handlers
    WireUpHotkeys();

    // Wire up core callbacks
    WireUpCoreCallbacks();

    // Show tray icon
    _trayIconService?.Show();

    // Check for updates on startup (background)
    _ = CheckForUpdatesAsync();

    // Cleanup old update files
    _autoUpdateService?.CleanupOldUpdates();

    System.Diagnostics.Debug.WriteLine("Aether started successfully");
}
```

**Step 3: Simplify InitializeServices**

The initialization of `_aetherCore` should now assume directories exist. Update `InitializeServices`:

```csharp
private void InitializeServices()
{
    try
    {
        // 1. Initialize cursor service (needed for Halo positioning)
        _cursorService = new CursorService();

        // 2. Initialize clipboard service
        _clipboardService = new ClipboardService();

        // 3. Initialize screen capture service
        _screenCaptureService = new ScreenCaptureService();

        // 4. Initialize Rust core (directories already created by first-time init)
        _aetherCore = new AetherCore(_dispatcherQueue!);
        if (!_aetherCore.Initialize())
        {
            System.Diagnostics.Debug.WriteLine("Warning: Aether core initialization failed");
        }

        // 5. Initialize hotkey service
        _hotkeyService = new HotkeyService();

        // 6. Initialize tray icon service
        _trayIconService = new TrayIconService();
        _trayIconService.SettingsRequested += ShowSettings;
        _trayIconService.QuitRequested += Quit;

        // 7. Initialize auto-update service
        _autoUpdateService = new AutoUpdateService();
        _autoUpdateService.UpdateAvailable += OnUpdateAvailable;
    }
    catch (Exception ex)
    {
        System.Diagnostics.Debug.WriteLine($"Service initialization error: {ex.Message}");
    }
}
```

**Step 4: Add missing using**

Add at top of file:

```csharp
using Aether.Windows;
```

**Step 5: Commit**

```bash
git add platforms/windows/Aether/App.xaml.cs
git commit -m "feat(windows): add first-time initialization check to startup"
```

---

## Task 11: Update AetherCore.Initialize to Remove configPath

**Files:**
- Modify: `platforms/windows/Aether/Interop/AetherCore.cs`

**Step 1: Simplify Initialize method**

Replace `Initialize` method (around line 75-133):

```csharp
/// <summary>
/// Initialize the Rust core.
/// Assumes first-time initialization has already run (directories exist).
/// </summary>
public unsafe bool Initialize()
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

        // Call aether_init with default path
        var configPath = GetDefaultConfigPath();
        var pathBytes = Encoding.UTF8.GetBytes(configPath + '\0');
        int result;
        fixed (byte* pathPtr = pathBytes)
        {
            result = NativeMethods.aether_init(pathPtr);
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
        return false;
    }
    catch (Exception ex)
    {
        Log($"Initialization error: {ex.Message}");
        return false;
    }
}
```

**Step 2: Commit**

```bash
git add platforms/windows/Aether/Interop/AetherCore.cs
git commit -m "refactor(windows): simplify AetherCore.Initialize"
```

---

## Task 12: Final Integration Test

**Files:**
- Test: Full application flow

**Step 1: Build the full solution**

On Windows:
```powershell
cd platforms/windows
dotnet build Aether.sln
```

**Step 2: Manual test first-time initialization**

1. Delete `%USERPROFILE%\.config\aether` directory
2. Run the application
3. Verify initialization dialog appears
4. Verify 6 phases complete
5. Verify application starts normally after

**Step 3: Manual test subsequent launch**

1. Run application again (without deleting config)
2. Verify initialization dialog does NOT appear
3. Verify application starts normally

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat(windows): complete unified initialization implementation

- Add C ABI exports for first-time init (5 functions)
- Add IInitProgressHandler interface
- Create InitializationDialog with ContentDialog UI
- Refactor App.xaml.cs startup flow
- Remove old scattered initialization code

Closes #XXX"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Add callback types | `ffi_cabi.rs` |
| 2 | Add callback storage | `ffi_cabi.rs` |
| 3 | Add FFI functions | `ffi_cabi.rs` |
| 4 | Build & generate bindings | `NativeMethods.g.cs` |
| 5 | Add interface | `AetherCore.cs` |
| 6 | Add init methods | `AetherCore.cs` |
| 7 | Create dialog XAML | `InitializationDialog.xaml` |
| 8 | Create dialog code | `InitializationDialog.xaml.cs` |
| 9 | Remove old code | `App.xaml.cs` |
| 10 | Add init check | `App.xaml.cs` |
| 11 | Simplify Initialize | `AetherCore.cs` |
| 12 | Integration test | Manual testing |

Total: 12 tasks, ~15-20 commits
