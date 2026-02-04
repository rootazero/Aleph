using Microsoft.UI.Xaml.Controls;
using Aleph.ViewModels;

namespace Aleph.Views.Settings;

/// <summary>
/// Placeholder settings page for tabs not yet implemented.
/// </summary>
public sealed partial class PlaceholderSettingsPage : UserControl
{
    public PlaceholderSettingsPage()
    {
        InitializeComponent();
    }

    /// <summary>
    /// Configure the placeholder for a specific tab.
    /// </summary>
    public void Configure(SettingsTab tab)
    {
        PageIcon.Glyph = tab.GetIcon();
        PageTitle.Text = tab.GetDisplayName();
        PageDescription.Text = GetPlaceholderDescription(tab);
    }

    private static string GetPlaceholderDescription(SettingsTab tab) => tab switch
    {
        SettingsTab.Generation => "Configure image, video, and audio generation providers like Replicate, DALL-E, and Stable Diffusion.",
        SettingsTab.Routing => "Set up rules to route requests to different AI providers based on intent, keywords, or other criteria.",
        SettingsTab.Behavior => "Customize input/output modes, typewriter effects, and clipboard integration behavior.",
        SettingsTab.Search => "Configure web search providers like Brave Search, DuckDuckGo, and Google.",
        SettingsTab.Mcp => "Manage Model Context Protocol (MCP) servers for external tool integration.",
        SettingsTab.Skills => "Browse, install, and manage skills that extend Aleph's capabilities.",
        SettingsTab.Cowork => "Configure task orchestration and DAG-based multi-task execution settings.",
        SettingsTab.Policies => "Fine-tune system behavior including privacy, safety, and performance policies.",
        SettingsTab.Runtimes => "Manage external runtime environments like Python (uv), Node.js (fnm), and yt-dlp.",
        _ => "This section will be implemented in a future update."
    };
}
