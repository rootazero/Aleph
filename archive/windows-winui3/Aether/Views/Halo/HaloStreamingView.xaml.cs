using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Aether.Views.Halo;

/// <summary>
/// Streaming text display view with typewriter effect and cursor.
/// </summary>
public sealed partial class HaloStreamingView : UserControl
{
    private readonly DispatcherTimer _cursorTimer;
    private bool _cursorVisible = true;

    public HaloStreamingView()
    {
        InitializeComponent();

        // Cursor blink timer
        _cursorTimer = new DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(530)
        };
        _cursorTimer.Tick += (s, e) =>
        {
            _cursorVisible = !_cursorVisible;
            CursorRun.Text = _cursorVisible ? "▌" : "";
        };
    }

    /// <summary>
    /// Current streaming text.
    /// </summary>
    public string Text
    {
        get => StreamingRun.Text;
        set
        {
            StreamingRun.Text = value;
            ScrollToEnd();
        }
    }

    /// <summary>
    /// Append text to the stream.
    /// </summary>
    public void AppendText(string text)
    {
        StreamingRun.Text += text;
        ScrollToEnd();
    }

    /// <summary>
    /// Clear all text.
    /// </summary>
    public void Clear()
    {
        StreamingRun.Text = "";
    }

    /// <summary>
    /// Start the cursor blink animation.
    /// </summary>
    public void StartCursor()
    {
        _cursorVisible = true;
        CursorRun.Text = "▌";
        _cursorTimer.Start();
    }

    /// <summary>
    /// Stop the cursor blink animation.
    /// </summary>
    public void StopCursor()
    {
        _cursorTimer.Stop();
        CursorRun.Text = "";
    }

    /// <summary>
    /// Show cursor without blinking.
    /// </summary>
    public void ShowStaticCursor()
    {
        _cursorTimer.Stop();
        CursorRun.Text = "▌";
    }

    private void ScrollToEnd()
    {
        StreamingScrollViewer.ChangeView(null, StreamingScrollViewer.ScrollableHeight, null);
    }
}
