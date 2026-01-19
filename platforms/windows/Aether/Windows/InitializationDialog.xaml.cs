using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Aether.Interop;

namespace Aether.Windows;

/// <summary>
/// Dialog for first-time initialization progress.
/// </summary>
public sealed partial class InitializationDialog : ContentDialog, IInitProgressHandler
{
    private const int TotalPhases = 6;
    private int _currentPhase = 0;
    private bool _hasError = false;
    private bool _isRetryable = false;
    private bool _isCompleted = false;

    /// <summary>Event fired when user requests retry.</summary>
    public event Action? RetryRequested;

    /// <summary>Event fired when user requests quit.</summary>
    public event Action? QuitRequested;

    /// <summary>Event fired when initialization completes successfully.</summary>
    public event Action? InitCompleted;

    public InitializationDialog()
    {
        InitializeComponent();
    }

    #region IInitProgressHandler Implementation

    public void OnPhaseStarted(string phase, uint current, uint total)
    {
        _currentPhase = (int)current;
        _hasError = false;

        PhaseText.Text = GetPhaseDisplayName(phase);
        DetailText.Text = "";
        ErrorPanel.Visibility = Visibility.Collapsed;
        DownloadPanel.Visibility = Visibility.Collapsed;

        // Update overall progress
        double progress = ((current - 1) / (double)total) * 100;
        OverallProgress.Value = progress;
    }

    public void OnPhaseProgress(string phase, double progress, string message)
    {
        DetailText.Text = message;

        // Update overall progress with phase progress
        double phaseWeight = 100.0 / TotalPhases;
        double baseProgress = ((_currentPhase - 1) / (double)TotalPhases) * 100;
        OverallProgress.Value = baseProgress + (progress * phaseWeight);
    }

    public void OnPhaseCompleted(string phase)
    {
        // Update progress to reflect completed phase
        double progress = (_currentPhase / (double)TotalPhases) * 100;
        OverallProgress.Value = progress;

        // Check if all phases complete
        if (_currentPhase >= TotalPhases)
        {
            _isCompleted = true;
            PhaseText.Text = "Initialization Complete";
            DetailText.Text = "";
            InitCompleted?.Invoke();
        }
    }

    public void OnDownloadProgress(string item, ulong downloaded, ulong total)
    {
        DownloadPanel.Visibility = Visibility.Visible;

        if (total > 0)
        {
            double percent = (downloaded / (double)total) * 100;
            DownloadText.Text = $"Downloading {item}: {FormatBytes(downloaded)} / {FormatBytes(total)}";
            DownloadProgress.Value = percent;
            DownloadProgress.IsIndeterminate = false;
        }
        else
        {
            DownloadText.Text = $"Downloading {item}: {FormatBytes(downloaded)}";
            DownloadProgress.IsIndeterminate = true;
        }
    }

    public void OnError(string phase, string message, bool isRetryable)
    {
        _hasError = true;
        _isRetryable = isRetryable;

        ErrorPanel.Visibility = Visibility.Visible;
        ErrorText.Text = $"Error in {GetPhaseDisplayName(phase)}:\n{message}";
        RetryButton.Visibility = isRetryable ? Visibility.Visible : Visibility.Collapsed;
    }

    #endregion

    #region Event Handlers

    private void OnRetryClick(object sender, RoutedEventArgs e)
    {
        ErrorPanel.Visibility = Visibility.Collapsed;
        _hasError = false;
        RetryRequested?.Invoke();
    }

    private void OnQuitClick(object sender, RoutedEventArgs e)
    {
        QuitRequested?.Invoke();
    }

    private void OnClosing(ContentDialog sender, ContentDialogClosingEventArgs args)
    {
        // Prevent closing unless completed or user chose to quit
        if (!_isCompleted && !_hasError)
        {
            args.Cancel = true;
        }
    }

    #endregion

    #region Helpers

    private static string GetPhaseDisplayName(string phase)
    {
        return phase.ToLowerInvariant() switch
        {
            "directories" => "Creating directories...",
            "config" => "Generating configuration...",
            "embedding_model" or "embeddingmodel" => "Downloading embedding model...",
            "database" => "Initializing database...",
            "runtimes" => "Installing runtimes...",
            "skills" => "Setting up skills...",
            _ => phase
        };
    }

    private static string FormatBytes(ulong bytes)
    {
        string[] suffixes = { "B", "KB", "MB", "GB" };
        int i = 0;
        double size = bytes;
        while (size >= 1024 && i < suffixes.Length - 1)
        {
            size /= 1024;
            i++;
        }
        return $"{size:F1} {suffixes[i]}";
    }

    #endregion
}
