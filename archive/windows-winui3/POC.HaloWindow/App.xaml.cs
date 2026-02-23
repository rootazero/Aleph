using Microsoft.UI.Xaml;

namespace POC.HaloWindow;

/// <summary>
/// POC Application - Tests no-focus transparent Halo window
/// </summary>
public partial class App : Application
{
    private HaloWindow? _haloWindow;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Create and show the main control window
        var controlWindow = new ControlWindow();
        controlWindow.Activate();

        // Create the Halo window (initially hidden)
        _haloWindow = new HaloWindow();

        // Wire up control window events
        controlWindow.ShowHaloRequested += (x, y) =>
        {
            _haloWindow.ShowAt(x, y);
        };

        controlWindow.HideHaloRequested += () =>
        {
            _haloWindow.Hide();
        };
    }
}
