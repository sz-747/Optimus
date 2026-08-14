using Microsoft.UI.Xaml;
using Microsoft.UI.Windowing;

namespace Optimus;

/// <summary>
/// The single window hosting the frontend-first mission-control dashboard. The existing terminal
/// workspace host is deliberately kept out of the visual tree while the dashboard is finished and
/// verified; the later integration seam can attach it without recreating the chrome.
/// </summary>
public sealed partial class MainWindow : Window
{
    /// <summary>The live terminal/session backend, retained offscreen while the dashboard owns chrome.</summary>
    public Sidebar.WorkspaceHost WorkspaceHost => Host;

    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "optimus";
        this.ExtendsContentIntoTitleBar = true;
        this.SetTitleBar(Dashboard.TitleBarElement);
        ApplyTitleBarTheme();
        Dashboard.AttachBackend(Host);
        this.Closed += OnClosed;
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        var app = Application.Current as App;
        app?.StopPipeServer();

        Host.ShutdownAll();

        // Last, mirroring launch order (governor starts before the window): stop the capacity
        // ticker, persist the learned calibration, and release the Win32 provider.
        app?.StopCapacityGovernor();
    }

    private void ApplyTitleBarTheme()
    {
        if (!AppWindowTitleBar.IsCustomizationSupported())
        {
            return;
        }

        AppWindow.TitleBar.ButtonBackgroundColor = Design.Tokens.DashboardCanvas.Color;
        AppWindow.TitleBar.ButtonForegroundColor = Design.Tokens.TextSecondary.Color;
        AppWindow.TitleBar.ButtonHoverBackgroundColor = Design.Tokens.DashboardSelected.Color;
        AppWindow.TitleBar.ButtonHoverForegroundColor = Design.Tokens.TextPrimary.Color;
        AppWindow.TitleBar.ButtonInactiveBackgroundColor = Design.Tokens.DashboardCanvas.Color;
        AppWindow.TitleBar.ButtonInactiveForegroundColor = Design.Tokens.TextMuted.Color;
    }
}
