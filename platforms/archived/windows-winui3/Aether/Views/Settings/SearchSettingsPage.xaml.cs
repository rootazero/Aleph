using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aleph.ViewModels;
using Aleph.Interop;
using System.Collections.ObjectModel;
using System.Text.Json;

namespace Aleph.Views.Settings;

/// <summary>
/// Search Settings page - Search provider configuration.
/// Supports web search and code search providers.
/// </summary>
public sealed partial class SearchSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<SearchProviderItem> _providers = new();
    private SearchProviderItem? _selectedProvider;
    private bool _hasUnsavedChanges;
    private bool _isLoading = true;
    private AlephCore? _core;

    public SearchSettingsPage()
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
            var providersJson = _core?.ListSearchProviders();
            if (!string.IsNullOrEmpty(providersJson))
            {
                var providers = JsonSerializer.Deserialize<JsonElement>(providersJson);
                if (providers.ValueKind == JsonValueKind.Array)
                {
                    foreach (var p in providers.EnumerateArray())
                    {
                        var id = p.GetProperty("id").GetString() ?? "";
                        var existing = _providers.FirstOrDefault(x => x.Id == id);
                        if (existing != null)
                        {
                            existing.ApiKey = p.TryGetProperty("api_key", out var key) ? key.GetString() : null;
                            existing.BaseUrl = p.TryGetProperty("base_url", out var url) ? url.GetString() : null;
                            existing.SearchId = p.TryGetProperty("search_id", out var sid) ? sid.GetString() : null;
                            existing.MaxResults = p.TryGetProperty("max_results", out var mr) ? mr.GetInt32() : 10;
                            existing.SafeSearch = p.TryGetProperty("safe_search", out var ss) && ss.GetBoolean();
                            existing.IsDefault = p.TryGetProperty("is_default", out var def) && def.GetBoolean();

                            // Update status indicator
                            existing.StatusColor = string.IsNullOrWhiteSpace(existing.ApiKey)
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
        // Web Search Providers (synced with macOS)
        _providers.Add(new SearchProviderItem
        {
            Id = "tavily",
            Name = "Tavily",
            Description = "AI-optimized search API",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 79, 70, 229)), // #4F46E5
            RequiresSearchId = false,
            AllowCustomUrl = false
        });

        _providers.Add(new SearchProviderItem
        {
            Id = "searxng",
            Name = "SearXNG",
            Description = "Self-hosted meta search engine",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 49, 130, 206)), // #3182CE
            RequiresSearchId = false,
            AllowCustomUrl = true
        });

        _providers.Add(new SearchProviderItem
        {
            Id = "google",
            Name = "Google CSE",
            Description = "Google Custom Search Engine",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 66, 133, 244)), // #4285F4
            RequiresSearchId = true,
            AllowCustomUrl = false
        });

        _providers.Add(new SearchProviderItem
        {
            Id = "bing",
            Name = "Bing",
            Description = "Microsoft Bing Web Search",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 0, 131, 115)), // #008373
            RequiresSearchId = false,
            AllowCustomUrl = false
        });

        _providers.Add(new SearchProviderItem
        {
            Id = "brave",
            Name = "Brave",
            Description = "Privacy-focused web search",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 251, 84, 43)), // #FB542B
            RequiresSearchId = false,
            AllowCustomUrl = false
        });

        _providers.Add(new SearchProviderItem
        {
            Id = "exa",
            Name = "Exa",
            Description = "Neural search for AI applications",
            Category = "Web Search",
            Icon = "\uE721",
            IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 139, 92, 246)), // #8B5CF6
            RequiresSearchId = false,
            AllowCustomUrl = false
        });
    }

    private void ProviderListView_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ProviderListView.SelectedItem is SearchProviderItem provider)
        {
            SelectProvider(provider);
        }
        else
        {
            ClearSelection();
        }
    }

    private void SelectProvider(SearchProviderItem provider)
    {
        _isLoading = true;
        _selectedProvider = provider;

        // Update header
        ProviderNameText.Text = provider.Name;
        ProviderDescText.Text = provider.Description;
        ProviderIconBorder.Background = provider.IconBackground;
        ProviderIconGlyph.Glyph = provider.Icon;
        CategoryBadgeText.Text = provider.Category;

        // Show/hide optional fields
        BaseUrlGrid.Visibility = provider.AllowCustomUrl ? Visibility.Visible : Visibility.Collapsed;
        SearchIdGrid.Visibility = provider.RequiresSearchId ? Visibility.Visible : Visibility.Collapsed;

        // Load saved config
        ApiKeyBox.Password = provider.ApiKey ?? "";
        BaseUrlBox.Text = provider.BaseUrl ?? "";
        SearchIdBox.Text = provider.SearchId ?? "";
        MaxResultsBox.Value = provider.MaxResults;
        SafeSearchToggle.IsOn = provider.SafeSearch;
        DefaultProviderToggle.IsOn = provider.IsDefault;

        // Show detail panel
        DetailPanel.Visibility = Visibility.Visible;
        EmptyDetailState.Visibility = Visibility.Collapsed;

        _isLoading = false;
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
        TestResultText.Text = "Test your API connection";
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

    private void SearchIdBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MaxResultsBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void SafeSearchToggle_Toggled(object sender, RoutedEventArgs e)
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

        try
        {
            var hasApiKey = !string.IsNullOrWhiteSpace(ApiKeyBox.Password);
            if (!hasApiKey)
            {
                TestResultText.Text = "API key required";
                TestResultText.Foreground = new SolidColorBrush(Microsoft.UI.Colors.Orange);
                ShowStatus("Please enter an API key", InfoBarSeverity.Warning);
                return;
            }

            // Call AlephCore to test connection
            var result = await Task.Run(() => _core?.TestSearchProvider(_selectedProvider.Id, ApiKeyBox.Password));

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
        _selectedProvider.SearchId = SearchIdBox.Text;
        _selectedProvider.MaxResults = (int)MaxResultsBox.Value;
        _selectedProvider.SafeSearch = SafeSearchToggle.IsOn;

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
        _selectedProvider.StatusColor = string.IsNullOrWhiteSpace(_selectedProvider.ApiKey)
            ? new SolidColorBrush(Microsoft.UI.Colors.Gray)
            : new SolidColorBrush(Microsoft.UI.Colors.Green);

        // Save to AlephCore
        var configJson = JsonSerializer.Serialize(new
        {
            api_key = _selectedProvider.ApiKey,
            base_url = _selectedProvider.BaseUrl,
            search_id = _selectedProvider.SearchId,
            max_results = _selectedProvider.MaxResults,
            safe_search = _selectedProvider.SafeSearch,
            is_default = _selectedProvider.IsDefault
        });

        var success = await Task.Run(() => _core?.UpdateSearchProvider(_selectedProvider.Id, configJson) ?? false);

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

public class SearchProviderItem
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Description { get; set; } = "";
    public string Category { get; set; } = "";
    public string Icon { get; set; } = "\uE721";
    public Brush IconBackground { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public Brush StatusColor { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public string? ApiKey { get; set; }
    public string? BaseUrl { get; set; }
    public string? SearchId { get; set; }
    public int MaxResults { get; set; } = 10;
    public bool SafeSearch { get; set; } = true;
    public bool IsDefault { get; set; } = false;
    public bool RequiresSearchId { get; set; } = false;
    public bool AllowCustomUrl { get; set; } = false;
}
