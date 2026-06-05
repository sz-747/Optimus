using Microsoft.UI.Xaml;

namespace Cmux;

/// <summary>
/// Application entry point for the cmux WinUI 3 shell (plan §4). Phase 1 opens a single
/// window hosting one terminal pane.
/// </summary>
public partial class App : Application
{
    private Window? _window;

    public App()
    {
        this.InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();
    }
}
