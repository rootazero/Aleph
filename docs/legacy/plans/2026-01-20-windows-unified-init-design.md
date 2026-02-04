# Windows 统一初始化模块设计

> 日期：2026-01-20
> 状态：已批准，待实现

## 概述

重构 Windows 平台的初始化模块，与 macOS 版本同步，实现首次启动时统一完成以下内容的下载安装：

- 目录结构创建
- 配置文件生成
- 嵌入模型下载
- 向量数据库初始化
- 运行时环境安装（ffmpeg, yt-dlp, uv, fnm）
- 技能目录设置

## 设计决策

| 决策点 | 选择 |
|--------|------|
| FFI 方案 | 复用 init_unified 模块，通过 C ABI 导出 |
| 进度 UI | ContentDialog 模态对话框 |
| 错误处理 | 完全同步 macOS（区分可重试/不可重试，支持重试按钮） |
| 旧代码 | 全面清理，删除分散的初始化逻辑 |

## 架构

```
Rust Core (已有)          C ABI 导出 (新增)           C# 端 (新增)
─────────────────────────────────────────────────────────────────
init_unified/             ffi_cabi.rs                 AlephCore.cs
├─ coordinator.rs    →    aether_needs_init()    →    NeedsFirstTimeInit()
├─ phases (6个)      →    aether_run_init()      →    RunFirstTimeInit()
└─ progress handler  →    callback functions     →    IInitProgressHandler
```

## 详细设计

### 1. Rust C ABI 导出

文件：`core/src/ffi_cabi.rs`

```rust
// ===== 首次运行初始化 =====

/// 检查是否需要首次初始化
/// 返回：1 = 需要, 0 = 不需要
#[no_mangle]
pub extern "C" fn aether_needs_first_time_init() -> c_int

/// 检查嵌入模型是否存在
/// 返回：1 = 存在, 0 = 不存在
#[no_mangle]
pub extern "C" fn aether_check_embedding_model_exists() -> c_int

/// 注册初始化进度回调
#[no_mangle]
pub unsafe extern "C" fn aether_register_init_callbacks(
    on_phase_started: InitPhaseStartedCallback,
    on_phase_progress: InitPhaseProgressCallback,
    on_phase_completed: InitPhaseCompletedCallback,
    on_download_progress: InitDownloadProgressCallback,
    on_error: InitErrorCallback,
) -> c_int

/// 运行首次初始化（阻塞，通过回调报告进度）
/// 返回：0 = 成功, 负数 = 错误码
#[no_mangle]
pub extern "C" fn aether_run_first_time_init() -> c_int

/// 清除初始化回调
#[no_mangle]
pub extern "C" fn aether_clear_init_callbacks()
```

回调函数类型：

```rust
pub type InitPhaseStartedCallback = extern "C" fn(
    phase: *const c_char,
    current: u32,
    total: u32
);

pub type InitPhaseProgressCallback = extern "C" fn(
    phase: *const c_char,
    progress: f64,
    message: *const c_char
);

pub type InitPhaseCompletedCallback = extern "C" fn(
    phase: *const c_char
);

pub type InitDownloadProgressCallback = extern "C" fn(
    item: *const c_char,
    downloaded: u64,
    total: u64
);

pub type InitErrorCallback = extern "C" fn(
    phase: *const c_char,
    message: *const c_char,
    is_retryable: c_int
);
```

### 2. C# P/Invoke 绑定与包装器

#### NativeMethods.g.cs（csbindgen 自动生成）

```csharp
[DllImport("alephcore", CallingConvention = CallingConvention.Cdecl)]
public static extern int aether_needs_first_time_init();

[DllImport("alephcore", CallingConvention = CallingConvention.Cdecl)]
public static extern int aether_check_embedding_model_exists();

[DllImport("alephcore", CallingConvention = CallingConvention.Cdecl)]
public static extern int aether_run_first_time_init();

[DllImport("alephcore", CallingConvention = CallingConvention.Cdecl)]
public static extern unsafe int aether_register_init_callbacks(
    delegate* unmanaged[Cdecl]<byte*, uint, uint, void> onPhaseStarted,
    delegate* unmanaged[Cdecl]<byte*, double, byte*, void> onPhaseProgress,
    delegate* unmanaged[Cdecl]<byte*, void> onPhaseCompleted,
    delegate* unmanaged[Cdecl]<byte*, ulong, ulong, void> onDownloadProgress,
    delegate* unmanaged[Cdecl]<byte*, byte*, int, void> onError
);

[DllImport("alephcore", CallingConvention = CallingConvention.Cdecl)]
public static extern void aether_clear_init_callbacks();
```

#### AlephCore.cs 新增

```csharp
/// <summary>
/// 初始化进度处理接口
/// </summary>
public interface IInitProgressHandler
{
    void OnPhaseStarted(string phase, uint current, uint total);
    void OnPhaseProgress(string phase, double progress, string message);
    void OnPhaseCompleted(string phase);
    void OnDownloadProgress(string item, ulong downloaded, ulong total);
    void OnError(string phase, string message, bool isRetryable);
}

// AlephCore 类新增方法
public bool NeedsFirstTimeInit()
{
    return NativeMethods.aether_needs_first_time_init() == 1;
}

public bool CheckEmbeddingModelExists()
{
    return NativeMethods.aether_check_embedding_model_exists() == 1;
}

public void RunFirstTimeInit(IInitProgressHandler handler)
{
    // 注册回调 → 执行初始化 → 清除回调
}
```

### 3. 初始化进度 UI

#### 新增文件：`Windows/InitializationDialog.xaml`

