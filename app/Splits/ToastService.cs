using System;
using System.Globalization;
using Cmux.Core;
using Microsoft.UI.Dispatching;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace Cmux.Splits;

/// <summary>
/// Windows desktop-toast surface (plan Phase 3 U8) over the Windows App SDK
/// <see cref="AppNotificationManager"/>. Deliberately isolated and cuttable (KTD8): every entry point
/// is wrapped so that if unpackaged COM-activation registration ever fails, the service goes inert
/// and the in-app flash + unread badge (U7) still deliver the core value — no crash, no half state.
///
/// <para>Builds a toast from a <see cref="TerminalNotification"/> (title/body), embeds the
/// originating <see cref="SurfaceId"/> in the activation arguments, and raises <see cref="Activated"/>
/// on the UI thread when the toast is clicked. The <c>NotificationInvoked</c> callback arrives on a
/// background thread, so it copies the id out and marshals via the dispatcher — the same UI-thread
/// hop discipline as <c>EngineHandle</c>'s host-event callback.</para>
/// </summary>
public sealed class ToastService : IDisposable
{
    private const string SurfaceArgKey = "surfaceId";

    private readonly DispatcherQueue _dispatcher;
    private bool _registered;

    /// <summary>Raised on the UI thread when a toast is clicked, carrying its originating surface (AE6).</summary>
    public event Action<SurfaceId>? Activated;

    public ToastService(DispatcherQueue dispatcher) => _dispatcher = dispatcher;

    /// <summary>
    /// Register the unpackaged COM activator so Windows can route toast clicks back to the running
    /// app. Wrapped in try/catch: a registration failure leaves the service inert (KTD8) rather than
    /// taking down startup, and <see cref="Show"/> then becomes a no-op.
    /// </summary>
    public void Register()
    {
        if (_registered)
        {
            return;
        }
        try
        {
            AppNotificationManager.Default.NotificationInvoked += OnInvoked;
            AppNotificationManager.Default.Register();
            _registered = true;
        }
        catch (Exception ex)
        {
            AppNotificationManager.Default.NotificationInvoked -= OnInvoked;
            App.LogError("ToastService.Register", ex);
        }
    }

    /// <summary>Unregister the COM activator at app exit. Safe to call when registration never succeeded.</summary>
    public void Unregister()
    {
        if (!_registered)
        {
            return;
        }
        try
        {
            AppNotificationManager.Default.NotificationInvoked -= OnInvoked;
            AppNotificationManager.Default.Unregister();
        }
        catch (Exception ex)
        {
            App.LogError("ToastService.Unregister", ex);
        }
        _registered = false;
    }

    /// <summary>
    /// Build and show a toast for <paramref name="n"/>. No-op (not an error) when registration failed,
    /// so the caller never has to branch on toast availability.
    /// </summary>
    public void Show(TerminalNotification n)
    {
        if (!_registered)
        {
            return;
        }
        try
        {
            string title = string.IsNullOrEmpty(n.Title) ? "cmux" : n.Title;
            AppNotificationBuilder builder = new AppNotificationBuilder()
                .AddArgument(SurfaceArgKey, n.SurfaceId.Value.ToString(CultureInfo.InvariantCulture))
                .AddText(title);
            if (!string.IsNullOrEmpty(n.Body))
            {
                builder.AddText(n.Body);
            }
            AppNotificationManager.Default.Show(builder.BuildNotification());
        }
        catch (Exception ex)
        {
            App.LogError("ToastService.Show", ex);
        }
    }

    private void OnInvoked(AppNotificationManager sender, AppNotificationActivatedEventArgs args)
    {
        if (args.Arguments.TryGetValue(SurfaceArgKey, out string? raw)
            && int.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out int id))
        {
            _dispatcher.TryEnqueue(() => Activated?.Invoke(new SurfaceId(id)));
        }
    }

    public void Dispose() => Unregister();
}
