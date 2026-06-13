using Optimus.Core;

namespace Optimus.Splits;

/// <summary>
/// The app's <see cref="ISurfaceFactory"/> for <see cref="SurfaceKind.Web"/> (p6 U4): builds a
/// <see cref="WebView2Surface"/> for each id the <see cref="SurfaceManager"/> asks for. Registered
/// alongside the terminal factory in <c>WorkspaceView</c>. <c>cmdline</c> carries an optional initial
/// URL; <c>cwd</c> is unused for a browser pane.
/// </summary>
internal sealed class WebView2SurfaceFactory : ISurfaceFactory
{
    public ISurface Create(SurfaceId id, string? cwd, string? cmdline) =>
        new WebView2Surface(id, initialUrl: cmdline);
}
