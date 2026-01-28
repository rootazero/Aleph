using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;

namespace Aether.Views.Settings;

/// <summary>
/// Memory settings page - History and retention policies.
/// </summary>
public sealed partial class MemorySettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    public MemorySettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        UpdateMemoryStats();
    }

    private void UpdateMemoryStats()
    {
        if (ViewModel != null)
        {
            MemoryUsageText.Text = $"{ViewModel.MemoryUsageMb} MB";
            // TODO: Get actual stats from Rust core
            FactsCountText.Text = "0";
            TopicsCountText.Text = "0";
        }
    }

    private async void CompactNowButton_Click(object sender, RoutedEventArgs e)
    {
        CompactNowButton.IsEnabled = false;
        CompactNowButton.Content = "Compacting...";

        try
        {
            // TODO: Call Rust core to compact
            await Task.Delay(2000); // Simulate compaction

            CompactNowButton.Content = "Done!";
            await Task.Delay(1000);
        }
        finally
        {
            CompactNowButton.Content = "Compact Now";
            CompactNowButton.IsEnabled = true;
            UpdateMemoryStats();
        }
    }

    private async void ClearHistoryButton_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new ContentDialog
        {
            Title = "Clear All History",
            Content = "Are you sure you want to clear all conversation history? This action cannot be undone.",
            PrimaryButtonText = "Clear",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = this.XamlRoot
        };

        var result = await dialog.ShowAsync();

        if (result == ContentDialogResult.Primary)
        {
            ViewModel?.ClearMemoryCommand.Execute(null);
            UpdateMemoryStats();
        }
    }

    private void ViewFactsButton_Click(object sender, RoutedEventArgs e)
    {
        // TODO: Open facts browser window
        System.Diagnostics.Debug.WriteLine("[Memory] View facts requested");
    }
}
