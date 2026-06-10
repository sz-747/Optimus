using System;

namespace Optimus.Core;

/// <summary>
/// A live terminal surface as the model plane sees it (KTD3): an id plus the three lifecycle
/// levers the <see cref="SurfaceManager"/> pulls. Deliberately knows nothing about WinUI, wgpu, or
/// the engine FFI — the app provides the concrete <c>TerminalPane</c> implementation, which keeps
/// the manager unit-testable against a fake.
/// </summary>
public interface ISurface
{
    /// <summary>The surface's stable id (matches the model's <see cref="SurfaceId"/>).</summary>
    SurfaceId Id { get; }

    /// <summary>
    /// Raised (on the UI thread) when the shell sets the window/tab title (OSC 0/2). The owning
    /// pane projects it into the tab header text (U4); never bound to a live observable (KTD5).
    /// </summary>
    event Action<string>? TitleChanged;

    /// <summary>
    /// Raised (on the UI thread) when a program in this surface fires a notification escape
    /// sequence — OSC 9 / 777 (parsed by the engine) or OSC 99 / Kitty (Phase 3 U1). All three
    /// arrive as the same engine TOAST host event and surface here as one payload (Phase 3 U2,
    /// KTD1). The owning <c>WorkspaceView</c> feeds it to the notification coordinator; like
    /// <see cref="TitleChanged"/> it is a plain event, never a bound observable (KTD3).
    /// </summary>
    event Action<SurfaceNotification>? NotificationRaised;

    /// <summary>
    /// Composite this surface (<c>true</c>) or collapse it out of the composition tree
    /// (<c>false</c>) so inactive surfaces cost no composition (R3/R11).
    /// </summary>
    void SetActive(bool active);

    /// <summary>Give this surface OS keyboard focus (programmatic — R8).</summary>
    void FocusSurface();

    /// <summary>
    /// Tear the surface down: stop its render thread / engine, then release native resources.
    /// Must be idempotent (R2/R9) and must run independently of XAML <c>Unloaded</c> so
    /// re-parenting never destroys a live shell (KTD9/R10).
    /// </summary>
    void Shutdown();
}

/// <summary>
/// Creates concrete <see cref="ISurface"/>s for the <see cref="SurfaceManager"/>. The app supplies
/// a factory that builds real <c>TerminalPane</c>s; tests supply a fake.
/// </summary>
public interface ISurfaceFactory
{
    /// <summary>Create a surface for <paramref name="id"/>, optionally with a working directory / command line.</summary>
    ISurface Create(SurfaceId id, string? cwd, string? cmdline);
}
