using Microsoft.UI.Xaml;

namespace Cmux;

/// <summary>
/// The single Phase-1 window. Hosts one terminal pane (wired in U6/U7); for now it
/// shows a scaffolding placeholder so the project builds and publishes end-to-end (U2).
/// </summary>
public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "cmux";
    }
}
