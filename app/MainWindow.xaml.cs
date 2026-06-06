using Microsoft.UI.Xaml;

namespace Cmux;

/// <summary>
/// The single Phase-2 window, hosting a <see cref="Splits.WorkspaceView"/> (a split tree of tabbed
/// terminal panes). Forwards the focused surface's title to the window chrome and tears every engine
/// down on close so render threads stop before their panels are disposed (plan §7.2 / R9).
/// </summary>
public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "cmux";

        Workspace.ActiveTitleChanged += OnTitleChanged;
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
        // A UserControl cannot read its window's activation, so push it down to the workspace (feeds
        // the notification suppression rule R4). Deactivated == app is no longer foreground.
        Workspace.AppFocused = args.WindowActivationState != WindowActivationState.Deactivated;
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        if (Application.Current is App app)
        {
            app.StopPipeServer();
        }

        Workspace.ShutdownAll();
    }
}
