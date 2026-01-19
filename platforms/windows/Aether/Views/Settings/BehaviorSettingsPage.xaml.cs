using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.ViewModels;
using Aether.Interop;
using System.Text.Json;

namespace Aether.Views.Settings;

/// <summary>
/// Behavior Settings page - Output mode, typing speed, and PII protection configuration.
/// </summary>
public sealed partial class BehaviorSettingsPage : UserControl
{
    public SettingsViewModel ViewModel { get; set; } = null!;

    private bool _hasUnsavedChanges;
    private bool _isLoading = true;
    private AetherCore? _core;

    public BehaviorSettingsPage()
    {
        InitializeComponent();
        _isLoading = false;
    }

    public void SetViewModel(SettingsViewModel viewModel)
    {
        ViewModel = viewModel;
        _core = App.Current?.AetherCore;
        LoadSettings();
    }

    private void LoadSettings()
    {
        _isLoading = true;

        try
        {
            var configJson = _core?.GetBehaviorConfig();
            if (!string.IsNullOrEmpty(configJson))
            {
                var config = JsonSerializer.Deserialize<JsonElement>(configJson);

                // Output mode
                var outputMode = config.TryGetProperty("output_mode", out var mode) ? mode.GetString() : "typewriter";
                OutputModePicker.SelectedIndex = outputMode switch
                {
                    "instant" => 1,
                    "stream" => 2,
                    _ => 0 // typewriter
                };

                // Typing speed
                var typingSpeed = config.TryGetProperty("typing_speed", out var speed) ? speed.GetInt32() : 50;
                TypingSpeedSlider.Value = typingSpeed;
                TypingSpeedValue.Text = $"{typingSpeed} c/s";

                // PII settings
                if (config.TryGetProperty("pii", out var pii))
                {
                    PiiMasterToggle.IsOn = pii.TryGetProperty("enabled", out var en) && en.GetBoolean();
                    EmailToggle.IsOn = pii.TryGetProperty("email", out var em) && em.GetBoolean();
                    PhoneToggle.IsOn = pii.TryGetProperty("phone", out var ph) && ph.GetBoolean();
                    SsnToggle.IsOn = pii.TryGetProperty("ssn", out var ss) && ss.GetBoolean();
                    CreditCardToggle.IsOn = pii.TryGetProperty("credit_card", out var cc) && cc.GetBoolean();
                    IpAddressToggle.IsOn = pii.TryGetProperty("ip_address", out var ip) && ip.GetBoolean();
                }

                // Formatting settings
                if (config.TryGetProperty("formatting", out var fmt))
                {
                    SyntaxHighlightToggle.IsOn = !fmt.TryGetProperty("syntax_highlight", out var sh) || sh.GetBoolean();
                    MarkdownToggle.IsOn = !fmt.TryGetProperty("markdown", out var md) || md.GetBoolean();
                    CodeCopyButtonToggle.IsOn = !fmt.TryGetProperty("code_copy_button", out var cb) || cb.GetBoolean();
                }
            }
            else
            {
                // Set defaults
                OutputModePicker.SelectedIndex = 0;
                TypingSpeedSlider.Value = 50;
                TypingSpeedValue.Text = "50 c/s";
                PiiMasterToggle.IsOn = true;
                EmailToggle.IsOn = true;
                PhoneToggle.IsOn = true;
                SsnToggle.IsOn = true;
                CreditCardToggle.IsOn = true;
                IpAddressToggle.IsOn = false;
                SyntaxHighlightToggle.IsOn = true;
                MarkdownToggle.IsOn = true;
                CodeCopyButtonToggle.IsOn = true;
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"LoadSettings error: {ex.Message}");
            // Set defaults on error
            OutputModePicker.SelectedIndex = 0;
            TypingSpeedSlider.Value = 50;
            PiiMasterToggle.IsOn = true;
        }

        UpdateTypingSpeedVisibility();
        UpdatePiiOptionsVisibility();

        _isLoading = false;
        _hasUnsavedChanges = false;
        SaveButton.IsEnabled = false;
    }

    private void OutputModePicker_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoading)
        {
            UpdateTypingSpeedVisibility();
            MarkAsChanged();
        }
    }

    private void UpdateTypingSpeedVisibility()
    {
        // Show typing speed card only when Typewriter mode is selected
        var isTypewriter = OutputModePicker.SelectedIndex == 0;
        TypingSpeedCard.Visibility = isTypewriter ? Visibility.Visible : Visibility.Collapsed;
    }

    private void TypingSpeedSlider_ValueChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs e)
    {
        if (TypingSpeedValue != null)
        {
            TypingSpeedValue.Text = $"{(int)e.NewValue} c/s";
        }
        if (!_isLoading) MarkAsChanged();
    }

    private void PiiMasterToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading)
        {
            UpdatePiiOptionsVisibility();
            MarkAsChanged();
        }
    }

    private void UpdatePiiOptionsVisibility()
    {
        // Enable/disable individual PII options based on master toggle
        PiiOptionsPanel.Opacity = PiiMasterToggle.IsOn ? 1.0 : 0.5;
        EmailToggle.IsEnabled = PiiMasterToggle.IsOn;
        PhoneToggle.IsEnabled = PiiMasterToggle.IsOn;
        SsnToggle.IsEnabled = PiiMasterToggle.IsOn;
        CreditCardToggle.IsEnabled = PiiMasterToggle.IsOn;
        IpAddressToggle.IsEnabled = PiiMasterToggle.IsOn;
    }

    private void PiiOption_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void FormattingOption_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_isLoading) MarkAsChanged();
    }

    private void MarkAsChanged()
    {
        _hasUnsavedChanges = true;
        SaveButton.IsEnabled = true;
    }

    private async void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        // Get output mode
        var outputMode = OutputModePicker.SelectedIndex switch
        {
            1 => "instant",
            2 => "stream",
            _ => "typewriter"
        };

        // Get typing speed
        var typingSpeed = (int)TypingSpeedSlider.Value;

        // Build config JSON
        var configJson = JsonSerializer.Serialize(new
        {
            output_mode = outputMode,
            typing_speed = typingSpeed,
            pii = new
            {
                enabled = PiiMasterToggle.IsOn,
                email = EmailToggle.IsOn,
                phone = PhoneToggle.IsOn,
                ssn = SsnToggle.IsOn,
                credit_card = CreditCardToggle.IsOn,
                ip_address = IpAddressToggle.IsOn
            },
            formatting = new
            {
                syntax_highlight = SyntaxHighlightToggle.IsOn,
                markdown = MarkdownToggle.IsOn,
                code_copy_button = CodeCopyButtonToggle.IsOn
            }
        });

        // Save to AetherCore
        var success = await Task.Run(() => _core?.UpdateBehaviorConfig(configJson) ?? false);

        if (success)
        {
            _hasUnsavedChanges = false;
            SaveButton.IsEnabled = false;
            ShowStatus("Behavior settings saved", InfoBarSeverity.Success);
        }
        else
        {
            ShowStatus("Failed to save behavior settings", InfoBarSeverity.Error);
        }
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
