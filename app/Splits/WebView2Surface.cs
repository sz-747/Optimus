using System;
using System.Threading.Tasks;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.Web.WebView2.Core;
using Windows.System;

namespace Optimus.Splits;

/// <summary>
/// A WebView2 browser pane (p6 U4): a second <see cref="ISurface"/> kind that lives in the split
/// tree beside the terminals. It mirrors <c>TerminalPane</c>'s lifecycle discipline — the
/// <see cref="SurfaceLifecycleGuard"/> attaches the heavy native resource (here the CoreWebView2,
/// not a wgpu surface) <b>exactly once</b> and tears it down <b>exactly once</b>, driven by the
/// <c>SurfaceManager</c> rather than XAML <c>Unloaded</c>, so a split re-parent never destroys a
/// live page (KTD9/R10).
///
/// <para><b>Capacity.</b> A web pane consumes one RAM safe-zone slot exactly like a terminal — it is
/// admitted through the same <c>SurfaceManager.TryCreateSurface</c> choke point, so the capacity
/// indicator and the cap apply to it unchanged. (Tier-2 Job-Object backstopping of the browser
/// process tree, the terminal's <c>EnrollChildInJobObject</c> equivalent, is deferred: web panes
/// share one browser process group via the shared environment, so there is no per-pane child to cap.
/// The tier-1 admission slot is the headline guarantee and is enforced.)</para>
///
/// <para><b>Runtime fail-open.</b> If the WebView2 Evergreen runtime is absent (installer contract
/// <c>installer/README.md</c> §3), the pane shows an inline "runtime required" panel with a download
/// link and never throws — terminal spawning is never blocked, mirroring the governor's fail-open
/// rule.</para>
/// </summary>
internal sealed class WebView2Surface : UserControl, ISurface
{
    private const double AddressBarHeight = 32.0;
    private const string DefaultUrl = "about:blank";
    private const string RuntimeDownloadUrl = "https://developer.microsoft.com/microsoft-edge/webview2/";

    private readonly SurfaceId _id;
    private readonly SurfaceLifecycleGuard _lifecycle = new();
    private readonly Grid _root = new() { Background = Tokens.Surface0 };
    private readonly TextBox _address;
    private readonly WebView2 _webView = new();
    private readonly FrameworkElement _runtimeMissing;

    private string _pendingUrl;
    private bool _coreReady;

    /// <summary>This surface's stable id (<see cref="ISurface.Id"/>).</summary>
    public SurfaceId Id => _id;

    /// <summary>Raised (on the UI thread) when the page document title changes — drives the tab header.</summary>
    public event Action<string>? TitleChanged;

    /// <summary>
    /// Required by <see cref="ISurface"/> but never raised — a web pane has no engine notification
    /// channel (OSC 9/99/777). Implemented with empty accessors so the unused event does not trip
    /// the CS0067 "never used" warning (the build is 0-warnings).
    /// </summary>
    public event Action<SurfaceNotification>? NotificationRaised { add { } remove { } }

