using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aleph.ViewModels;
using System.Collections.ObjectModel;
using System.Runtime.InteropServices;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace Aleph.Views.Settings;

/// <summary>
/// MCP Settings page - Model Context Protocol server configuration.
/// Features Master-Detail layout matching macOS implementation.
/// </summary>
public sealed partial class McpSettingsPage : UserControl
{
    [DllImport("user32.dll")]
    private static extern IntPtr GetActiveWindow();

    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<McpServerItem> _servers = new();
    private McpServerItem? _selectedServer;
    private McpServerItem? _originalServer;
    private bool _hasUnsavedChanges;

    // Arguments and environment variables
    private ObservableCollection<StringWrapper> _args = new();
    private ObservableCollection<EnvVarItem> _envVars = new();

    public McpSettingsPage()
    {
        InitializeComponent();
        ServerListView.ItemsSource = _servers;
        ArgsItemsControl.ItemsSource = _args;
        EnvVarsItemsControl.ItemsSource = _envVars;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        LoadServers();
    }

    #region Data Loading

    private void LoadServers()
    {
        _servers.Clear();

        // TODO: Load from AlephCore when integrated
        // For now, show empty state
        UpdateEmptyState();
    }

    private void UpdateEmptyState()
    {
        EmptyState.Visibility = _servers.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        ServerListView.Visibility = _servers.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    #endregion

    #region Server Selection

    private void ServerListView_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ServerListView.SelectedItem is McpServerItem server)
        {
            SelectServer(server);
        }
        else
        {
            ClearSelection();
        }
    }

    private void SelectServer(McpServerItem server)
    {
        _selectedServer = server.Clone();
        _originalServer = server.Clone();

        // Update UI
        ServerNameText.Text = server.Name;
        ServerTriggerText.Text = server.TriggerCommand ?? $"/mcp/{server.Id}";
        EnabledToggle.IsOn = server.Enabled;
        CommandBox.Text = server.Command ?? "";
        WorkingDirBox.Text = server.WorkingDirectory ?? "";
        RequiresConfirmationToggle.IsOn = server.RequiresConfirmation;

        // Load arguments
        _args.Clear();
        foreach (var arg in server.Args)
        {
            _args.Add(new StringWrapper { Value = arg });
        }

        // Load environment variables
        _envVars.Clear();
        foreach (var env in server.EnvVars)
        {
            _envVars.Add(new EnvVarItem { Key = env.Key, Value = env.Value });
        }

        // Update status
        UpdateStatusIndicator(server.Status);

        // Show detail panel
        DetailPanel.Visibility = Visibility.Visible;
        EmptyDetailState.Visibility = Visibility.Collapsed;
        DeleteButton.IsEnabled = true;

        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
    }

    private void ClearSelection()
    {
        _selectedServer = null;
        _originalServer = null;
        DetailPanel.Visibility = Visibility.Collapsed;
        EmptyDetailState.Visibility = Visibility.Visible;
        DeleteButton.IsEnabled = false;
    }

    private void UpdateStatusIndicator(McpServerStatus status)
    {
        switch (status)
        {
            case McpServerStatus.Running:
                StatusDot.Fill = new SolidColorBrush(Microsoft.UI.Colors.Green);
                StatusText.Text = "Running";
                StatusBadge.Background = (Brush)Application.Current.Resources["SystemFillColorSuccessBackgroundBrush"];
                break;
            case McpServerStatus.Stopped:
                StatusDot.Fill = new SolidColorBrush(Microsoft.UI.Colors.Gray);
                StatusText.Text = "Stopped";
                StatusBadge.Background = (Brush)Application.Current.Resources["SubtleFillColorSecondaryBrush"];
                break;
            case McpServerStatus.Starting:
                StatusDot.Fill = new SolidColorBrush(Microsoft.UI.Colors.Orange);
                StatusText.Text = "Starting";
                StatusBadge.Background = (Brush)Application.Current.Resources["SystemFillColorCautionBackgroundBrush"];
                break;
            case McpServerStatus.Error:
                StatusDot.Fill = new SolidColorBrush(Microsoft.UI.Colors.Red);
                StatusText.Text = "Error";
                StatusBadge.Background = (Brush)Application.Current.Resources["SystemFillColorCriticalBackgroundBrush"];
                break;
        }
    }

    #endregion

    #region Add Server

    private async void AddServer_Click(object sender, RoutedEventArgs e)
    {
        NewServerNameBox.Text = "";
        NewServerCommandBox.Text = "";
        NewServerArgsBox.Text = "";

        AddServerDialog.XamlRoot = this.XamlRoot;
        await AddServerDialog.ShowAsync();
    }

    private void AddServerDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        var name = NewServerNameBox.Text.Trim();
        var command = NewServerCommandBox.Text.Trim();
        var argsText = NewServerArgsBox.Text.Trim();

        if (string.IsNullOrEmpty(name) || string.IsNullOrEmpty(command))
        {
            args.Cancel = true;
            ShowStatus("Name and command are required", InfoBarSeverity.Warning);
            return;
        }

        var id = name.ToLowerInvariant().Replace(" ", "-");
        var argsList = string.IsNullOrEmpty(argsText)
            ? new List<string>()
            : argsText.Split(',').Select(a => a.Trim()).ToList();

        var newServer = new McpServerItem
        {
            Id = id,
            Name = name,
            Command = command,
            Args = argsList,
            Enabled = true,
            ServerType = McpServerType.External,
            TriggerCommand = $"/mcp/{id}",
            RequiresConfirmation = true,
            Status = McpServerStatus.Stopped,
            StatusColor = new SolidColorBrush(Microsoft.UI.Colors.Gray)
        };

        _servers.Add(newServer);
        UpdateEmptyState();
        ServerListView.SelectedItem = newServer;

        // TODO: Save to AlephCore
        ShowStatus($"Server '{name}' added", InfoBarSeverity.Success);
    }

    #endregion

    #region Delete Server

    private async void DeleteServer_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedServer == null) return;

        var dialog = new ContentDialog
        {
            Title = "Delete Server",
            Content = $"Are you sure you want to delete '{_selectedServer.Name}'?",
            PrimaryButtonText = "Delete",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = this.XamlRoot
        };

        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary)
        {
            var serverToRemove = _servers.FirstOrDefault(s => s.Id == _selectedServer.Id);
            if (serverToRemove != null)
            {
                _servers.Remove(serverToRemove);
                // TODO: Delete from AlephCore
            }

            ClearSelection();
            UpdateEmptyState();
            ShowStatus("Server deleted", InfoBarSeverity.Success);
        }
    }

    #endregion

    #region Edit Handlers

    private void EnabledToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_selectedServer != null)
        {
            _selectedServer.Enabled = EnabledToggle.IsOn;
            MarkAsChanged();
        }
    }

    private void CommandBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (_selectedServer != null)
        {
            _selectedServer.Command = CommandBox.Text;
            MarkAsChanged();
        }
    }

    private void WorkingDirBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (_selectedServer != null)
        {
            _selectedServer.WorkingDirectory = WorkingDirBox.Text;
            MarkAsChanged();
        }
    }

    private void RequiresConfirmationToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_selectedServer != null)
        {
            _selectedServer.RequiresConfirmation = RequiresConfirmationToggle.IsOn;
            MarkAsChanged();
        }
    }

    private async void BrowseCommand_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FileOpenPicker();
        picker.FileTypeFilter.Add("*");

        var hwnd = GetActiveWindow();
        InitializeWithWindow.Initialize(picker, hwnd);

        var file = await picker.PickSingleFileAsync();
        if (file != null)
        {
            CommandBox.Text = file.Path;
        }
    }

    #endregion

    #region Arguments

    private void AddArg_Click(object sender, RoutedEventArgs e)
    {
        _args.Add(new StringWrapper { Value = "" });
        MarkAsChanged();
    }

    private void RemoveArg_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button btn && btn.Tag is StringWrapper arg)
        {
            _args.Remove(arg);
            MarkAsChanged();
        }
    }

    #endregion

    #region Environment Variables

    private void AddEnvVar_Click(object sender, RoutedEventArgs e)
    {
        _envVars.Add(new EnvVarItem { Key = "", Value = "" });
        MarkAsChanged();
    }

    private void RemoveEnvVar_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button btn && btn.Tag is EnvVarItem env)
        {
            _envVars.Remove(env);
            MarkAsChanged();
        }
    }

    private void ToggleEnvVisibility_Click(object sender, RoutedEventArgs e)
    {
        // TODO: Toggle password visibility
    }

    #endregion

    #region Save/Cancel

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private void SaveServer_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedServer == null) return;

        // Update arguments
        _selectedServer.Args = _args.Select(a => a.Value).ToList();

        // Update environment variables
        _selectedServer.EnvVars = _envVars.ToDictionary(e => e.Key, e => e.Value);

        // Update in list
        var index = _servers.ToList().FindIndex(s => s.Id == _selectedServer.Id);
        if (index >= 0)
        {
            _servers[index] = _selectedServer.Clone();
        }

        // TODO: Save to AlephCore
        _originalServer = _selectedServer.Clone();
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;

        ShowStatus("Server configuration saved", InfoBarSeverity.Success);
    }

    #endregion

    #region View Logs

    private async void ViewLogs_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedServer == null) return;

        var dialog = new ContentDialog
        {
            Title = $"Logs: {_selectedServer.Name}",
            Content = new ScrollViewer
            {
                Content = new TextBlock
                {
                    Text = "No logs available yet.\n\nLogs will appear here when the server is running.",
                    TextWrapping = TextWrapping.Wrap,
                    FontFamily = new FontFamily("Consolas"),
                    Foreground = (Brush)Application.Current.Resources["TextFillColorSecondaryBrush"]
                },
                MaxHeight = 400
            },
            CloseButtonText = "Close",
            XamlRoot = this.XamlRoot
        };

        await dialog.ShowAsync();
    }

    #endregion

    #region Status

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusInfoBar.Message = message;
        StatusInfoBar.Severity = severity;
        StatusInfoBar.IsOpen = true;

        // Auto-close after 3 seconds
        DispatcherQueue.TryEnqueue(async () =>
        {
            await Task.Delay(3000);
            StatusInfoBar.IsOpen = false;
        });
    }

    #endregion
}

