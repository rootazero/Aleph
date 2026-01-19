using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Aether.ViewModels;

/// <summary>
/// ViewModel for SettingsWindow.
///
/// Manages:
/// - Settings navigation (11 tabs matching macOS)
/// - Configuration data binding
/// - Save/Cancel operations
/// </summary>
public partial class SettingsViewModel : ObservableObject
{
    #region Observable Properties

    [ObservableProperty]
    private SettingsTab _selectedTab = SettingsTab.General;

    [ObservableProperty]
    private bool _hasUnsavedChanges;

    [ObservableProperty]
    private bool _isSaving;

    [ObservableProperty]
    private string _statusMessage = "";

    #endregion

    #region General Settings

    [ObservableProperty]
    private bool _soundEnabled;

    [ObservableProperty]
    private bool _launchAtStartup;

    [ObservableProperty]
    private string _selectedLanguage = "system";

    [ObservableProperty]
    private string _appVersion = "0.1.0";

    #endregion

    #region Provider Settings

    [ObservableProperty]
    private string _defaultProvider = "openai";

    [ObservableProperty]
    private string _openAiApiKey = "";

    [ObservableProperty]
    private string _anthropicApiKey = "";

    [ObservableProperty]
    private string _geminiApiKey = "";

    #endregion

    #region Shortcut Settings

    [ObservableProperty]
    private string _haloHotkey = "Shift + Shift";

    [ObservableProperty]
    private string _conversationHotkey = "Win + Alt + /";

    #endregion

    #region Behavior Settings

    [ObservableProperty]
    private bool _autoCopyToClipboard = true;

    [ObservableProperty]
    private bool _typewriterEffect = true;

    [ObservableProperty]
    private int _typewriterDelayMs = 20;

    #endregion

    #region Memory Settings

    [ObservableProperty]
    private int _retentionDays = 30;

    [ObservableProperty]
    private bool _autoCompactEnabled = true;

    [ObservableProperty]
    private int _memoryUsageMb;

    #endregion

    public SettingsViewModel()
    {
        LoadSettings();
    }

    #region Commands

    [RelayCommand]
    public void NavigateTo(SettingsTab tab)
    {
        SelectedTab = tab;
    }

    [RelayCommand]
    public async Task SaveSettingsAsync()
    {
        if (IsSaving) return;

        IsSaving = true;
        StatusMessage = "Saving...";

        try
        {
            // TODO: Call Rust core to save settings
            await Task.Delay(500); // Simulate save

            HasUnsavedChanges = false;
            StatusMessage = "Settings saved";

            // Clear status after delay
            await Task.Delay(2000);
            StatusMessage = "";
        }
        catch (Exception ex)
        {
            StatusMessage = $"Error: {ex.Message}";
        }
        finally
        {
            IsSaving = false;
        }
    }

    [RelayCommand]
    public void CancelChanges()
    {
        LoadSettings();
        HasUnsavedChanges = false;
        StatusMessage = "";
    }

    [RelayCommand]
    public void ClearMemory()
    {
        // TODO: Call Rust core to clear memory
        System.Diagnostics.Debug.WriteLine("[Settings] Clear memory requested");
    }

    [RelayCommand]
    public void ViewLogs()
    {
        // TODO: Open log viewer
        System.Diagnostics.Debug.WriteLine("[Settings] View logs requested");
    }

    [RelayCommand]
    public void CheckForUpdates()
    {
        // TODO: Check for updates
        System.Diagnostics.Debug.WriteLine("[Settings] Check updates requested");
    }

    [RelayCommand]
    public void OpenConfigFolder()
    {
        // Use ~/.config/aether/ for cross-platform consistency
        var configPath = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
            ".config",
            "aether"
        );

