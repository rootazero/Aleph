using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;
using WinRT.Interop;
using Aether.ViewModels;
using Aether.Models;

namespace Aether.Windows;

/// <summary>
/// Halo floating window - the core AI interaction overlay.
///
/// Key behaviors:
/// - Does not steal focus from other applications (WS_EX_NOACTIVATE)
/// - Transparent background with blur effect
/// - Always on top (HWND_TOPMOST)
/// - No taskbar entry (WS_EX_TOOLWINDOW)
/// - Follows cursor position
/// - Displays all 21 AI response states
/// - Integrates with HaloViewModel for MVVM pattern
/// </summary>
public sealed partial class HaloWindow : Window
{
    #region Win32 Constants

    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_TOPMOST = 0x00000008;

    private const int GWL_STYLE = -16;
    private const int WS_CAPTION = 0x00C00000;
    private const int WS_SYSMENU = 0x00080000;

    private const uint SWP_NOMOVE = 0x0002;
    private const uint SWP_NOSIZE = 0x0001;
    private const uint SWP_NOACTIVATE = 0x0010;
    private const uint SWP_SHOWWINDOW = 0x0040;

    private static readonly IntPtr HWND_TOPMOST = new(-1);

    private const int SW_SHOWNOACTIVATE = 4;
    private const int SW_HIDE = 0;

    #endregion

    #region Win32 Imports

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter,
        int X, int Y, int cx, int cy, uint uFlags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    #endregion

    private readonly IntPtr _hwnd;
    private readonly AppWindow _appWindow;
    private readonly HaloViewModel _viewModel;

    public bool IsVisible { get; private set; }

    /// <summary>
    /// The ViewModel for data binding.
    /// </summary>
    public HaloViewModel ViewModel => _viewModel;

    public HaloWindow()
    {
        InitializeComponent();

        _hwnd = WindowNative.GetWindowHandle(this);
        var windowId = Win32Interop.GetWindowIdFromWindow(_hwnd);
        _appWindow = AppWindow.GetFromWindowId(windowId);

        _viewModel = new HaloViewModel();
        _viewModel.PropertyChanged += ViewModel_PropertyChanged;

        ConfigureWindow();
    }

    private void ConfigureWindow()
    {
        // Remove title bar
        if (_appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
            presenter.SetBorderAndTitleBar(false, false);
        }

        // Set initial size
        _appWindow.Resize(new SizeInt32(280, 180));

        // Apply no-focus window styles
        int exStyle = GetWindowLong(_hwnd, GWL_EXSTYLE);
        exStyle |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        SetWindowLong(_hwnd, GWL_EXSTYLE, exStyle);

        // Remove caption
        int style = GetWindowLong(_hwnd, GWL_STYLE);
        style &= ~WS_CAPTION;
        style &= ~WS_SYSMENU;
        SetWindowLong(_hwnd, GWL_STYLE, style);

        // Apply topmost
        SetWindowPos(_hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }

    /// <summary>
    /// Show the Halo at specified screen coordinates.
    /// </summary>
    public void ShowAt(int x, int y)
    {
        _appWindow.Move(new PointInt32(x, y));
        ShowWindow(_hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(_hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);

        IsVisible = true;
        _viewModel.StartListening();
    }

    /// <summary>
    /// Hide the Halo window.
    /// </summary>
    public new void Hide()
    {
        ShowWindow(_hwnd, SW_HIDE);
        IsVisible = false;
        _viewModel.Hide();
    }

    #region State Management

    /// <summary>
    /// Set the current Halo state via ViewModel.
    /// </summary>
    public void SetState(HaloState state)
    {
        _viewModel.TransitionTo(state);
    }

    /// <summary>
    /// Start tool execution.
    /// </summary>
    public void StartToolExecution(string toolName)
    {
        _viewModel.StartToolExecution(toolName);
        ToolView.ShowExecuting(toolName);
    }

    /// <summary>
    /// Complete tool execution successfully.
    /// </summary>
    public void CompleteToolExecution(string? result = null)
    {
        _viewModel.ToolComplete(result);
        ToolView.ShowSuccess(result);
    }

    /// <summary>
    /// Mark tool execution as failed.
    /// </summary>
    public void FailToolExecution(string error)
    {
        _viewModel.ToolFailed(error);
        ToolView.ShowError(error);
    }

    /// <summary>
    /// Append streaming text.
    /// </summary>
    public void AppendStreamingText(string text)
    {
        _viewModel.AppendText(text);
        StreamingView.AppendText(text);
    }

    /// <summary>
    /// Clear streaming text.
    /// </summary>
    public void ClearStreamingText()
    {
        _viewModel.ClearStreamingText();
        StreamingView.Clear();
    }

    /// <summary>
    /// Set error message.
    /// </summary>
    public void SetError(string message)
    {
        _viewModel.SetError(message);
    }

    /// <summary>
    /// Set clarification message.
    /// </summary>
    public void SetClarification(string message)
    {
        ClarificationText.Text = message;
        _viewModel.RequestClarification();
    }

    #endregion

    #region ViewModel Property Changed Handler

    private void ViewModel_PropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(HaloViewModel.State))
        {
            UpdateUIForState(_viewModel.State);
        }
        else if (e.PropertyName == nameof(HaloViewModel.GradientStartColor) ||
                 e.PropertyName == nameof(HaloViewModel.GradientEndColor))
        {
            SetHaloColor(_viewModel.GradientStartColor, _viewModel.GradientEndColor);
        }
    }

