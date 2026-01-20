using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aether.ViewModels;
using Aether.Interop;
using System.Collections.ObjectModel;
using System.Text.Json;

namespace Aether.Views.Settings;

/// <summary>
/// AI Providers settings page - Master-Detail layout for API keys and connection testing.
/// </summary>
public sealed partial class ProvidersSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<ProviderItem> _providers = new();
    private ProviderItem? _selectedProvider;
    private bool _hasUnsavedChanges;
    private bool _isLoading = true;
    private AetherCore? _core;

    public ProvidersSettingsPage()
    {
        InitializeComponent();
        InitializeProviders();
        ProviderListView.ItemsSource = _providers;
        _isLoading = false;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        _core = App.Instance.Core;
        LoadProvidersFromCore();
    }

    private void LoadProvidersFromCore()
    {
        try
        {
            // Load full config and extract provider info
            var configJson = _core?.LoadConfigJson();
            if (!string.IsNullOrEmpty(configJson))
            {
                var config = JsonSerializer.Deserialize<JsonElement>(configJson);
                if (config.TryGetProperty("providers", out var providers) && providers.ValueKind == JsonValueKind.Object)
                {
                    foreach (var provider in _providers)
                    {
                        if (providers.TryGetProperty(provider.Id, out var p))
                        {
                            provider.ApiKey = p.TryGetProperty("api_key", out var key) ? key.GetString() : null;
                            provider.BaseUrl = p.TryGetProperty("base_url", out var url) ? url.GetString() : null;
                            provider.SelectedModel = p.TryGetProperty("model", out var model) ? model.GetString() : null;
                            provider.IsDefault = p.TryGetProperty("is_default", out var def) && def.GetBoolean();

                            // Update status indicator
                            provider.StatusColor = string.IsNullOrWhiteSpace(provider.ApiKey) && !provider.IsLocalProvider
                                ? new SolidColorBrush(Microsoft.UI.Colors.Gray)
                                : new SolidColorBrush(Microsoft.UI.Colors.Green);
                        }
                    }
                }
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"LoadProvidersFromCore error: {ex.Message}");
        }
    }

    private void InitializeProviders()
    {
        _providers.Add(new ProviderItem
        {
            Id = "openai",
            Name = "OpenAI",
            Description = "GPT-4o and GPT-3.5 models",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/OpenAI.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 16, 163, 127)), // #10a37f
            ApiKeyPlaceholder = "sk-...",
            ApiKeyUrl = "https://platform.openai.com/api-keys",
            Models = new[] { "gpt-4o", "gpt-4-turbo", "gpt-4", "gpt-3.5-turbo" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "anthropic",
            Name = "Anthropic",
            Description = "Claude models for analysis and coding",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Claude.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 217, 119, 87)), // #d97757
            ApiKeyPlaceholder = "sk-ant-...",
            ApiKeyUrl = "https://console.anthropic.com/",
            Models = new[] { "claude-3-5-sonnet-20241022", "claude-3-opus-20240229", "claude-3-sonnet-20240229" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "google-gemini",
            Name = "Google Gemini",
            Description = "Multimodal AI models",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Gemini.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 66, 133, 244)), // #4285f4
            ApiKeyPlaceholder = "AIza...",
            ApiKeyUrl = "https://makersuite.google.com/app/apikey",
            Models = new[] { "gemini-1.5-pro", "gemini-1.5-flash", "gemini-pro" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "ollama",
            Name = "Ollama",
            Description = "Run models locally",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Ollama.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Black), // #000000
            ApiKeyPlaceholder = "",
            ApiKeyUrl = "https://ollama.ai/download",
            BaseUrl = "http://localhost:11434",
            Models = new[] { "llama3", "mistral", "codellama", "mixtral" },
            IsLocalProvider = true,
            RequiresBaseUrl = true
        });

        _providers.Add(new ProviderItem
        {
            Id = "deepseek",
            Name = "DeepSeek",
            Description = "AI models with reasoning",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/DeepSeek.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 0, 102, 204)), // #0066cc
            ApiKeyPlaceholder = "sk-...",
            ApiKeyUrl = "https://platform.deepseek.com/",
            Models = new[] { "deepseek-chat", "deepseek-coder" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "moonshot",
            Name = "Moonshot",
            Description = "Long-context models",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Moonshot.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 107)), // #ff6b6b
            ApiKeyPlaceholder = "sk-...",
            ApiKeyUrl = "https://platform.moonshot.cn/",
            Models = new[] { "moonshot-v1-8k", "moonshot-v1-32k", "moonshot-v1-128k" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "openrouter",
            Name = "OpenRouter",
            Description = "Multiple AI models via unified API",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/OpenRouter.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 139, 92, 246)), // #8b5cf6
            ApiKeyPlaceholder = "sk-or-...",
            ApiKeyUrl = "https://openrouter.ai/keys",
            Models = new[] { "anthropic/claude-3.5-sonnet", "openai/gpt-4-turbo", "google/gemini-pro" },
            IsLocalProvider = false
        });

        _providers.Add(new ProviderItem
        {
            Id = "t8star",
            Name = "T8Star",
            Description = "OpenAI-compatible API proxy",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/OpenAI.svg"),  // Uses OpenAI icon
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 53)), // #FF6B35
            ApiKeyPlaceholder = "sk-...",
            ApiKeyUrl = "https://t8star.com/",
            Models = new[] { "gpt-4-turbo", "gpt-4", "gpt-3.5-turbo" },
            IsLocalProvider = false,
            RequiresBaseUrl = true
        });

        _providers.Add(new ProviderItem
        {
            Id = "azure-openai",
            Name = "Azure OpenAI",
            Description = "Microsoft Azure hosted models",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Azure.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 0, 120, 212)), // #0078d4
            ApiKeyPlaceholder = "",
            ApiKeyUrl = "https://azure.microsoft.com/products/ai-services/openai-service",
            Models = new[] { "gpt-4", "gpt-35-turbo" },
            IsLocalProvider = false,
            RequiresBaseUrl = true
        });

        _providers.Add(new ProviderItem
        {
            Id = "github-copilot",
            Name = "GitHub Copilot",
            Description = "GitHub Copilot API",
            Icon = "\uE8B9",
            IconPath = new Uri("ms-appx:///Assets/ProviderIcons/Github.svg"),
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 36, 41, 46)), // #24292e
            ApiKeyPlaceholder = "",
            ApiKeyUrl = "https://github.com/features/copilot",
            Models = new[] { "gpt-4o", "gpt-4" },
            IsLocalProvider = false
        });

    }

    private void ProviderListView_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ProviderListView.SelectedItem is ProviderItem provider)
        {
            SelectProvider(provider);
        }
        else
        {
            ClearSelection();
        }
    }

    private void SelectProvider(ProviderItem provider)
    {
        _isLoading = true;
        _selectedProvider = provider;

        // Update header
        ProviderNameText.Text = provider.Name;
        ProviderDescText.Text = provider.Description;
        ProviderIconBorder.Background = provider.IconBackground;
        if (provider.IconPath != null)
        {
            ProviderIconImage.Source = new Microsoft.UI.Xaml.Media.Imaging.SvgImageSource(provider.IconPath);
        }

        // Show/hide API key field based on provider type
        ApiKeyGrid.Visibility = provider.IsLocalProvider ? Visibility.Collapsed : Visibility.Visible;
        BaseUrlGrid.Visibility = provider.RequiresBaseUrl ? Visibility.Visible : Visibility.Collapsed;

        // Load saved config
        ApiKeyBox.Password = provider.ApiKey ?? "";
        ApiKeyBox.PlaceholderText = provider.ApiKeyPlaceholder;
        BaseUrlBox.Text = provider.BaseUrl ?? "";
        DefaultProviderToggle.IsOn = provider.IsDefault;

        // Populate model dropdown
        ModelComboBox.Items.Clear();
        foreach (var model in provider.Models)
        {
            ModelComboBox.Items.Add(model);
        }
        if (!string.IsNullOrEmpty(provider.SelectedModel))
        {
            ModelComboBox.SelectedItem = provider.SelectedModel;
        }
        else if (provider.Models.Length > 0)
        {
            ModelComboBox.SelectedIndex = 0;
        }

        // Update API key help link
        GetApiKeyLink.NavigateUri = new Uri(provider.ApiKeyUrl);
        ApiKeyHelpText.Text = provider.IsLocalProvider
            ? "Download and install Ollama to run local models"
            : $"Visit {provider.Name}'s website to get an API key";

        // Show detail panel
        DetailPanel.Visibility = Visibility.Visible;
        EmptyDetailState.Visibility = Visibility.Collapsed;

        _isLoading = false;
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
        TestResultText.Text = "Test your API connection";
        TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    }

    private void ClearSelection()
    {
        _selectedProvider = null;
        DetailPanel.Visibility = Visibility.Collapsed;
        EmptyDetailState.Visibility = Visibility.Visible;
    }

    private void ApiKeyBox_PasswordChanged(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void BaseUrlBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void ModelComboBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void DefaultProviderToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private async void TestConnection_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedProvider == null) return;

        TestConnectionButton.IsEnabled = false;
        TestResultText.Text = "Testing connection...";
        TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Gray);

        try
        {
            // For cloud providers, require API key
            if (!_selectedProvider.IsLocalProvider && string.IsNullOrWhiteSpace(ApiKeyBox.Password))
            {
                TestResultText.Text = "API key required";
                TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Orange);
                ShowStatus("Please enter an API key", InfoBarSeverity.Warning);
                return;
            }

            // Call AetherCore to test connection
            var configJson = JsonSerializer.Serialize(new
            {
                api_key = _selectedProvider.IsLocalProvider ? null : ApiKeyBox.Password,
                base_url = _selectedProvider.IsLocalProvider ? BaseUrlBox.Text : null
            });
            var result = await Task.Run(() => _core?.TestProviderConnection(_selectedProvider.Id, configJson));

            if (result?.Success == true)
            {
                TestResultText.Text = "Connection successful";
                TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Green);
                ShowStatus("Connection test passed", InfoBarSeverity.Success);
            }
            else
            {
                TestResultText.Text = result?.Message ?? "Connection failed";
                TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Red);
                ShowStatus(result?.Message ?? "Connection test failed", InfoBarSeverity.Error);
            }
        }
        catch (Exception ex)
        {
            TestResultText.Text = $"Connection failed: {ex.Message}";
            TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Red);
            ShowStatus($"Connection test failed: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            TestConnectionButton.IsEnabled = true;
        }
    }

    private async void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedProvider == null) return;

        // Update provider config
        _selectedProvider.ApiKey = ApiKeyBox.Password;
        _selectedProvider.BaseUrl = BaseUrlBox.Text;
        _selectedProvider.SelectedModel = ModelComboBox.SelectedItem?.ToString();

        // Handle default provider toggle
        if (DefaultProviderToggle.IsOn)
        {
            // Clear other defaults
            foreach (var p in _providers)
            {
                p.IsDefault = p == _selectedProvider;
            }
        }
        else
        {
            _selectedProvider.IsDefault = false;
        }

        // Update status indicator
        _selectedProvider.StatusColor = (string.IsNullOrWhiteSpace(_selectedProvider.ApiKey) && !_selectedProvider.IsLocalProvider)
            ? new SolidColorBrush(Microsoft.UI.Colors.Gray)
            : new SolidColorBrush(Microsoft.UI.Colors.Green);

        // Save to AetherCore
        var configJson = JsonSerializer.Serialize(new
        {
            api_key = _selectedProvider.ApiKey,
            base_url = _selectedProvider.BaseUrl,
            model = _selectedProvider.SelectedModel,
            is_default = _selectedProvider.IsDefault
        });

        var success = await Task.Run(() => _core?.UpdateProvider(_selectedProvider.Id, configJson) ?? false);

        if (success)
        {
            _hasUnsavedChanges = false;
            SaveButton.IsEnabled = false;
            ShowStatus($"{_selectedProvider.Name} configuration saved", InfoBarSeverity.Success);
        }
        else
        {
            ShowStatus($"Failed to save {_selectedProvider.Name} configuration", InfoBarSeverity.Error);
        }
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusInfoBar.Message = message;
        StatusInfoBar.Severity = severity;
        StatusInfoBar.IsOpen = true;

        if (severity == InfoBarSeverity.Success)
        {
            DispatcherQueue.TryEnqueue(async () =>
            {
                await Task.Delay(3000);
                StatusInfoBar.IsOpen = false;
            });
        }
    }
}

/// <summary>
/// Represents an AI provider item in the list.
/// </summary>
public class ProviderItem
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Description { get; set; } = "";
    public string Icon { get; set; } = "\uE8B9";
    public Uri? IconPath { get; set; }  // Path to SVG icon
    public Brush IconBackground { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public Brush StatusColor { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public string? ApiKey { get; set; }
    public string ApiKeyPlaceholder { get; set; } = "";
    public string ApiKeyUrl { get; set; } = "";
    public string? BaseUrl { get; set; }
    public string? SelectedModel { get; set; }
    public string[] Models { get; set; } = Array.Empty<string>();
    public bool IsDefault { get; set; } = false;
    public bool IsLocalProvider { get; set; } = false;
    public bool RequiresBaseUrl { get; set; } = false;
}