#region Data Models

public class McpServerItem
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public McpServerType ServerType { get; set; } = McpServerType.External;
    public bool Enabled { get; set; } = true;
    public string? Command { get; set; }
    public List<string> Args { get; set; } = new();
    public Dictionary<string, string> EnvVars { get; set; } = new();
    public string? WorkingDirectory { get; set; }
    public string? TriggerCommand { get; set; }
    public bool RequiresConfirmation { get; set; } = true;
    public McpServerStatus Status { get; set; } = McpServerStatus.Stopped;
    public Brush StatusColor { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);

    public McpServerItem Clone()
    {
        return new McpServerItem
        {
            Id = Id,
            Name = Name,
            ServerType = ServerType,
            Enabled = Enabled,
            Command = Command,
            Args = new List<string>(Args),
            EnvVars = new Dictionary<string, string>(EnvVars),
            WorkingDirectory = WorkingDirectory,
            TriggerCommand = TriggerCommand,
            RequiresConfirmation = RequiresConfirmation,
            Status = Status,
            StatusColor = StatusColor
        };
    }
}

public enum McpServerType
{
    Builtin,
    External
}

public enum McpServerStatus
{
    Running,
    Stopped,
    Starting,
    Error
}

public class StringWrapper
{
    public string Value { get; set; } = "";
}

public class EnvVarItem
{
    public string Key { get; set; } = "";
    public string Value { get; set; } = "";
}

#endregion
