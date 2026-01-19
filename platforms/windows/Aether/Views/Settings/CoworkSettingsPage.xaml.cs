using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;
using Aether.Interop;
using System.Text.Json;

namespace Aether.Views.Settings;

/// <summary>
/// Cowork Settings page - Task orchestration configuration.
/// Configure parallelism, model routing, timeouts, and error handling.
/// </summary>
public sealed partial class CoworkSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private bool _hasUnsavedChanges;
    private bool _isLoading = true;
    private AetherCore? _core;

    public CoworkSettingsPage()
    {
        InitializeComponent();
        _isLoading = false;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        _core = App.Current?.AetherCore;
        LoadSettings();
    }

    private void LoadSettings()
    {
        _isLoading = true;

        try
        {
            var configJson = _core?.GetCoworkConfig();
            if (!string.IsNullOrEmpty(configJson))
            {
                var config = JsonSerializer.Deserialize<JsonElement>(configJson);

                CoworkEnabledToggle.IsOn = !config.TryGetProperty("enabled", out var en) || en.GetBoolean();
                MaxConcurrentBox.Value = config.TryGetProperty("max_concurrent", out var mc) ? mc.GetInt32() : 4;
                MaxDepthBox.Value = config.TryGetProperty("max_depth", out var md) ? md.GetInt32() : 3;

                // Model routing
                SelectModelByTag(PlanningModelCombo, config.TryGetProperty("planning_model", out var pm) ? pm.GetString() : "claude-3-5-sonnet");
                SelectModelByTag(ExecutionModelCombo, config.TryGetProperty("execution_model", out var em) ? em.GetString() : "auto");
                SelectModelByTag(SynthesisModelCombo, config.TryGetProperty("synthesis_model", out var sm) ? sm.GetString() : "claude-3-5-sonnet");

                // Timeouts
                TaskTimeoutBox.Value = config.TryGetProperty("task_timeout", out var tt) ? tt.GetInt32() : 60;
                TotalTimeoutBox.Value = config.TryGetProperty("total_timeout", out var tot) ? tot.GetInt32() : 300;

                // Error handling
                RetryToggle.IsOn = !config.TryGetProperty("retry_enabled", out var re) || re.GetBoolean();
                MaxRetriesBox.Value = config.TryGetProperty("max_retries", out var mr) ? mr.GetInt32() : 3;
                ContinueOnFailureToggle.IsOn = config.TryGetProperty("continue_on_failure", out var cof) && cof.GetBoolean();
            }
            else
            {
                // Set defaults
                CoworkEnabledToggle.IsOn = true;
                MaxConcurrentBox.Value = 4;
                MaxDepthBox.Value = 3;
                PlanningModelCombo.SelectedIndex = 0;
                ExecutionModelCombo.SelectedIndex = 0;
                SynthesisModelCombo.SelectedIndex = 0;
                TaskTimeoutBox.Value = 60;
                TotalTimeoutBox.Value = 300;
                RetryToggle.IsOn = true;
                MaxRetriesBox.Value = 3;
                ContinueOnFailureToggle.IsOn = false;
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"LoadSettings error: {ex.Message}");
            // Set defaults on error
            CoworkEnabledToggle.IsOn = true;
            MaxConcurrentBox.Value = 4;
            MaxDepthBox.Value = 3;
        }

        UpdateCoworkDependentControls();

        _isLoading = false;
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
    }

    private void SelectModelByTag(ComboBox combo, string? tag)
    {
        if (string.IsNullOrEmpty(tag))
        {
            combo.SelectedIndex = 0;
            return;
        }

        foreach (var item in combo.Items)
        {
            if (item is ComboBoxItem cbi && cbi.Tag?.ToString() == tag)
            {
                combo.SelectedItem = item;
                return;
            }
        }
        combo.SelectedIndex = 0;
    }

    private void CoworkEnabledToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading)
        {
            UpdateCoworkDependentControls();
            MarkAsChanged();
        }
    }

    private void UpdateCoworkDependentControls()
    {
        var isEnabled = CoworkEnabledToggle.IsOn;

        ParallelismCard.Opacity = isEnabled ? 1.0 : 0.5;
        ModelRoutingCard.Opacity = isEnabled ? 1.0 : 0.5;
        TimeoutCard.Opacity = isEnabled ? 1.0 : 0.5;
        ErrorHandlingCard.Opacity = isEnabled ? 1.0 : 0.5;

        MaxConcurrentBox.IsEnabled = isEnabled;
        MaxDepthBox.IsEnabled = isEnabled;
        PlanningModelCombo.IsEnabled = isEnabled;
        ExecutionModelCombo.IsEnabled = isEnabled;
        SynthesisModelCombo.IsEnabled = isEnabled;
        TaskTimeoutBox.IsEnabled = isEnabled;
        TotalTimeoutBox.IsEnabled = isEnabled;
        RetryToggle.IsEnabled = isEnabled;
        MaxRetriesBox.IsEnabled = isEnabled && RetryToggle.IsOn;
        ContinueOnFailureToggle.IsEnabled = isEnabled;
    }

    private void MaxConcurrentBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MaxDepthBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void ModelCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void TaskTimeoutBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void TotalTimeoutBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void RetryToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading)
        {
            MaxRetriesBox.IsEnabled = RetryToggle.IsOn && CoworkEnabledToggle.IsOn;
            MarkAsChanged();
        }
    }

    private void MaxRetriesBox_ValueChanged(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void ContinueOnFailureToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private async void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        // Get selected models
        var planningModel = (PlanningModelCombo.SelectedItem as ComboBoxItem)?.Tag?.ToString() ?? "claude-3-5-sonnet";
        var executionModel = (ExecutionModelCombo.SelectedItem as ComboBoxItem)?.Tag?.ToString() ?? "auto";
        var synthesisModel = (SynthesisModelCombo.SelectedItem as ComboBoxItem)?.Tag?.ToString() ?? "claude-3-5-sonnet";

        var configJson = JsonSerializer.Serialize(new
        {
            enabled = CoworkEnabledToggle.IsOn,
            max_concurrent = (int)MaxConcurrentBox.Value,
            max_depth = (int)MaxDepthBox.Value,
            planning_model = planningModel,
            execution_model = executionModel,
            synthesis_model = synthesisModel,
            task_timeout = (int)TaskTimeoutBox.Value,
            total_timeout = (int)TotalTimeoutBox.Value,
            retry_enabled = RetryToggle.IsOn,
            max_retries = (int)MaxRetriesBox.Value,
            continue_on_failure = ContinueOnFailureToggle.IsOn
        });

        // Save to AetherCore
        var success = await Task.Run(() => _core?.UpdateCoworkConfig(configJson) ?? false);

        if (success)
        {
            _hasUnsavedChanges = false;
            SaveButton.IsEnabled = false;
            ShowStatus("Cowork settings saved", InfoBarSeverity.Success);
        }
        else
        {
            ShowStatus("Failed to save cowork settings", InfoBarSeverity.Error);
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
