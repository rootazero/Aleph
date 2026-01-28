using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;
using System.Collections.ObjectModel;

namespace Aether.Views.Settings;

/// <summary>
/// Routing Settings page - Model routing rules configuration.
/// Configure cost strategy, default model, and task type routing.
/// </summary>
public sealed partial class RoutingSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<RoutingRuleItem> _routingRules = new();
    private bool _hasUnsavedChanges;
    private bool _isLoading = true;

    public RoutingSettingsPage()
    {
        InitializeComponent();
        InitializeRoutingRules();
        RoutingRulesControl.ItemsSource = _routingRules;
        _isLoading = false;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        LoadSettings();
    }

    private void InitializeRoutingRules()
    {
        _routingRules.Add(new RoutingRuleItem { TaskType = "code_generation", TaskName = "Code Generation", TaskDescription = "Writing new code", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "code_review", TaskName = "Code Review", TaskDescription = "Reviewing and analyzing code", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "image_analysis", TaskName = "Image Analysis", TaskDescription = "Understanding images", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "video_understanding", TaskName = "Video Understanding", TaskDescription = "Processing video content", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "long_document", TaskName = "Long Document", TaskDescription = "Processing large documents", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "quick_tasks", TaskName = "Quick Tasks", TaskDescription = "Simple, fast queries", SelectedModel = "claude-3-haiku" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "privacy_sensitive", TaskName = "Privacy Sensitive", TaskDescription = "Handling sensitive data", SelectedModel = "default" });
        _routingRules.Add(new RoutingRuleItem { TaskType = "reasoning", TaskName = "Reasoning", TaskDescription = "Complex logical reasoning", SelectedModel = "claude-3-opus" });
    }

    private void LoadSettings()
    {
        _isLoading = true;

        // TODO: Load from AetherCore
        // Set default values
        CostStrategyPicker.SelectedIndex = 1; // Balanced
        DefaultModelComboBox.SelectedIndex = 0; // Claude 3.5 Sonnet
        PipelineToggle.IsOn = true;

        _isLoading = false;
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
    }

    private void CostStrategyPicker_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void DefaultModelComboBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void PipelineToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void RoutingRule_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        // Get selected strategy
        var selectedStrategy = "balanced";
        if (CostStrategyPicker.SelectedItem is RadioButton rb && rb.Tag is string tag)
        {
            selectedStrategy = tag.ToLowerInvariant();
        }

        // Get default model
        var defaultModel = "claude-3-5-sonnet";
        if (DefaultModelComboBox.SelectedItem is ComboBoxItem item && item.Tag is string modelTag)
        {
            defaultModel = modelTag;
        }

        // TODO: Save to AetherCore
        System.Diagnostics.Debug.WriteLine($"Saving routing config: strategy={selectedStrategy}, default={defaultModel}, pipeline={PipelineToggle.IsOn}");

        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;

        ShowStatus("Routing configuration saved", InfoBarSeverity.Success);
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

public class RoutingRuleItem
{
    public string TaskType { get; set; } = "";
    public string TaskName { get; set; } = "";
    public string TaskDescription { get; set; } = "";
    public string SelectedModel { get; set; } = "default";
}
