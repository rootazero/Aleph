// Aether Windows Application
// Main entry point for the WinUI 3 application

using Microsoft.UI.Xaml;
using System;
using System.IO;

namespace Aether
{
    /// <summary>
    /// Aether Windows application entry point.
    /// Provides application-level services and manages the main window lifecycle.
    /// </summary>
    public partial class App : Application
    {
        private Window? _window;

        /// <summary>
        /// Initializes the singleton application object.
        /// </summary>
        public App()
        {
            this.InitializeComponent();
        }

        /// <summary>
        /// Invoked when the application is launched.
        /// </summary>
        /// <param name="args">Details about the launch request and process.</param>
        protected override void OnLaunched(LaunchActivatedEventArgs args)
        {
            // Initialize the Aether core library
            InitializeCore();

            // Create main window (placeholder for now)
            _window = new MainWindow();
            _window.Activate();
        }

        /// <summary>
        /// Initialize the Aether Rust core library.
        /// </summary>
        private void InitializeCore()
        {
            try
            {
                // Get config path
                var configDir = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                    "Aether"
                );
                var configPath = Path.Combine(configDir, "config.toml");

                // Ensure config directory exists
                Directory.CreateDirectory(configDir);

                // TODO: Initialize core when bindings are ready
                // var result = NativeMethods.aether_init(configPath);
                // if (result != 0)
                // {
                //     throw new Exception($"Failed to initialize Aether core: error code {result}");
                // }

                System.Diagnostics.Debug.WriteLine("Aether core initialization placeholder");
            }
            catch (Exception ex)
            {
                System.Diagnostics.Debug.WriteLine($"Failed to initialize Aether core: {ex.Message}");
            }
        }
    }
}
