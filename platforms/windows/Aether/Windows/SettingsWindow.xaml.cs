using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Graphics;
using Windows.ApplicationModel.Resources;
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
    private McpSettingsPage? _mcpPage;
    private SkillsSettingsPage? _skillsPage;
    private GenerationSettingsPage? _generationPage;
    private RoutingSettingsPage? _routingPage;
    private BehaviorSettingsPage? _behaviorPage;
    private SearchSettingsPage? _searchPage;
    private CoworkSettingsPage? _coworkPage;
    private PoliciesSettingsPage? _policiesPage;
    private RuntimesSettingsPage? _runtimesPage;

    public SettingsWindow()
    {
        InitializeComponent();

        // Set localized window title
        try
        {
            var resourceLoader = new ResourceLoader();
            Title = resourceLoader.GetString("SettingsWindowTitle");
            if (string.IsNullOrEmpty(Title))
                Title = "Aether Settings";
        }
        catch
        {
            Title = "Aether Settings";
        }

        // Set window size
        var presenter = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(
            Microsoft.UI.Win32Interop.GetWindowIdFromWindow(
                WinRT.Interop.WindowNative.GetWindowHandle(this)));
        presenter.Resize(new SizeInt32(800, 700));

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

            case SettingsTab.Mcp:
                _mcpPage ??= new McpSettingsPage();
                _mcpPage.SetViewModel(_viewModel);
                ContentFrame.Content = _mcpPage;
                break;

            case SettingsTab.Skills:
                _skillsPage ??= new SkillsSettingsPage();
                _skillsPage.SetViewModel(_viewModel);
                ContentFrame.Content = _skillsPage;
                break;

            case SettingsTab.Generation:
                _generationPage ??= new GenerationSettingsPage();
                _generationPage.SetViewModel(_viewModel);
                ContentFrame.Content = _generationPage;
                break;

            case SettingsTab.Routing:
                _routingPage ??= new RoutingSettingsPage();
                _routingPage.SetViewModel(_viewModel);
                ContentFrame.Content = _routingPage;
                break;

            case SettingsTab.Behavior:
                _behaviorPage ??= new BehaviorSettingsPage();
                _behaviorPage.SetViewModel(_viewModel);
                ContentFrame.Content = _behaviorPage;
                break;

            case SettingsTab.Search:
                _searchPage ??= new SearchSettingsPage();
                _searchPage.SetViewModel(_viewModel);
                ContentFrame.Content = _searchPage;
                break;

            case SettingsTab.Cowork:
                _coworkPage ??= new CoworkSettingsPage();
                _coworkPage.SetViewModel(_viewModel);
                ContentFrame.Content = _coworkPage;
                break;

            case SettingsTab.Policies:
                _policiesPage ??= new PoliciesSettingsPage();
                _policiesPage.SetViewModel(_viewModel);
                ContentFrame.Content = _policiesPage;
                break;

            case SettingsTab.Runtimes:
                _runtimesPage ??= new RuntimesSettingsPage();
                _runtimesPage.SetViewModel(_viewModel);
                ContentFrame.Content = _runtimesPage;
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