    /// <summary>
    /// Create a web pane bound to <paramref name="id"/>, optionally opening <paramref name="initialUrl"/>
    /// (empty → a blank page the user navigates from the address bar).
    /// </summary>
    public WebView2Surface(SurfaceId id, string? initialUrl = null)
    {
        _id = id;
        _pendingUrl = NormalizeUrl(initialUrl) ?? DefaultUrl;

        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(AddressBarHeight) });
        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

        _address = new TextBox
        {
            PlaceholderText = "Enter a URL and press Enter",
            Background = Tokens.Surface2,
            Foreground = Tokens.TextPrimary,
            BorderThickness = new Thickness(0),
            FontSize = Tokens.FontBody,
            VerticalAlignment = VerticalAlignment.Stretch,
            VerticalContentAlignment = VerticalAlignment.Center,
            Padding = new Thickness(8, 0, 8, 0),
        };
        _address.KeyDown += OnAddressKeyDown;
        Grid.SetRow(_address, 0);
        _root.Children.Add(_address);

        Grid.SetRow(_webView, 1);
        _root.Children.Add(_webView);

        _runtimeMissing = BuildRuntimeMissingPanel();
        _runtimeMissing.Visibility = Visibility.Collapsed;
        Grid.SetRow(_runtimeMissing, 1);
        _root.Children.Add(_runtimeMissing);

        Content = _root;
        this.Loaded += OnLoaded;
        // Deliberately NOT wiring Unloaded → Shutdown: a split re-parent fires Unloaded/Loaded, and
        // teardown is the SurfaceManager's job (KTD9/R10), exactly as in TerminalPane.
    }

    // ---- Lifecycle ---------------------------------------------------------------------------

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        // Attach exactly once: a re-parent's second Loaded (or a post-shutdown one) is a no-op so we
        // never spin up a second CoreWebView2 (KTD9/R10).
        if (!_lifecycle.TryAttach())
        {
            return;
        }
        await InitializeAsync();
    }

    private async Task InitializeAsync()
    {
        // Runtime detection (installer contract §1): GetAvailableBrowserVersionString returns the
        // installed Evergreen version, or throws WebView2RuntimeNotFoundException when it is absent.
        try
        {
            string version = CoreWebView2Environment.GetAvailableBrowserVersionString();
            if (string.IsNullOrEmpty(version))
            {
                ShowRuntimeMissing();
                return;
            }
        }
        catch (Exception ex)
        {
            App.LogError("WebView2Surface.DetectRuntime", ex);
            ShowRuntimeMissing();
            return;
        }

        try
        {
            // Shared, per-user-UDF environment (App.GetWebView2EnvironmentAsync). Re-check the
            // shutdown guard after each await: the window can close (Shutdown) while we are awaiting.
            CoreWebView2Environment environment = await App.GetWebView2EnvironmentAsync();
            if (_lifecycle.IsDisposed)
            {
                return;
            }

            await _webView.EnsureCoreWebView2Async(environment);
            if (_lifecycle.IsDisposed)
            {
                return;
            }

            _coreReady = true;
            _webView.CoreWebView2.DocumentTitleChanged += OnDocumentTitleChanged;
            TitleChanged?.Invoke("New tab");

            _address.Text = _pendingUrl == DefaultUrl ? string.Empty : _pendingUrl;
            Navigate(_pendingUrl);
        }
        catch (Exception ex)
        {
            // Init can still fail (corrupt UDF, runtime removed mid-session). Degrade to the inline
            // panel rather than crash the whole app (terminals must keep running).
            App.LogError("WebView2Surface.Initialize", ex);
            ShowRuntimeMissing();
        }
    }

    /// <summary>
    /// Tear down the CoreWebView2 and its browser process. Idempotent (<see cref="ISurface.Shutdown"/>),
    /// driven by the surface manager / window <c>Closed</c> handler — never by <c>Unloaded</c>.
    /// </summary>
    public void Shutdown()
    {
        if (!_lifecycle.TryShutdown())
        {
            return;
        }

        try
        {
            if (_coreReady && _webView.CoreWebView2 is not null)
            {
                _webView.CoreWebView2.DocumentTitleChanged -= OnDocumentTitleChanged;
            }
            _webView.Close(); // documented WinUI WebView2 disposal — releases the browser process
        }
        catch (Exception ex)
        {
            App.LogError("WebView2Surface.Shutdown", ex);
        }
    }

    /// <summary>Composite or collapse the pane (<see cref="ISurface.SetActive"/>, R3/R11) — like TerminalPane.</summary>
    public void SetActive(bool active) =>
        this.Visibility = active ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Programmatic focus (<see cref="ISurface.FocusSurface"/>, R8): the page if ready, else the address bar.</summary>
    public void FocusSurface()
    {
        if (_coreReady)
        {
            _webView.Focus(FocusState.Programmatic);
        }
        else
        {
            _address.Focus(FocusState.Programmatic);
        }
    }

    // ---- Navigation --------------------------------------------------------------------------

    private void OnAddressKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter)
        {
            return;
        }
        e.Handled = true;

        string? url = NormalizeUrl(_address.Text);
        if (url is null)
        {
            return;
        }
        _address.Text = url;
        Navigate(url);
    }

    private void Navigate(string url)
    {
        if (!_coreReady || _webView.CoreWebView2 is null)
        {
            return;
        }
        try
        {
            _webView.CoreWebView2.Navigate(url);
        }
        catch (Exception ex)
        {
            // A malformed/blocked navigation must not crash the UI thread.
            App.LogError("WebView2Surface.Navigate", ex);
        }
    }

    private void OnDocumentTitleChanged(CoreWebView2 sender, object args)
    {
        string title = sender.DocumentTitle;
        TitleChanged?.Invoke(string.IsNullOrEmpty(title) ? "Web" : title);
    }

    /// <summary>
    /// Coerce free-form address-bar text into an absolute URL: trim, accept <c>about:blank</c>,
    /// prepend <c>https://</c> when no scheme is present, and validate. Returns null for blank or
    /// un-parseable input (the caller then does nothing).
    /// </summary>
    private static string? NormalizeUrl(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }
        string text = raw.Trim();
        if (text.Equals(DefaultUrl, StringComparison.OrdinalIgnoreCase))
        {
            return DefaultUrl;
        }
        if (!text.Contains("://", StringComparison.Ordinal))
        {
            text = "https://" + text;
        }
        return Uri.TryCreate(text, UriKind.Absolute, out Uri? uri) ? uri.AbsoluteUri : null;
    }

    // ---- Runtime-missing fallback ------------------------------------------------------------

    private void ShowRuntimeMissing()
    {
        _webView.Visibility = Visibility.Collapsed;
        _runtimeMissing.Visibility = Visibility.Visible;
    }

    /// <summary>
    /// The inline "WebView2 runtime required" panel (installer contract §3): a heading, a calm
    /// explanation that terminals are unaffected, and a button that opens the Evergreen download.
    /// All color/type come from <see cref="Tokens"/> (no raw literals — DESIGN.md / guard test).
    /// </summary>
    private FrameworkElement BuildRuntimeMissingPanel()
    {
        // Default StackPanel orientation is Vertical; not set explicitly to avoid the name clash
        // between Microsoft.UI.Xaml.Controls.Orientation and Optimus.Core.Orientation (splits).
        var stack = new StackPanel
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
            Spacing = 12,
            MaxWidth = 460,
            Padding = new Thickness(24),
        };

        stack.Children.Add(new TextBlock
        {
            Text = "WebView2 runtime required",
            Foreground = Tokens.TextPrimary,
            FontSize = Tokens.FontTitle,
            HorizontalAlignment = HorizontalAlignment.Center,
        });

        stack.Children.Add(new TextBlock
        {
            Text = "This web pane needs the Microsoft Edge WebView2 runtime, which isn't installed. "
                 + "Your terminals and the safe-zone cap are unaffected — only this pane is disabled.",
            Foreground = Tokens.TextMuted,
            FontSize = Tokens.FontBody,
            TextWrapping = TextWrapping.Wrap,
            HorizontalAlignment = HorizontalAlignment.Center,
            TextAlignment = TextAlignment.Center,
        });

        var install = new Button
        {
            Content = new TextBlock { Text = "Install WebView2 runtime", FontSize = Tokens.FontBody },
            Foreground = Tokens.TextPrimary,
            Background = Tokens.SurfaceSelected,
            BorderThickness = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Center,
            Padding = new Thickness(12, 6, 12, 6),
        };
        install.Click += (_, _) => LaunchRuntimeDownload();
        stack.Children.Add(install);

        var host = new Grid { Background = Tokens.Surface0 };
        host.Children.Add(stack);
        return host;
    }

    private static async void LaunchRuntimeDownload()
    {
        try
        {
            await Launcher.LaunchUriAsync(new Uri(RuntimeDownloadUrl));
        }
        catch (Exception ex)
        {
            App.LogError("WebView2Surface.LaunchRuntimeDownload", ex);
        }
    }
}