        if (Directory.Exists(configPath))
        {
            System.Diagnostics.Process.Start("explorer.exe", configPath);
        }
    }

    #endregion

    #region Private Methods

    private void LoadSettings()
    {
        // TODO: Load from Rust core
        // For now, use defaults

        // Get app version
        var version = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
        AppVersion = version?.ToString() ?? "0.1.0";

        // Estimate memory usage (placeholder)
        MemoryUsageMb = 0;
    }

    partial void OnSoundEnabledChanged(bool value) => MarkAsChanged();
    partial void OnLaunchAtStartupChanged(bool value) => MarkAsChanged();
    partial void OnSelectedLanguageChanged(string value) => MarkAsChanged();
    partial void OnDefaultProviderChanged(string value) => MarkAsChanged();
    partial void OnOpenAiApiKeyChanged(string value) => MarkAsChanged();
    partial void OnAnthropicApiKeyChanged(string value) => MarkAsChanged();
    partial void OnGeminiApiKeyChanged(string value) => MarkAsChanged();
    partial void OnAutoCopyToClipboardChanged(bool value) => MarkAsChanged();
    partial void OnTypewriterEffectChanged(bool value) => MarkAsChanged();
    partial void OnTypewriterDelayMsChanged(int value) => MarkAsChanged();
    partial void OnRetentionDaysChanged(int value) => MarkAsChanged();
    partial void OnAutoCompactEnabledChanged(bool value) => MarkAsChanged();

    private void MarkAsChanged()
    {
        HasUnsavedChanges = true;
    }

    #endregion
}

/// <summary>
/// Settings navigation tabs (matches macOS for feature parity).
/// </summary>
public enum SettingsTab
{
    General,
    Providers,
    Generation,
    Routing,
    Shortcuts,
    Behavior,
    Memory,
    Search,
    Mcp,
    Skills,
    Cowork,
    Policies
}

/// <summary>
/// Extension methods for SettingsTab.
/// </summary>
public static class SettingsTabExtensions
{
    public static string GetDisplayName(this SettingsTab tab) => tab switch
    {
        SettingsTab.General => "General",
        SettingsTab.Providers => "Providers",
        SettingsTab.Generation => "Generation",
        SettingsTab.Routing => "Routing Rules",
        SettingsTab.Shortcuts => "Keyboard Shortcuts",
        SettingsTab.Behavior => "Behavior",
        SettingsTab.Memory => "Memory Management",
        SettingsTab.Search => "Search Engine",
        SettingsTab.Mcp => "MCP Tools",
        SettingsTab.Skills => "Claude Skills",
        SettingsTab.Cowork => "Cowork",
        SettingsTab.Policies => "Policies",
        _ => tab.ToString()
    };

    public static string GetIcon(this SettingsTab tab) => tab switch
    {
        SettingsTab.General => "\uE713",      // Settings
        SettingsTab.Providers => "\uE8F1",    // Cloud
        SettingsTab.Generation => "\uE8B9",   // Picture
        SettingsTab.Routing => "\uE8D1",      // Switch
        SettingsTab.Shortcuts => "\uE765",    // Keyboard
        SettingsTab.Behavior => "\uE771",     // Slider
        SettingsTab.Memory => "\uE7C3",       // Brain
        SettingsTab.Search => "\uE721",       // Search
        SettingsTab.Mcp => "\uE912",          // Link
        SettingsTab.Skills => "\uE734",       // Favorite
        SettingsTab.Cowork => "\uE902",       // Flow
        SettingsTab.Policies => "\uE8D4",     // Shield
        _ => "\uE713"
    };

    public static string GetDescription(this SettingsTab tab) => tab switch
    {
        SettingsTab.General => "Configure general application settings including sound, language, and updates",
        SettingsTab.Providers => "Configure AI providers, API keys, and model parameters for routing",
        SettingsTab.Generation => "Configure image, video, audio, and speech generation providers",
        SettingsTab.Routing => "Define how content is routed to AI providers based on patterns",
        SettingsTab.Shortcuts => "Configure global keyboard shortcuts for Aether",
        SettingsTab.Behavior => "Configure how Aether captures input and delivers output",
        SettingsTab.Memory => "Aether remembers past interactions to provide context-aware responses",
        SettingsTab.Search => "Configure web search providers for AI-augmented information retrieval",
        SettingsTab.Mcp => "Configure Model Context Protocol services for AI tool access",
        SettingsTab.Skills => "Install and manage Claude Agent Skills for enhanced AI capabilities",
        SettingsTab.Cowork => "Configure Cowork task orchestration for complex requests",
        SettingsTab.Policies => "Configure behavioral parameters that control how Aether operates",
        _ => ""
    };
}
