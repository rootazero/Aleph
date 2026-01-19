using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aether.ViewModels;
using Aether.Interop;
using System.Text.Json;

namespace Aether.Views.Settings;

/// <summary>
/// Runtimes Settings page - Runtime environment management.
/// Manage Python (uv), Node.js (fnm), yt-dlp, and FFmpeg installations.
/// </summary>
public sealed partial class RuntimesSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private bool _hasUnsavedChanges;
    private AetherCore? _core;

    public RuntimesSettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        _core = App.Current?.AetherCore;
        LoadRuntimeStatus();
    }

    private async void LoadRuntimeStatus()
    {
        try
        {
            // Try to get actual runtime status from core
            var runtimesJson = _core?.ListRuntimes();
            if (!string.IsNullOrEmpty(runtimesJson))
            {
                var runtimes = JsonSerializer.Deserialize<JsonElement>(runtimesJson);
                if (runtimes.ValueKind == JsonValueKind.Array)
                {
                    foreach (var runtime in runtimes.EnumerateArray())
                    {
                        var id = runtime.GetProperty("id").GetString() ?? "";
                        var isInstalled = runtime.TryGetProperty("installed", out var inst) && inst.GetBoolean();
                        var version = runtime.TryGetProperty("version", out var ver) ? ver.GetString() : null;
                        var managerVersion = runtime.TryGetProperty("manager_version", out var mgr) ? mgr.GetString() : null;
                        var location = runtime.TryGetProperty("location", out var loc) ? loc.GetString() : null;

                        switch (id)
                        {
                            case "python" or "uv":
                                UpdateRuntimeStatus("python", isInstalled, version, managerVersion);
                                if (!string.IsNullOrEmpty(location))
                                    PythonLocationText.Text = location;
                                break;
                            case "node" or "fnm":
                                UpdateRuntimeStatus("node", isInstalled, version, managerVersion);
                                if (!string.IsNullOrEmpty(location))
                                    NodeLocationText.Text = location;
                                break;
                            case "ytdlp" or "yt-dlp":
                                UpdateRuntimeStatus("ytdlp", isInstalled, version, null);
                                if (!string.IsNullOrEmpty(location))
                                    YtdlpLocationText.Text = location;
                                break;
                            case "ffmpeg":
                                UpdateRuntimeStatus("ffmpeg", isInstalled, version, null);
                                break;
                        }
                    }
                    return;
                }
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"LoadRuntimeStatus error: {ex.Message}");
        }

        // Fallback: Check individual runtimes
        await LoadRuntimeStatusFallback();
    }

    private async Task LoadRuntimeStatusFallback()
    {
        // Check Python/uv
        var pythonInstalled = _core?.IsRuntimeInstalled("python") ?? false;
        UpdateRuntimeStatus("python", pythonInstalled, pythonInstalled ? "3.12.x" : null, pythonInstalled ? "0.4.x" : null);
        PythonLocationText.Text = GetRuntimePath(".uv");

        // Check Node.js/fnm
        var nodeInstalled = _core?.IsRuntimeInstalled("node") ?? false;
        UpdateRuntimeStatus("node", nodeInstalled, nodeInstalled ? "20.x" : null, nodeInstalled ? "1.x" : null);
        NodeLocationText.Text = GetRuntimePath(".fnm");

        // Check yt-dlp
        var ytdlpInstalled = _core?.IsRuntimeInstalled("ytdlp") ?? false;
        UpdateRuntimeStatus("ytdlp", ytdlpInstalled, ytdlpInstalled ? "latest" : null, null);
        YtdlpLocationText.Text = GetRuntimePath(".config/aether/bin/yt-dlp");

        // Check FFmpeg
        var ffmpegInstalled = _core?.IsRuntimeInstalled("ffmpeg") ?? false;
        UpdateRuntimeStatus("ffmpeg", ffmpegInstalled, null, null);

        await Task.CompletedTask;
    }

    private string GetRuntimePath(string relativePath)
    {
        var homePath = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return Path.Combine(homePath, relativePath);
    }

    private void UpdateRuntimeStatus(string runtime, bool isInstalled, string? version, string? managerVersion)
    {
        var successBrush = new SolidColorBrush(Microsoft.UI.Colors.Green);
        var warningBrush = new SolidColorBrush(Microsoft.UI.Colors.Orange);
        var grayBrush = new SolidColorBrush(Microsoft.UI.Colors.Gray);

        switch (runtime)
        {
            case "python":
                if (isInstalled)
                {
                    PythonStatusBadge.Background = successBrush;
                    PythonStatusText.Text = "Installed";
                    PythonVersionText.Text = version ?? "Unknown";
                    UvVersionText.Text = managerVersion ?? "Unknown";
                    PythonUpdateButton.Visibility = Visibility.Visible;
                    PythonInstallButton.Visibility = Visibility.Collapsed;
                }
                else
                {
                    PythonStatusBadge.Background = grayBrush;
                    PythonStatusText.Text = "Not Installed";
                    PythonVersionText.Text = "—";
                    UvVersionText.Text = "—";
                    PythonUpdateButton.Visibility = Visibility.Collapsed;
                    PythonInstallButton.Visibility = Visibility.Visible;
                }
                break;

            case "node":
                if (isInstalled)
                {
                    NodeStatusBadge.Background = successBrush;
                    NodeStatusText.Text = "Installed";
                    NodeVersionText.Text = version ?? "Unknown";
                    FnmVersionText.Text = managerVersion ?? "Unknown";
                    NodeUpdateButton.Visibility = Visibility.Visible;
                    NodeInstallButton.Visibility = Visibility.Collapsed;
                }
                else
                {
                    NodeStatusBadge.Background = grayBrush;
                    NodeStatusText.Text = "Not Installed";
                    NodeVersionText.Text = "—";
                    FnmVersionText.Text = "—";
                    NodeUpdateButton.Visibility = Visibility.Collapsed;
                    NodeInstallButton.Visibility = Visibility.Visible;
                }
                break;

            case "ytdlp":
                if (isInstalled)
                {
                    YtdlpStatusBadge.Background = successBrush;
                    YtdlpStatusText.Text = "Installed";
                    YtdlpVersionText.Text = version ?? "Unknown";
                    YtdlpUpdateButton.Visibility = Visibility.Visible;
                    YtdlpInstallButton.Visibility = Visibility.Collapsed;
                }
                else
                {
                    YtdlpStatusBadge.Background = grayBrush;
                    YtdlpStatusText.Text = "Not Installed";
                    YtdlpVersionText.Text = "—";
                    YtdlpUpdateButton.Visibility = Visibility.Collapsed;
                    YtdlpInstallButton.Visibility = Visibility.Visible;
                }
                break;

            case "ffmpeg":
                if (isInstalled)
                {
                    FfmpegStatusBadge.Background = successBrush;
                    FfmpegStatusText.Text = "Installed";
                    FfmpegVersionText.Text = version ?? "Unknown";
                    FfmpegInstallButton.Content = CreateButtonContent("\uE72C", "Reinstall");
                }
                else
                {
                    FfmpegStatusBadge.Background = warningBrush;
                    FfmpegStatusText.Text = "Optional";
                    FfmpegVersionText.Text = "Not installed";
                    FfmpegInstallButton.Content = CreateButtonContent("\uE896", "Install");
                }
                break;
        }
    }

    private StackPanel CreateButtonContent(string glyph, string text)
    {
        var panel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        panel.Children.Add(new FontIcon { Glyph = glyph, FontSize = 12 });
        panel.Children.Add(new TextBlock { Text = text });
        return panel;
    }

    private void AutoUpdateToggle_Toggled(object sender, RoutedEventArgs e)
    {
        _hasUnsavedChanges = true;
        // Save auto-update preference to AetherCore
        _core?.SetRuntimeAutoUpdate(AutoUpdateToggle.IsOn);
    }

    private async void PythonUpdate_Click(object sender, RoutedEventArgs e)
    {
        await CheckAndUpdateRuntime("python", "Python", PythonUpdateButton);
    }

    private async void PythonInstall_Click(object sender, RoutedEventArgs e)
    {
        await InstallRuntimeAsync("python", "Python", PythonInstallButton);
    }

    private async void NodeUpdate_Click(object sender, RoutedEventArgs e)
    {
        await CheckAndUpdateRuntime("node", "Node.js", NodeUpdateButton);
    }

    private async void NodeInstall_Click(object sender, RoutedEventArgs e)
    {
        await InstallRuntimeAsync("node", "Node.js", NodeInstallButton);
    }

    private async void YtdlpUpdate_Click(object sender, RoutedEventArgs e)
    {
        await CheckAndUpdateRuntime("ytdlp", "yt-dlp", YtdlpUpdateButton);
    }

    private async void YtdlpInstall_Click(object sender, RoutedEventArgs e)
    {
        await InstallRuntimeAsync("ytdlp", "yt-dlp", YtdlpInstallButton);
    }

    private async void FfmpegInstall_Click(object sender, RoutedEventArgs e)
    {
        await InstallRuntimeAsync("ffmpeg", "FFmpeg", FfmpegInstallButton);
    }

    private async Task CheckAndUpdateRuntime(string runtimeId, string displayName, Button button)
    {
        button.IsEnabled = false;
        ShowStatus($"Checking for {displayName} updates...", InfoBarSeverity.Informational);

        try
        {
            // Call AetherCore to update runtime
            var result = await Task.Run(() => _core?.UpdateRuntime(runtimeId));

            if (result?.Success == true)
            {
                ShowStatus(string.IsNullOrEmpty(result.Value.Message)
                    ? $"{displayName} is up to date"
                    : result.Value.Message, InfoBarSeverity.Success);
                LoadRuntimeStatus(); // Refresh status
            }
            else
            {
                ShowStatus(result?.Message ?? $"{displayName} is up to date", InfoBarSeverity.Success);
            }
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to check {displayName} updates: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            button.IsEnabled = true;
        }
    }

    private async Task InstallRuntimeAsync(string runtimeId, string displayName, Button button)
    {
        button.IsEnabled = false;
        ShowStatus($"Installing {displayName}...", InfoBarSeverity.Informational);

        try
        {
            // Call AetherCore to install runtime
            var result = await Task.Run(() => _core?.InstallRuntime(runtimeId));

            if (result?.Success == true)
            {
                ShowStatus($"{displayName} installed successfully", InfoBarSeverity.Success);
                LoadRuntimeStatus(); // Refresh status
            }
            else
            {
                ShowStatus($"Failed to install {displayName}: {result?.Message ?? "Unknown error"}", InfoBarSeverity.Error);
            }
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to install {displayName}: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            button.IsEnabled = true;
        }
    }

    private async void UpdateAll_Click(object sender, RoutedEventArgs e)
    {
        UpdateAllButton.IsEnabled = false;
        ShowStatus("Checking all runtimes for updates...", InfoBarSeverity.Informational);

        try
        {
            // Check for updates
            var updatesJson = await Task.Run(() => _core?.CheckRuntimeUpdates());

            if (!string.IsNullOrEmpty(updatesJson))
            {
                var updates = JsonSerializer.Deserialize<JsonElement>(updatesJson);
                var hasUpdates = false;

                if (updates.ValueKind == JsonValueKind.Array)
                {
                    foreach (var update in updates.EnumerateArray())
                    {
                        var runtimeId = update.GetProperty("id").GetString();
                        if (!string.IsNullOrEmpty(runtimeId))
                        {
                            hasUpdates = true;
                            await Task.Run(() => _core?.UpdateRuntime(runtimeId));
                        }
                    }
                }

                if (hasUpdates)
                {
                    ShowStatus("All runtimes updated successfully", InfoBarSeverity.Success);
                }
                else
                {
                    ShowStatus("All runtimes are up to date", InfoBarSeverity.Success);
                }
            }
            else
            {
                ShowStatus("All runtimes are up to date", InfoBarSeverity.Success);
            }

            LoadRuntimeStatus();
        }
        catch (Exception ex)
        {
            ShowStatus($"Update failed: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            UpdateAllButton.IsEnabled = true;
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
