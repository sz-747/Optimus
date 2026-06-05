using Microsoft.UI.Xaml;

namespace Cmux;

/// <summary>
/// The single Phase-1 window, hosting one <see cref="Controls.TerminalPane"/>. Forwards the
/// shell's title to the window chrome and tears the pane (and its engine) down on close so the
/// render thread stops before the panel is disposed (plan §7.2).
/// </summary>
public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "cmux";

        Pane.TitleChanged += OnTitleChanged;
        this.Closed += OnClosed;
    }

    private void OnTitleChanged(string title)
    {
        // Raised on the UI thread by the engine event callback.
        this.Title = string.IsNullOrEmpty(title) ? "cmux" : title;
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        Pane.Shutdown();
    }
}
