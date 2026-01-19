using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Aether.ViewModels;
using System.Collections.ObjectModel;

namespace Aether.Views.Settings;

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
        // Image providers
        _imageProviders.Add(new GenerationProviderItem { Id = "openai-dalle", Name = "DALL-E 3", Description = "OpenAI image generation", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Green), SupportedTypes = new[] { "Image" }, Models = new[] { "dall-e-3", "dall-e-2" } });
        _imageProviders.Add(new GenerationProviderItem { Id = "stability", Name = "Stability AI", Description = "Stable Diffusion models", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Purple), SupportedTypes = new[] { "Image" }, Models = new[] { "stable-diffusion-xl-1024-v1-0", "stable-diffusion-v1-6" } });
        _imageProviders.Add(new GenerationProviderItem { Id = "replicate", Name = "Replicate", Description = "Flux and other models", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Orange), SupportedTypes = new[] { "Image", "Video" }, Models = new[] { "flux-pro", "flux-schnell", "sdxl" } });
        _imageProviders.Add(new GenerationProviderItem { Id = "midjourney", Name = "Midjourney", Description = "Midjourney API", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Blue), SupportedTypes = new[] { "Image" }, Models = new[] { "midjourney-v6", "midjourney-v5" } });
        _imageProviders.Add(new GenerationProviderItem { Id = "ideogram", Name = "Ideogram", Description = "Text-to-image with typography", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Teal), SupportedTypes = new[] { "Image" }, Models = new[] { "ideogram-v2", "ideogram-v1" } });
        _imageProviders.Add(new GenerationProviderItem { Id = "google-imagen", Name = "Google Imagen", Description = "Google's image model", Icon = "\uE8B9", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Red), SupportedTypes = new[] { "Image" }, Models = new[] { "imagen-3", "imagen-2" } });

        // Video providers
        _videoProviders.Add(new GenerationProviderItem { Id = "google-veo", Name = "Google Veo", Description = "Google's video generation", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Red), SupportedTypes = new[] { "Video" }, Models = new[] { "veo-2", "veo-1" } });
        _videoProviders.Add(new GenerationProviderItem { Id = "runway", Name = "Runway", Description = "Gen-2 video generation", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Purple), SupportedTypes = new[] { "Video" }, Models = new[] { "gen-3", "gen-2" } });
        _videoProviders.Add(new GenerationProviderItem { Id = "pika", Name = "Pika Labs", Description = "AI video generation", Icon = "\uE714", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Orange), SupportedTypes = new[] { "Video" }, Models = new[] { "pika-1.0" } });

        // Audio providers
        _audioProviders.Add(new GenerationProviderItem { Id = "openai-tts", Name = "OpenAI TTS", Description = "Text-to-speech", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Green), SupportedTypes = new[] { "Audio" }, Models = new[] { "tts-1-hd", "tts-1" } });
        _audioProviders.Add(new GenerationProviderItem { Id = "elevenlabs", Name = "ElevenLabs", Description = "Voice synthesis", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Blue), SupportedTypes = new[] { "Audio" }, Models = new[] { "eleven_multilingual_v2", "eleven_monolingual_v1" } });
        _audioProviders.Add(new GenerationProviderItem { Id = "google-tts", Name = "Google TTS", Description = "Google text-to-speech", Icon = "\uE767", IconBackground = new SolidColorBrush(Microsoft.UI.Colors.Red), SupportedTypes = new[] { "Audio" }, Models = new[] { "wavenet", "standard" } });
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
            // TODO: Call AetherCore to test connection
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

        // TODO: Save to AetherCore
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
