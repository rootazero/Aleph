using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aleph.ViewModels;
using System.Collections.ObjectModel;
using System.Runtime.InteropServices;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace Aleph.Views.Settings;

/// <summary>
/// Skills Settings page - Claude Agent Skills management.
/// Features install, delete, and browse capabilities matching macOS implementation.
/// </summary>
public sealed partial class SkillsSettingsPage : UserControl
{
    [DllImport("user32.dll")]
    private static extern IntPtr GetActiveWindow();

    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<SkillItem> _skills = new();
    private SkillItem? _skillToDelete;
    private string? _selectedZipPath;

    public SkillsSettingsPage()
    {
        InitializeComponent();
        SkillsItemsControl.ItemsSource = _skills;

        // Handle install method selection change
        InstallMethodPicker.SelectionChanged += InstallMethodPicker_SelectionChanged;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        LoadSkills();
        UpdateSkillsDirectory();
    }

    #region Data Loading

    private async void LoadSkills()
    {
        LoadingState.Visibility = Visibility.Visible;
        EmptyState.Visibility = Visibility.Collapsed;
        SkillsItemsControl.Visibility = Visibility.Collapsed;

        try
        {
            // Simulate loading delay
            await Task.Delay(500);

            _skills.Clear();

            // TODO: Load from AlephCore when integrated
            // For now, show empty state

            UpdateVisibility();
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to load skills: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            LoadingState.Visibility = Visibility.Collapsed;
        }
    }

    private void UpdateVisibility()
    {
        if (_skills.Count == 0)
        {
            EmptyState.Visibility = Visibility.Visible;
            SkillsItemsControl.Visibility = Visibility.Collapsed;
        }
        else
        {
            EmptyState.Visibility = Visibility.Collapsed;
            SkillsItemsControl.Visibility = Visibility.Visible;
        }
    }

    private void UpdateSkillsDirectory()
    {
        // Use ~/.config/aleph/ for cross-platform consistency
        var userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        SkillsDirectoryText.Text = Path.Combine(userProfile, ".config", "aleph", "skills");
    }

    #endregion

    #region Refresh

    private void RefreshSkills_Click(object sender, RoutedEventArgs e)
    {
        LoadSkills();
    }

    #endregion

    #region Install Skill

    private async void InstallSkill_Click(object sender, RoutedEventArgs e)
    {
        // Reset dialog state
        InstallMethodPicker.SelectedIndex = 0;
        UrlInputBox.Text = "";
        _selectedZipPath = null;
        ZipPathText.Text = "No file selected";
        UrlInputPanel.Visibility = Visibility.Visible;
        ZipInputPanel.Visibility = Visibility.Collapsed;

        InstallSkillDialog.XamlRoot = this.XamlRoot;
        await InstallSkillDialog.ShowAsync();
    }

    private void InstallMethodPicker_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (InstallMethodPicker.SelectedItem is RadioButton rb)
        {
            var method = rb.Tag?.ToString() ?? "url";
            UrlInputPanel.Visibility = method == "url" ? Visibility.Visible : Visibility.Collapsed;
            ZipInputPanel.Visibility = method == "zip" ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    private async void BrowseZip_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FileOpenPicker();
        picker.FileTypeFilter.Add(".zip");

        var hwnd = GetActiveWindow();
        InitializeWithWindow.Initialize(picker, hwnd);

        var file = await picker.PickSingleFileAsync();
        if (file != null)
        {
            _selectedZipPath = file.Path;
            ZipPathText.Text = file.Name;
            ZipPathText.Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["TextFillColorPrimaryBrush"];
        }
    }

    private async void InstallSkillDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        var method = (InstallMethodPicker.SelectedItem as RadioButton)?.Tag?.ToString() ?? "url";