```xml
<ContentDialog
    x:Class="Aleph.Windows.InitializationDialog"
    Title="初始化 Aleph"
    CloseButtonText=""
    IsPrimaryButtonEnabled="False"
    IsSecondaryButtonEnabled="False">

    <StackPanel Width="400" Spacing="16">
        <!-- 当前阶段 -->
        <TextBlock x:Name="PhaseText" Text="正在准备..." />

        <!-- 总体进度条 -->
        <ProgressBar x:Name="OverallProgress" Maximum="100" />

        <!-- 详细信息 -->
        <TextBlock x:Name="DetailText" Opacity="0.7" FontSize="12" />

        <!-- 下载进度（仅在下载时显示） -->
        <StackPanel x:Name="DownloadPanel" Visibility="Collapsed">
            <TextBlock x:Name="DownloadText" FontSize="12" />
            <ProgressBar x:Name="DownloadProgress" Maximum="100" />
        </StackPanel>

        <!-- 错误面板（仅在出错时显示） -->
        <StackPanel x:Name="ErrorPanel" Visibility="Collapsed">
            <TextBlock x:Name="ErrorText" Foreground="Red" TextWrapping="Wrap" />
            <StackPanel Orientation="Horizontal" Spacing="8" Margin="0,8,0,0">
                <Button x:Name="RetryButton" Content="重试" Click="OnRetryClick" />
                <Button x:Name="QuitButton" Content="退出" Click="OnQuitClick" />
            </StackPanel>
        </StackPanel>
    </StackPanel>
</ContentDialog>
```

#### 阶段本地化

| Phase | 中文 | English |
|-------|------|---------|
| Directories | 创建目录 | Creating directories |
| Config | 生成配置 | Generating config |
| EmbeddingModel | 下载嵌入模型 | Downloading embedding model |
| Database | 初始化数据库 | Initializing database |
| Runtimes | 安装运行时 | Installing runtimes |
| Skills | 设置技能 | Setting up skills |

### 4. App.xaml.cs 启动流程

```
OnLaunched()
    │
    ├─ InitializeBasicServices()      // 仅初始化不依赖 Rust 的服务
    │   ├─ CursorService
    │   ├─ ClipboardService
    │   └─ ScreenCaptureService
    │
    ├─ CheckAndRunFirstTimeInit()     // 首次运行检查
    │   │
    │   ├─ if aether_needs_first_time_init() == true
    │   │   └─ ShowInitializationDialog()
    │   │       ├─ aether_run_first_time_init()
    │   │       ├─ 成功 → ContinueStartup()
    │   │       └─ 失败 → 显示重试/退出选项
    │   │
    │   └─ else → ContinueStartup()
    │
    └─ ContinueStartup()              // 后续初始化
        ├─ AlephCore.Initialize()    // 仅加载已存在的配置
        ├─ HotkeyService
        ├─ TrayIconService
        ├─ AutoUpdateService
        ├─ CreateWindows()
        └─ TrayIcon.Show()
```

### 5. 旧代码清理

#### core/src/ffi_cabi.rs

- 重构 `aether_init`：仅加载配置，不创建目录/文件
- 首次初始化逻辑移至 `aether_run_first_time_init`

#### platforms/windows/Aether/Interop/AetherCore.cs

删除：
- `Initialize(string? configPath)` 的 configPath 参数
- 直接创建目录的代码
- 失败时的静默处理逻辑

简化为：
```csharp
public bool Initialize()
{
    if (_isInitialized) return true;

    RegisterCallbacks();
    var result = NativeMethods.aether_init(null);
    _isInitialized = (result == 0);
    return _isInitialized;
}
```

#### platforms/windows/Aether/App.xaml.cs

删除：
- `GetConfigPath()` 中的 `Directory.CreateDirectory()`
- `InitializeServices()` 中忽略初始化失败的逻辑
- 任何分散的运行时检查代码

重构：
- 拆分为 `InitializeBasicServices()` + `ContinueStartup()`
- 新增 `CheckAndRunFirstTimeInit()`

## 文件变更总览

| 文件 | 操作 | 说明 |
|------|------|------|
| `core/src/ffi_cabi.rs` | 修改 | 添加 5 个新函数，重构 aether_init |
| `core/build.rs` | 无变化 | csbindgen 自动处理新函数 |
| `NativeMethods.g.cs` | 重新生成 | 运行构建后自动更新 |
| `AetherCore.cs` | 修改 | 添加接口，简化 Initialize |
| `App.xaml.cs` | 修改 | 重构启动流程 |
| `InitializationDialog.xaml` | **新增** | 进度对话框 UI |
| `InitializationDialog.xaml.cs` | **新增** | 进度对话框逻辑 |

## 目录结构（初始化完成后）

```
~/.aleph/                    # Windows: %USERPROFILE%\.config\aether
├── config.toml                      # 配置文件
├── memory.db                        # 向量数据库
├── logs/                            # 日志目录
├── cache/                           # 缓存目录
├── skills/                          # 技能目录
├── models/                          # 模型目录
│   └── fastembed/
│       └── models--BAAI--bge-small-zh-v1.5/
│           └── snapshots/{hash}/
│               ├── model.onnx
│               └── tokenizer.json
└── runtimes/                        # 运行时环境
    ├── manifest.json
    ├── ffmpeg/
    ├── yt-dlp/
    ├── uv/
    └── fnm/
```

## 测试计划

1. **首次启动测试**：删除 ~/.aether，启动应用，验证初始化流程
2. **已初始化启动测试**：保留配置，启动应用，验证跳过初始化
3. **网络错误测试**：断网启动，验证重试按钮功能
4. **部分初始化测试**：中断初始化，重启验证恢复逻辑
