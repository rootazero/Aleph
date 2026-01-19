using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;

namespace Aether.Views.Settings;

/// <summary>
/// AI Providers settings page - API keys and connection testing.
/// </summary>
public sealed partial class ProvidersSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    public ProvidersSettingsPage()
    {
        InitializeComponent();
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        // TODO: Update UI bindings when x:Bind is implemented
    }

    private async void TestOpenAi_Click(object sender, RoutedEventArgs e)
    {
        await TestProviderAsync("OpenAI", OpenAiKeyBox.Password);
    }

    private async void TestAnthropic_Click(object sender, RoutedEventArgs e)
    {
        await TestProviderAsync("Anthropic", AnthropicKeyBox.Password);
    }

    private async void TestGemini_Click(object sender, RoutedEventArgs e)
    {
        await TestProviderAsync("Gemini", GeminiKeyBox.Password);
    }

    private async void TestOllama_Click(object sender, RoutedEventArgs e)
    {
        await TestProviderAsync("Ollama", OllamaUrlBox.Text, isUrl: true);
    }

    private async Task TestProviderAsync(string provider, string credential, bool isUrl = false)
    {
        if (string.IsNullOrWhiteSpace(credential))
        {
            ShowStatus($"Please enter {(isUrl ? "URL" : "API key")} for {provider}", InfoBarSeverity.Warning);
            return;
        }

        ShowStatus($"Testing {provider} connection...", InfoBarSeverity.Informational);

        try
        {
            // TODO: Call Rust core to test connection
            await Task.Delay(1000); // Simulate test

            // For now, just show success
            ShowStatus($"{provider} connection successful!", InfoBarSeverity.Success);
        }
        catch (Exception ex)
        {
            ShowStatus($"{provider} connection failed: {ex.Message}", InfoBarSeverity.Error);
        }
    }

    private void ShowStatus(string message, InfoBarSeverity severity)
    {
        StatusInfoBar.Message = message;
        StatusInfoBar.Severity = severity;
        StatusInfoBar.IsOpen = true;
    }
}
