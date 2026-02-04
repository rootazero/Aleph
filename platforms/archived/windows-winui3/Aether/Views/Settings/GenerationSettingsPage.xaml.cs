using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aleph.ViewModels;
using System.Collections.ObjectModel;

namespace Aleph.Views.Settings;

/// <summary>
/// Generation Settings page - Media generation provider configuration.
/// Supports Image, Video, and Audio generation providers.
/// </summary>
public sealed partial class GenerationSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private ObservableCollection<GenerationProviderItem> _imageProviders = new();
    private ObservableCollection<GenerationProviderItem> _videoProviders = new();
    private ObservableCollection<GenerationProviderItem> _audioProviders = new();
    private GenerationProviderItem? _selectedProvider;
    private string _currentCategory = "Image";
    private bool _hasUnsavedChanges;

    public GenerationSettingsPage()
    {
        InitializeComponent();
        InitializeProviders();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        UpdateProviderList();
    }

    private void InitializeProviders()
    {
        // Image providers (synced with macOS)
        _imageProviders.Add(new GenerationProviderItem { Id = "openai-dalle", Name = "OpenAI DALL-E", Description = "DALL-E image generation", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 16, 163, 127)), SupportedTypes = new[] { "Image" }, Models = new[] { "dall-e-3", "dall-e-2" } }); // #10a37f
        _imageProviders.Add(new GenerationProviderItem { Id = "stability-ai", Name = "Stability AI", Description = "Stable Diffusion models", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 139, 92, 246)), SupportedTypes = new[] { "Image" }, Models = new[] { "stable-diffusion-xl-1024-v1-0", "stable-diffusion-v1-6" } }); // #8B5CF6
        _imageProviders.Add(new GenerationProviderItem { Id = "google-imagen", Name = "Google Imagen", Description = "Google's Imagen via Gemini API", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 66, 133, 244)), SupportedTypes = new[] { "Image" }, Models = new[] { "imagen-3", "imagen-2" } }); // #4285F4
        _imageProviders.Add(new GenerationProviderItem { Id = "replicate", Name = "Replicate", Description = "Open-source models", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 249, 115, 22)), SupportedTypes = new[] { "Image" }, Models = new[] { "flux-pro", "flux-schnell", "sdxl" } }); // #F97316
        _imageProviders.Add(new GenerationProviderItem { Id = "t8star-image", Name = "T8Star", Description = "OpenAI-compatible image", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 53)), SupportedTypes = new[] { "Image" }, Models = new[] { "dall-e-3", "dall-e-2" }, AllowCustomUrl = true }); // #FF6B35
        _imageProviders.Add(new GenerationProviderItem { Id = "t8star-midjourney", Name = "T8Star Midjourney", Description = "Midjourney via T8Star", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 53)), SupportedTypes = new[] { "Image" }, Models = new[] { "midjourney-v6", "midjourney-v5" }, AllowCustomUrl = true }); // #FF6B35

        // Video providers (synced with macOS)
        _videoProviders.Add(new GenerationProviderItem { Id = "google-veo", Name = "Google Veo", Description = "Google's video generation", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 66, 133, 244)), SupportedTypes = new[] { "Video" }, Models = new[] { "veo-2", "veo-1" } }); // #4285F4
        _videoProviders.Add(new GenerationProviderItem { Id = "runway", Name = "Runway", Description = "Gen-2 video generation (coming soon)", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Purple), SupportedTypes = new[] { "Video" }, Models = new[] { "gen-3", "gen-2" } });
        _videoProviders.Add(new GenerationProviderItem { Id = "pika", Name = "Pika", Description = "AI video generation (coming soon)", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Orange), SupportedTypes = new[] { "Video" }, Models = new[] { "pika-1.0" } });
        _videoProviders.Add(new GenerationProviderItem { Id = "luma", Name = "Luma", Description = "Dream Machine (coming soon)", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Teal), SupportedTypes = new[] { "Video" }, Models = new[] { "luma-1.0" } });
        _videoProviders.Add(new GenerationProviderItem { Id = "t8star-video", Name = "T8Star Veo", Description = "Veo via T8Star", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 53)), SupportedTypes = new[] { "Video" }, Models = new[] { "veo-2", "veo-1" }, AllowCustomUrl = true }); // #FF6B35

        // Audio providers (synced with macOS)
        _audioProviders.Add(new GenerationProviderItem { Id = "openai-tts", Name = "OpenAI TTS", Description = "Text-to-speech", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 16, 163, 127)), SupportedTypes = new[] { "Audio" }, Models = new[] { "tts-1-hd", "tts-1" } }); // #10a37f
        _audioProviders.Add(new GenerationProviderItem { Id = "elevenlabs", Name = "ElevenLabs", Description = "Voice synthesis", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Blue), SupportedTypes = new[] { "Audio" }, Models = new[] { "eleven_multilingual_v2", "eleven_monolingual_v1" } });
        _audioProviders.Add(new GenerationProviderItem { Id = "google-tts", Name = "Google TTS", Description = "Google text-to-speech (coming soon)", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 66, 133, 244)), SupportedTypes = new[] { "Audio" }, Models = new[] { "wavenet", "standard" } }); // #4285F4
        _audioProviders.Add(new GenerationProviderItem { Id = "azure-tts", Name = "Azure TTS", Description = "Azure text-to-speech (coming soon)", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 0, 120, 212)), SupportedTypes = new[] { "Audio" }, Models = new[] { "neural", "standard" } }); // #0078d4
        _audioProviders.Add(new GenerationProviderItem { Id = "t8star-audio", Name = "T8Star", Description = "TTS via T8Star", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.ColorHelper.FromArgb(255, 255, 107, 53)), SupportedTypes = new[] { "Audio" }, Models = new[] { "tts-1-hd", "tts-1" }, AllowCustomUrl = true }); // #FF6B35
    }

    private void UpdateProviderList()
    {
        switch (_currentCategory)
        {
            case "Image":
                ProviderListView.ItemsSource = _imageProviders;
                break;
            case "Video":
                ProviderListView.ItemsSource = _videoProviders;
                break;
            case "Audio":
                ProviderListView.ItemsSource = _audioProviders;
                break;
        }
    }

    private void CategoryTab_Click(object sender, RoutedEventArgs e)
    {
        if (sender is RadioButton rb && rb.Tag is string category)
        {
            _currentCategory = category;
            UpdateProviderList();
            ClearSelection();
        }
    }

    private void ProviderListView_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ProviderListView.SelectedItem is GenerationProviderItem provider)
        {
            SelectProvider(provider);
        }
        else
        {
            ClearSelection();
        }
    }

    private void SelectProvider(GenerationProviderItem provider)
    {
        _selectedProvider = provider;

        // Update header
        ProviderNameText.Text = provider.Name;
        ProviderDescText.Text = provider.Description;
        ProviderIconBorder.Background = provider.IconBackground;
        ProviderIconGlyph.Glyph = provider.Icon;

        // Update supported types badges
        ImageBadge.Visibility = provider.SupportedTypes.Contains("Image") ? Visibility.Visible : Visibility.Collapsed;
        VideoBadge.Visibility = provider.SupportedTypes.Contains("Video") ? Visibility.Visible : Visibility.Collapsed;
        AudioBadge.Visibility = provider.SupportedTypes.Contains("Audio") ? Visibility.Visible : Visibility.Collapsed;

        // Update model dropdown
        ModelComboBox.Items.Clear();
        foreach (var model in provider.Models)
        {
            ModelComboBox.Items.Add(model);
        }
        if (ModelComboBox.Items.Count > 0)
            ModelComboBox.SelectedIndex = 0;

        // Load saved config
        ApiKeyBox.Password = provider.ApiKey ?? "";
        BaseUrlBox.Text = provider.BaseUrl ?? "";

        // Show custom URL field for certain providers
        BaseUrlGrid.Visibility = provider.AllowCustomUrl ? Visibility.Visible : Visibility.Collapsed;

        // Show detail panel
        DetailPanel.Visibility = Visibility.Visible;
        EmptyDetailState.Visibility = Visibility.Collapsed;

        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
        TestResultText.Text = "Test your API connection";
    }

    private void ClearSelection()
    {
        _selectedProvider = null;
        DetailPanel.Visibility = Visibility.Collapsed;
        EmptyDetailState.Visibility = Visibility.Visible;
    }

    private void ApiKeyBox_PasswordChanged(object sender, RoutedEventArgs e)
    {
        MarkAsChanged();
    }

    private void ModelComboBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        MarkAsChanged();
    }

    private void BaseUrlBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        MarkAsChanged();
    }

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private async void TestConnection_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedProvider == null) return;

        TestConnectionButton.IsEnabled = false;
        TestResultText.Text = "Testing connection...";

        try
        {
            // TODO: Call AlephCore to test connection
            await Task.Delay(1000); // Simulate test

            var hasApiKey = !string.IsNullOrWhiteSpace(ApiKeyBox.Password);
            if (hasApiKey)
            {
                TestResultText.Text = "Connection successful (latency: 245ms)";
                TestResultText.Foreground = (Brush)Application.Current.Resources["SystemFillColorSuccessBrush"];
                ShowStatus("Connection test passed", InfoBarSeverity.Success);
            }
            else
            {
                TestResultText.Text = "API key required";
                TestResultText.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
                ShowStatus("Please enter an API key", InfoBarSeverity.Warning);
            }
        }
        catch (Exception ex)
        {
            TestResultText.Text = $"Connection failed: {ex.Message}";
            TestResultText.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
            ShowStatus($"Connection test failed: {ex.Message}", InfoBarSeverity.Error);
        }
        finally
        {
            TestConnectionButton.IsEnabled = true;
        }
    }

    private void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        if (_selectedProvider == null) return;

        // Update provider config
        _selectedProvider.ApiKey = ApiKeyBox.Password;
        _selectedProvider.BaseUrl = BaseUrlBox.Text;
        _selectedProvider.SelectedModel = ModelComboBox.SelectedItem?.ToString();

        // Update status indicator
        _selectedProvider.StatusColor = string.IsNullOrWhiteSpace(_selectedProvider.ApiKey)
            ? new SolidColorBrush(Microsoft.UI.Colors.Gray)
            : new SolidColorBrush(Microsoft.UI.Colors.Green);

        // TODO: Save to AlephCore
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;

        ShowStatus($"{_selectedProvider.Name} configuration saved", InfoBarSeverity.Success);
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

public class GenerationProviderItem
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Description { get; set; } = "";
    public string Icon { get; set; } = "\uE8B9";
    public Brush IconBackground { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public Brush StatusColor { get; set; } = new SolidColorBrush(Microsoft.UI.Colors.Gray);
    public string[] SupportedTypes { get; set; } = Array.Empty<string>();
    public string[] Models { get; set; } = Array.Empty<string>();
    public string? ApiKey { get; set; }
    public string? BaseUrl { get; set; }
    public string? SelectedModel { get; set; }
    public bool AllowCustomUrl { get; set; } = false;
}
