// Aether Main Window
// Primary window for the Aether Windows application

using Microsoft.UI.Xaml;

namespace Aether
{
    /// <summary>
    /// Main window for the Aether application.
    /// Note: In production, Aether will be a system tray application
    /// without a persistent main window.
    /// </summary>
    public sealed partial class MainWindow : Window
    {
        public MainWindow()
        {
            this.InitializeComponent();

            // Set window properties
            this.Title = "Aether";

            // TODO: Configure as system tray application
            // - Hide from taskbar
            // - Add system tray icon
            // - Register global hotkeys
        }
    }
}
