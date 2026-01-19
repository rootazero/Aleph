using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Graphics;
using Aether.ViewModels;
using Aether.Views.Settings;

namespace Aether.Windows;

/// <summary>
/// Settings window with NavigationView sidebar.
///
/// Features:
/// - 13 settings tabs matching macOS for feature parity
/// - Save/Cancel bar for unsaved changes
/// - MVVM pattern with SettingsViewModel
/// </summary>
public sealed partial class SettingsWindow : Window
{
    private readonly SettingsViewModel _viewModel;

    // Page instances (cached for performance)
    private GeneralSettingsPage? _generalPage;
    private ProvidersSettingsPage? _providersPage;
    private ShortcutsSettingsPage? _shortcutsPage;
    private MemorySettingsPage? _memoryPage;
    private PlaceholderSettingsPage? _placeholderPage;

    public SettingsWindow()
    {
        InitializeComponent();
        Title = "Aether Settings";

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(900, 700));

        // Initialize ViewModel
        _viewModel = new SettingsViewModel();
        _viewModel.PropertyChanged += ViewModel_PropertyChanged;

        // Select first item
        if (SettingsNav.MenuItems.Count > 0)
        {
            SettingsNav.SelectedItem = SettingsNav.MenuItems[0];
        }
    }

    private void ViewModel_PropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(SettingsViewModel.HasUnsavedChanges))
        {
            SaveBar.Visibility = _viewModel.HasUnsavedChanges ? Visibility.Visible : Visibility.Collapsed;
        }
        else if (e.PropertyName == nameof(SettingsViewModel.StatusMessage))
        {
            if (!string.IsNullOrEmpty(_viewModel.StatusMessage))
            {
                StatusMessageText.Text = _viewModel.StatusMessage;
            }
            else
            {
                StatusMessageText.Text = "You have unsaved changes";
            }
        }
    }

    private void SettingsNav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is NavigationViewItem item && item.Tag is string tag)
        {
            NavigateToPage(tag);
        }
    }

    private void NavigateToPage(string tag)
    {
        var tab = Enum.Parse<SettingsTab>(tag);
        _viewModel.NavigateToCommand.Execute(tab);

        // Navigate to appropriate page
        switch (tab)
        {
            case SettingsTab.General:
                _generalPage ??= new GeneralSettingsPage();
                _generalPage.SetViewModel(_viewModel);
                ContentFrame.Content = _generalPage;
                break;

            case SettingsTab.Providers:
                _providersPage ??= new ProvidersSettingsPage();
                _providersPage.SetViewModel(_viewModel);
                ContentFrame.Content = _providersPage;
                break;

            case SettingsTab.Shortcuts:
                _shortcutsPage ??= new ShortcutsSettingsPage();
                _shortcutsPage.SetViewModel(_viewModel);
                ContentFrame.Content = _shortcutsPage;
                break;

            case SettingsTab.Memory:
                _memoryPage ??= new MemorySettingsPage();
                _memoryPage.SetViewModel(_viewModel);
                ContentFrame.Content = _memoryPage;
                break;

            // Placeholder pages for tabs not yet implemented
            case SettingsTab.Generation:
            case SettingsTab.Routing:
            case SettingsTab.Behavior:
            case SettingsTab.Search:
            case SettingsTab.Mcp:
            case SettingsTab.Skills:
            case SettingsTab.Cowork:
            case SettingsTab.Policies:
            case SettingsTab.Runtimes:
                _placeholderPage ??= new PlaceholderSettingsPage();
                _placeholderPage.Configure(tab);
                ContentFrame.Content = _placeholderPage;
                break;
        }
    }

    private async void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        await _viewModel.SaveSettingsAsync();
    }

    private void CancelButton_Click(object sender, RoutedEventArgs e)
    {
        _viewModel.CancelChangesCommand.Execute(null);
    }
}
