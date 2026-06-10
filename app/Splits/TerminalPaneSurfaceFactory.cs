using Optimus.Controls;
using Optimus.Core;

namespace Optimus.Splits;

/// <summary>
/// The app's concrete <see cref="ISurfaceFactory"/> (Phase 2 U2): builds a real
/// <see cref="TerminalPane"/> (engine + <see cref="Microsoft.UI.Xaml.Controls.SwapChainPanel"/>)
/// for each <see cref="SurfaceId"/> the <see cref="SurfaceManager"/> asks for. Kept tiny so the
/// manager itself stays WinUI-free and unit-testable against a fake.
/// </summary>
internal sealed class TerminalPaneSurfaceFactory : ISurfaceFactory
{
    public ISurface Create(SurfaceId id, string? cwd, string? cmdline) =>
        new TerminalPane(id, cwd, cmdline);
}
