using Microsoft.UI.Xaml;

namespace POC.RustFFI;

/// <summary>
/// POC Application - Tests Rust FFI callback mechanism
/// </summary>
public partial class App : Application
{
    private MainWindow? _mainWindow;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _mainWindow = new MainWindow();
        _mainWindow.Activate();
    }
}
