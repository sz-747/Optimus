using Microsoft.UI.Xaml;

namespace Cmux;

/// <summary>
/// The single window, hosting a <see cref="Sidebar.WorkspaceHost"/> (the workspace sidebar beside
/// the selected workspace's split tree — Phase 5). Forwards the focused surface's title to the
/// window chrome and tears every engine down on close so render threads stop before their panels
/// are disposed (plan §7.2 / R9).
/// </summary>
public sealed partial class MainWindow : Window
{
    public Sidebar.WorkspaceHost WorkspaceHost => Host;

    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "cmux";

        Host.ActiveTitleChanged += OnTitleChanged;
        this.Activated += OnActivated;
        this.Closed += OnClosed;
    }

    private void OnTitleChanged(string title)
    {
        // Raised on the UI thread when the focused surface's title (or the focus) changes.
        this.Title = string.IsNullOrEmpty(title) ? "cmux" : title;
    }

    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
        // A UserControl cannot read its window's activation, so push it down to the host (feeds
        // the notification suppression rule R4). Deactivated == app is no longer foreground.
        Host.AppFocused = args.WindowActivationState != WindowActivationState.Deactivated;
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        if (Application.Current is App app)
        {
            app.StopPipeServer();
        }

        Host.ShutdownAll();
    }
}
