using CommunityToolkit.Mvvm.Input;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Aether.Services;

/// <summary>
/// System tray icon service.
///
/// Provides:
/// - Tray icon with Aether branding
/// - Right-click context menu
/// - Double-click to show settings
/// </summary>
public sealed class TrayIconService : IDisposable
{
    private TaskbarIcon? _trayIcon;
    private bool _disposed;

    public event Action? SettingsRequested;
    public event Action? QuitRequested;

    public TrayIconService()
    {
        // Tray icon is created when Show() is called
    }

    public void Show()
    {
        if (_trayIcon != null) return;

        _trayIcon = new TaskbarIcon
        {
            ToolTipText = "Aether - AI Assistant",
            ContextMenuMode = ContextMenuMode.SecondWindow,
        };

        // Create context menu
        var contextMenu = new MenuFlyout();

        var settingsItem = new MenuFlyoutItem { Text = "Settings" };
        settingsItem.Click += (s, e) => SettingsRequested?.Invoke();
        contextMenu.Items.Add(settingsItem);

        contextMenu.Items.Add(new MenuFlyoutSeparator());

        var aboutItem = new MenuFlyoutItem { Text = "About Aether" };
        aboutItem.Click += (s, e) => ShowAbout();
        contextMenu.Items.Add(aboutItem);

        contextMenu.Items.Add(new MenuFlyoutSeparator());

        var quitItem = new MenuFlyoutItem { Text = "Quit" };
        quitItem.Click += (s, e) => QuitRequested?.Invoke();
        contextMenu.Items.Add(quitItem);

        _trayIcon.ContextFlyout = contextMenu;

        // Double-click opens settings (using command)
        _trayIcon.DoubleClickCommand = new RelayCommand(() => SettingsRequested?.Invoke());

        // Set icon
        SetDefaultIcon();

        // CRITICAL: Force create the tray icon to make it visible
        // H.NotifyIcon.WinUI requires this for programmatic creation
        try
        {
            _trayIcon.ForceCreate(enablesEfficiencyMode: false);
            System.Diagnostics.Debug.WriteLine("[TrayIcon] ForceCreate called successfully");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[TrayIcon] ForceCreate failed: {ex.Message}");
        }
    }

    public void Hide()
    {
        _trayIcon?.Dispose();
        _trayIcon = null;
    }

    public void SetStatus(TrayStatus status)
    {
        if (_trayIcon == null) return;

        _trayIcon.ToolTipText = status switch
        {
            TrayStatus.Idle => "Aether - Ready",
            TrayStatus.Processing => "Aether - Processing...",
            TrayStatus.Error => "Aether - Error occurred",
            _ => "Aether"
        };

        // Could also change icon based on status
    }

    private void SetDefaultIcon()
    {
        try
        {
            // Use H.NotifyIcon's GeneratedIconSource to create a text-based icon
            // This is the recommended approach for WinUI 3
            _trayIcon!.IconSource = new H.NotifyIcon.GeneratedIconSource
            {
                Text = "A",  // "A" for Aether
                Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(
                    Microsoft.UI.Colors.White),
                Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(
                    Microsoft.UI.ColorHelper.FromArgb(255, 0, 120, 212)),  // Windows accent blue
                FontSize = 20,
            };
            System.Diagnostics.Debug.WriteLine("[TrayIcon] Generated icon set successfully");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[TrayIcon] Failed to set icon: {ex.Message}");
        }
    }

    private void ShowAbout()
    {
        // Show about dialog
        // For now, just log
        System.Diagnostics.Debug.WriteLine("About Aether requested");
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _trayIcon?.Dispose();
        _trayIcon = null;
    }
}

public enum TrayStatus
{
    Idle,
    Processing,
    Error
}
