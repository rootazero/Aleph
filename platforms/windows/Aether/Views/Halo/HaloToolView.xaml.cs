using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Aether.Views.Halo;

/// <summary>
/// Tool execution status view with progress indicator and result display.
/// </summary>
public sealed partial class HaloToolView : UserControl
{
    public HaloToolView()
    {
        InitializeComponent();
    }

    /// <summary>
    /// Tool name being executed.
    /// </summary>
    public string ToolName
    {
        get => ToolNameText.Text;
        set => ToolNameText.Text = value;
    }

    /// <summary>
    /// Show tool as executing (indeterminate progress).
    /// </summary>
    public void ShowExecuting(string toolName, string? icon = null)
    {
        ToolNameText.Text = toolName;
        StatusText.Text = "Executing";
        StatusBadge.Background = new SolidColorBrush(ColorFromHex("#30FFFFFF"));

        ExecutionProgress.IsIndeterminate = true;
        ExecutionProgress.Visibility = Visibility.Visible;
        ExecutionProgress.Foreground = new SolidColorBrush(ColorFromHex("#FF4ECAFF"));

        DetailsBorder.Visibility = Visibility.Collapsed;

        // Set icon based on tool type
        ToolIcon.Glyph = GetToolIcon(toolName, icon);
    }

    /// <summary>
    /// Show tool as completed successfully.
    /// </summary>
    public void ShowSuccess(string? result = null)
    {
        StatusText.Text = "Completed";
        StatusBadge.Background = new SolidColorBrush(ColorFromHex("#304EFF6B"));

        ExecutionProgress.IsIndeterminate = false;
        ExecutionProgress.Value = 100;
        ExecutionProgress.Foreground = new SolidColorBrush(ColorFromHex("#FF4EFF6B"));

        if (!string.IsNullOrEmpty(result))
        {
            DetailsText.Text = result;
            DetailsBorder.Visibility = Visibility.Visible;
        }
    }

    /// <summary>
    /// Show tool as failed.
    /// </summary>
    public void ShowError(string? errorMessage = null)
    {
        StatusText.Text = "Failed";
        StatusBadge.Background = new SolidColorBrush(ColorFromHex("#30FF4E4E"));

        ExecutionProgress.IsIndeterminate = false;
        ExecutionProgress.Value = 0;
        ExecutionProgress.Foreground = new SolidColorBrush(ColorFromHex("#FFFF4E4E"));

        if (!string.IsNullOrEmpty(errorMessage))
        {
            DetailsText.Text = errorMessage;
            DetailsText.Foreground = new SolidColorBrush(ColorFromHex("#FFFF6B6B"));
            DetailsBorder.Visibility = Visibility.Visible;
        }
    }

    /// <summary>
    /// Show/hide the details panel.
    /// </summary>
    public void SetDetailsVisible(bool visible)
    {
        DetailsBorder.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>
    /// Set details text content.
    /// </summary>
    public void SetDetails(string details)
    {
        DetailsText.Text = details;
        DetailsBorder.Visibility = Visibility.Visible;
    }

    /// <summary>
    /// Reset to initial state.
    /// </summary>
    public void Reset()
    {
        ToolNameText.Text = "Tool";
        StatusText.Text = "Pending";
        StatusBadge.Background = new SolidColorBrush(ColorFromHex("#30FFFFFF"));

        ExecutionProgress.IsIndeterminate = false;
        ExecutionProgress.Value = 0;
        ExecutionProgress.Visibility = Visibility.Collapsed;

        DetailsText.Text = "";
        DetailsBorder.Visibility = Visibility.Collapsed;
    }

    /// <summary>
    /// Get appropriate icon for tool type.
    /// </summary>
    private static string GetToolIcon(string toolName, string? customIcon)
    {
        if (!string.IsNullOrEmpty(customIcon))
            return customIcon;

        // Map common tool names to icons
        return toolName.ToLowerInvariant() switch
        {
            "search" or "web_search" => "\uE721",      // Search
            "browser" or "web" => "\uE774",            // Globe
            "file" or "read_file" => "\uE8A5",         // Document
            "write" or "write_file" => "\uE70F",       // Edit
            "terminal" or "shell" or "bash" => "\uE756", // Command
            "code" or "execute" => "\uE943",           // Code
            "image" or "vision" => "\uE8B9",           // Picture
            "memory" or "recall" => "\uE7C3",          // Brain/Memory
            "calculator" or "math" => "\uE8EF",        // Calculator
            "calendar" or "date" => "\uE787",          // Calendar
            "email" or "mail" => "\uE715",             // Mail
            "settings" or "config" => "\uE713",        // Settings
            "download" => "\uE896",                    // Download
            "upload" => "\uE898",                      // Upload
            "clipboard" => "\uE77F",                   // Paste
            _ => "\uE90F"                              // Default: Wrench/Tool
        };
    }

    /// <summary>
    /// Parse hex color string to Windows.UI.Color.
    /// </summary>
    private static Windows.UI.Color ColorFromHex(string hex)
    {
        hex = hex.TrimStart('#');

        byte a = 255;
        int startIndex = 0;

        if (hex.Length == 8)
        {
            a = Convert.ToByte(hex[..2], 16);
            startIndex = 2;
        }

        byte r = Convert.ToByte(hex.Substring(startIndex, 2), 16);
        byte g = Convert.ToByte(hex.Substring(startIndex + 2, 2), 16);
        byte b = Convert.ToByte(hex.Substring(startIndex + 4, 2), 16);

        return Windows.UI.Color.FromArgb(a, r, g, b);
    }
}
