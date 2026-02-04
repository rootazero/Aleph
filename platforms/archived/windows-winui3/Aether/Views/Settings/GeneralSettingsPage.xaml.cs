using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aleph.ViewModels;

namespace Aleph.Views.Settings;

/// <summary>
/// General settings page - Sound, startup, language, updates, logs, about.
/// </summary>
public sealed partial class GeneralSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    public GeneralSettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        Bindings.Update();
    }

    private void CheckUpdatesButton_Click(object sender, RoutedEventArgs e)
    {
        ViewModel?.CheckForUpdatesCommand.Execute(null);
    }

    private void ViewLogsButton_Click(object sender, RoutedEventArgs e)
    {
        ViewModel?.ViewLogsCommand.Execute(null);
    }

    private void OpenConfigButton_Click(object sender, RoutedEventArgs e)
    {
        ViewModel?.OpenConfigFolderCommand.Execute(null);
    }
}