    private void UpdateUIForState(HaloState state)
    {
        // Update status text
        StatusText.Text = state.GetDisplayText();

        // Reset all visibility
        StatusText.Visibility = Visibility.Visible;
        StreamingView.Visibility = Visibility.Collapsed;
        ToolView.Visibility = Visibility.Collapsed;
        ErrorBorder.Visibility = Visibility.Collapsed;
        ClarificationBorder.Visibility = Visibility.Collapsed;
        MultiTurnBadge.Visibility = Visibility.Collapsed;
        AgentBadge.Visibility = Visibility.Collapsed;

        // Update thinking ring
        ThinkingRing.IsActive = state.IsLoading();

        // Update gradient colors
        var colors = state.GetGradientColors();
        SetHaloColor(colors.Start, colors.End);

        // State-specific UI updates
        switch (state)
        {
            case HaloState.Hidden:
                break;

            case HaloState.Streaming:
            case HaloState.MultiTurnStreaming:
                StatusText.Visibility = Visibility.Collapsed;
                StreamingView.Visibility = Visibility.Visible;
                StreamingView.StartCursor();
                if (state == HaloState.MultiTurnStreaming)
                    MultiTurnBadge.Visibility = Visibility.Visible;
                break;

            case HaloState.Success:
                StreamingView.StopCursor();
                break;

            case HaloState.Error:
            case HaloState.ToolError:
                StatusText.Visibility = Visibility.Collapsed;
                ErrorBorder.Visibility = Visibility.Visible;
                ErrorText.Text = _viewModel.ErrorMessage;
                if (state == HaloState.ToolError)
                    ErrorText.Text = _viewModel.ToolResult;
                break;

            case HaloState.ToolExecuting:
            case HaloState.ToolSuccess:
                StatusText.Visibility = Visibility.Collapsed;
                ToolView.Visibility = Visibility.Visible;
                break;

            case HaloState.ClarificationNeeded:
                StatusText.Visibility = Visibility.Collapsed;
                ClarificationBorder.Visibility = Visibility.Visible;
                break;

            case HaloState.ClarificationReceived:
                ClarificationBorder.Visibility = Visibility.Collapsed;
                break;

            case HaloState.MultiTurnActive:
            case HaloState.MultiTurnThinking:
                MultiTurnBadge.Visibility = Visibility.Visible;
                break;

            case HaloState.AgentPlanning:
            case HaloState.AgentExecuting:
            case HaloState.AgentComplete:
                AgentBadge.Visibility = Visibility.Visible;
                break;
        }

        // Update window size based on content
        UpdateWindowSize(state);
    }

    private void UpdateWindowSize(HaloState state)
    {
        var (width, height) = state switch
        {
            HaloState.Streaming or HaloState.MultiTurnStreaming => (360, 300),
            HaloState.ToolExecuting or HaloState.ToolSuccess or HaloState.ToolError => (320, 220),
            HaloState.ClarificationNeeded => (320, 200),
            HaloState.Error => (300, 180),
            _ => (280, 180)
        };

        _appWindow.Resize(new SizeInt32(width, height));
    }

    private void SetHaloColor(string startColor, string endColor)
    {
        GradientStart.Color = ParseColor(startColor);
        GradientEnd.Color = ParseColor(endColor);
        ThinkingRing.Foreground = new SolidColorBrush(ParseColor(startColor));
    }

    private static Windows.UI.Color ParseColor(string hex)
    {
        hex = hex.TrimStart('#');
        byte a = 255, r, g, b;

        if (hex.Length == 8)
        {
            a = Convert.ToByte(hex[..2], 16);
            r = Convert.ToByte(hex[2..4], 16);
            g = Convert.ToByte(hex[4..6], 16);
            b = Convert.ToByte(hex[6..8], 16);
        }
        else
        {
            r = Convert.ToByte(hex[..2], 16);
            g = Convert.ToByte(hex[2..4], 16);
            b = Convert.ToByte(hex[4..6], 16);
        }

        return Windows.UI.Color.FromArgb(a, r, g, b);
    }

    #endregion
}
