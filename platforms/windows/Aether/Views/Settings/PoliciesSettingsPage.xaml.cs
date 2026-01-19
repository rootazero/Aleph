using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;
using Aether.Interop;
using System.Text.Json;

namespace Aether.Views.Settings;

/// <summary>
/// Policies Settings page - Read-only view of system policies.
/// Displays content policies, data policies, API policies, and tool policies.
/// </summary>
public sealed partial class PoliciesSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;
    private AetherCore? _core;

    public PoliciesSettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        _core = App.Instance.Core;
        LoadPolicies();
    }

    private void LoadPolicies()
    {
        try
        {
            var policiesJson = _core?.GetPolicies();
            if (!string.IsNullOrEmpty(policiesJson))
            {
                var policies = JsonSerializer.Deserialize<JsonElement>(policiesJson);

                // Content Policies
                if (policies.TryGetProperty("content", out var content))
                {
                    ContentFilterValue.Text = content.TryGetProperty("filter_level", out var fl) ? fl.GetString() : "Standard";
                    SafeModeValue.Text = content.TryGetProperty("safe_mode", out var sm) && sm.GetBoolean() ? "Enabled" : "Disabled";
                    ExplicitContentValue.Text = content.TryGetProperty("explicit_content", out var ec) && ec.GetBoolean() ? "Allowed" : "Disabled";
                }

                // Data Policies
                if (policies.TryGetProperty("data", out var data))
                {
                    DataRetentionValue.Text = data.TryGetProperty("retention_days", out var rd) ? $"{rd.GetInt32()} days" : "30 days";
                    LocalStorageValue.Text = data.TryGetProperty("local_storage", out var ls) && ls.GetBoolean() ? "Enforced" : "Optional";
                    PiiAutoDeleteValue.Text = data.TryGetProperty("pii_auto_delete", out var pad) && pad.GetBoolean() ? "Enabled" : "Disabled";
                }

                // API Policies
                if (policies.TryGetProperty("api", out var api))
                {
                    RateLimitValue.Text = api.TryGetProperty("rate_limit", out var rl) ? $"{rl.GetInt32()} req/min" : "60 req/min";
                    CostLimitValue.Text = api.TryGetProperty("cost_limit", out var cl) ? $"${cl.GetDecimal()}/month" : "No limit";
                    AllowedProvidersValue.Text = api.TryGetProperty("allowed_providers", out var ap) ? ap.GetString() : "All";
                }

                // Tool Policies
                if (policies.TryGetProperty("tools", out var tools))
                {
                    CodeExecutionValue.Text = tools.TryGetProperty("code_execution", out var ce) ? ce.GetString() : "Sandboxed";
                    FileAccessValue.Text = tools.TryGetProperty("file_access", out var fa) ? fa.GetString() : "Read Only";
                    NetworkAccessValue.Text = tools.TryGetProperty("network_access", out var na) && na.GetBoolean() ? "Allowed" : "Denied";
                    McpInstallValue.Text = tools.TryGetProperty("mcp_install", out var mi) && mi.GetBoolean() ? "Allowed" : "Denied";
                }

                // Policy Source
                if (policies.TryGetProperty("source", out var source))
                {
                    PolicySourceText.Text = source.TryGetProperty("path", out var path) ? path.GetString() : "Local configuration";
                    PolicyLastUpdatedText.Text = source.TryGetProperty("last_updated", out var lu) ? $"Last updated: {lu.GetString()}" : $"Last updated: {DateTime.Now:yyyy-MM-dd HH:mm}";
                }
            }
            else
            {
                LoadDefaultPolicies();
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"LoadPolicies error: {ex.Message}");
            LoadDefaultPolicies();
        }
    }

    private void LoadDefaultPolicies()
    {
        // Content Policies
        ContentFilterValue.Text = "Standard";
        SafeModeValue.Text = "Enabled";
        ExplicitContentValue.Text = "Disabled";

        // Data Policies
        DataRetentionValue.Text = "30 days";
        LocalStorageValue.Text = "Enforced";
        PiiAutoDeleteValue.Text = "Enabled";

        // API Policies
        RateLimitValue.Text = "60 req/min";
        CostLimitValue.Text = "No limit";
        AllowedProvidersValue.Text = "All";

        // Tool Policies
        CodeExecutionValue.Text = "Sandboxed";
        FileAccessValue.Text = "Read Only";
        NetworkAccessValue.Text = "Allowed";
        McpInstallValue.Text = "Allowed";

        // Policy Source
        var configPath = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
            ".aether", "config.toml"
        );
        PolicySourceText.Text = $"Local configuration file ({configPath})";
        PolicyLastUpdatedText.Text = $"Last updated: {DateTime.Now:yyyy-MM-dd HH:mm}";
    }

    private async void RefreshButton_Click(object sender, RoutedEventArgs e)
    {
        RefreshButton.IsEnabled = false;

        try
        {
            // Refresh from AetherCore
            await Task.Run(() => _core?.ReloadConfig());
            LoadPolicies();
            ShowStatus("Policies refreshed", InfoBarSeverity.Success);
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to refresh policies: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            RefreshButton.IsEnabled = true;
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
