namespace Cmux.Core;

/// <summary>
/// The attach-once / shutdown-once guard at the heart of the Phase-2 correctness fix (KTD9, R10).
/// Tree restructuring re-parents leaf controls, which fires XAML <c>Unloaded</c>/<c>Loaded</c> a
/// second time; the surface must attach its GPU panel <b>exactly once</b> and tear down <b>exactly
/// once</b>, regardless of how many lifecycle events fire. Extracted from the WinUI control so the
/// invariant is unit-testable without a UI host.
/// </summary>
public sealed class SurfaceLifecycleGuard
{
    private bool _attached;
    private bool _disposed;

    /// <summary><c>true</c> once <see cref="TryAttach"/> has succeeded.</summary>
    public bool IsAttached => _attached;

    /// <summary><c>true</c> once <see cref="TryShutdown"/> has succeeded.</summary>
    public bool IsDisposed => _disposed;

    /// <summary>
    /// Returns <c>true</c> exactly once — on the first call before shutdown. The first
    /// <c>Loaded</c> attaches; any later <c>Loaded</c> from re-parenting gets <c>false</c> and must
    /// not re-attach (R10). Always <c>false</c> after shutdown.
    /// </summary>
    public bool TryAttach()
    {
        if (_disposed || _attached)
        {
            return false;
        }
        _attached = true;
        return true;
    }

    /// <summary>
    /// Returns <c>true</c> exactly once — making teardown idempotent (R2/R9). Subsequent calls
    /// (a second close, or a stray <c>Unloaded</c>) get <c>false</c>.
    /// </summary>
    public bool TryShutdown()
    {
        if (_disposed)
        {
            return false;
        }
        _disposed = true;
        return true;
    }
}
