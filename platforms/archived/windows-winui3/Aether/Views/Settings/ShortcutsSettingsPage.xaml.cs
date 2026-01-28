using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;

namespace Aether.Views.Settings;

/// <summary>
/// Shortcuts settings page - Hotkey configuration.
/// </summary>
public sealed partial class ShortcutsSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    public ShortcutsSettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        UpdateHotkeyDisplays();
    }

    private void UpdateHotkeyDisplays()
    {
        if (ViewModel != null)
        {
            HaloHotkeyText.Text = ViewModel.HaloHotkey;
            ConversationHotkeyText.Text = ViewModel.ConversationHotkey;
        }
    }
}