        if (method == "url")
        {
            var url = UrlInputBox.Text.Trim();
            if (string.IsNullOrEmpty(url))
            {
                args.Cancel = true;
                ShowStatus("Please enter a GitHub URL", InfoBarSeverity.Warning);
                return;
            }

            await InstallFromUrl(url);
        }
        else
        {
            if (string.IsNullOrEmpty(_selectedZipPath))
            {
                args.Cancel = true;
                ShowStatus("Please select a ZIP file", InfoBarSeverity.Warning);
                return;
            }

            await InstallFromZip(_selectedZipPath);
        }
    }

    private async Task InstallFromUrl(string url)
    {
        ShowStatus("Installing skill...", InfoBarSeverity.Informational);

        try
        {
            // TODO: Call AlephCore to install skill
            await Task.Delay(1000); // Simulate installation

            // For demo, add a mock skill
            var skillName = ExtractSkillNameFromUrl(url);
            var newSkill = new SkillItem
            {
                Id = skillName.ToLowerInvariant().Replace(" ", "-"),
                Name = skillName,
                Description = $"Skill installed from {url}",
                UsageHint = $"Use /{skillName.ToLowerInvariant().Replace(" ", "-")} to activate"
            };

            _skills.Add(newSkill);
            UpdateVisibility();

            ShowStatus($"Skill '{skillName}' installed successfully", InfoBarSeverity.Success);
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to install skill: {ex.Message}", InfoBarSeverity.Error);
        }
    }

    private async Task InstallFromZip(string path)
    {
        ShowStatus("Installing skills from ZIP...", InfoBarSeverity.Informational);

        try
        {
            // TODO: Call AlephCore to install from ZIP
            await Task.Delay(1000); // Simulate installation

            // For demo, add a mock skill
            var fileName = Path.GetFileNameWithoutExtension(path);
            var newSkill = new SkillItem
            {
                Id = fileName.ToLowerInvariant(),
                Name = fileName,
                Description = "Skill installed from ZIP file",
                UsageHint = $"Use /{fileName.ToLowerInvariant()} to activate"
            };

            _skills.Add(newSkill);
            UpdateVisibility();

            ShowStatus("Skills installed successfully", InfoBarSeverity.Success);
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to install skills: {ex.Message}", InfoBarSeverity.Error);
        }
    }

    private string ExtractSkillNameFromUrl(string url)
    {
        try
        {
            var uri = new Uri(url);
            var segments = uri.AbsolutePath.Trim('/').Split('/');
            if (segments.Length >= 2)
            {
                return segments[^1]; // Last segment is usually the repo name
            }
        }
        catch { }

        return "New Skill";
    }

    #endregion

    #region Delete Skill

    private async void DeleteSkill_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button btn && btn.Tag is SkillItem skill)
        {
            _skillToDelete = skill;
            DeleteConfirmText.Text = $"Are you sure you want to delete '{skill.Name}'?";
            DeleteConfirmDialog.XamlRoot = this.XamlRoot;
            await DeleteConfirmDialog.ShowAsync();
        }
    }

    private void DeleteConfirmDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        if (_skillToDelete != null)
        {
            try
            {
                // TODO: Call AlephCore to delete skill
                _skills.Remove(_skillToDelete);
                UpdateVisibility();
                ShowStatus($"Skill '{_skillToDelete.Name}' deleted", InfoBarSeverity.Success);
            }
            catch (Exception ex)
            {
                ShowStatus($"Failed to delete skill: {ex.Message}", InfoBarSeverity.Error);
            }
            finally
            {
                _skillToDelete = null;
            }
        }
    }

    #endregion

    #region Open Directory

    private void OpenSkillsDirectory_Click(object sender, RoutedEventArgs e)
    {
        var path = SkillsDirectoryText.Text;

        try
        {
            // Create directory if it doesn't exist
            if (!Directory.Exists(path))
            {
                Directory.CreateDirectory(path);
            }

            System.Diagnostics.Process.Start("explorer.exe", path);
        }
        catch (Exception ex)
        {
            ShowStatus($"Failed to open directory: {ex.Message}", InfoBarSeverity.Error);
        }
    }

    #endregion

    #region Status

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusInfoBar.Message = message;
        StatusInfoBar.Severity = severity;
        StatusInfoBar.IsOpen = true;

        // Auto-close success/info messages
        if (severity == InfoBarSeverity.Success || severity == InfoBarSeverity.Informational)
        {
            DispatcherQueue.TryEnqueue(async () =>
            {
                await Task.Delay(3000);
                StatusInfoBar.IsOpen = false;
            });
        }
    }

    #endregion
}

#region Data Models

public class SkillItem
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Description { get; set; } = "";
    public string UsageHint { get; set; } = "";
    public List<string> AllowedTools { get; set; } = new();
}

#endregion
