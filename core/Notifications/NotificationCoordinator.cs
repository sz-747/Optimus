using System;
using System.Collections.Generic;
using System.Linq;

namespace Optimus.Core;

/// <summary>
/// A notification that has cleared policy and is ready to surface (plan Phase 3 U6). Carries the
/// recorded <see cref="TerminalNotification"/> plus the two view-side effects the surface plane acts
/// on: whether to raise an OS toast and whether to flash the owning pane. The store/unread state is
/// read separately off <see cref="NotificationCoordinator.Store"/>.
/// </summary>
public readonly record struct SurfacedNotification(TerminalNotification Notification, bool ShowToast, bool Flash);

/// <summary>
/// The notification-plane orchestrator (plan Phase 3 U6): the single seam that wires the queue, the
/// store, and the policy together and exposes a tiny, UI-free surface the WorkspaceView drives. It is
/// the integration point that makes the whole feature provable in Core without WinUI, a dispatcher, or
/// a live window (R2-R6).
///
/// <para>Flow: the surface plane calls <see cref="OnNotification"/> for every raw engine toast (it
/// only enqueues — cheap, allocation-light, never inspects tree state). The WorkspaceView then calls
/// <see cref="Drain"/> on the dispatcher with a freshly captured <see cref="TreeSnapshot"/>; the
/// coordinator re-derives each notification's owning pane and visibility <i>at delivery time</i> from
/// that snapshot (KTD6), drops orphans whose surface has since closed (R5), asks the
/// <see cref="NotificationPolicy"/> what to do, records the survivors, and raises
/// <see cref="Surfaced"/> for the view to render. Deriving context at drain time (not enqueue time) is
/// the deliberate correctness choice: focus/visibility may change between the engine event and the
/// debounced drain.</para>
///
/// <para>Everything here runs on the UI thread (KTD3): the queue and store hold no thread primitives,
/// and the only async hop (scheduling the drain) lives in the view.</para>
/// </summary>
public sealed class NotificationCoordinator
{
    private readonly NotificationStore _store = new();
    private readonly NotificationQueue _queue = new();
    private readonly NotificationPolicy _policy;

    public NotificationCoordinator(NotificationPolicy? policy = null)
        => _policy = policy ?? new NotificationPolicy();

    /// <summary>The recorded-notification store (unread counts, per-pane indexes, history).</summary>
    public NotificationStore Store => _store;

    /// <summary>Whether <paramref name="surface"/> currently has an unread notification (U7 badge probe).</summary>
    public bool IsSurfaceUnread(SurfaceId surface) => _store.IsSurfaceUnread(surface);

    /// <summary>Newest notification per pane regardless of read state (the Phase 5 sidebar feed).</summary>
    public IReadOnlyDictionary<PaneId, TerminalNotification> LatestByPane => _store.LatestByPane;

    /// <summary>Currently-unread <c>(pane, surface)</c> keys (the U7 tab-dot / pane-badge source).</summary>
    public IReadOnlySet<(PaneId Pane, SurfaceId Surface)> UnreadByPaneSurface => _store.UnreadByPaneSurface;

    /// <summary>
    /// Raised once per delivered notification during <see cref="Drain"/>, after it has been recorded.
    /// The surface plane subscribes to render the OS toast and/or pane flash per the carried effects.
    /// </summary>
    public event Action<SurfacedNotification>? Surfaced;

    /// <summary>
    /// Raised when a focus change clears a pane's unread flash (its surface was just marked read).
    /// The view uses it to stop the flash animation on that pane.
    /// </summary>
    public event Action<PaneId>? FlashCleared;

    /// <summary>
    /// Enqueue a raw engine notification for <paramref name="surface"/>. Pure fan-in: it never touches
    /// tree/focus state (that is re-derived at <see cref="Drain"/> time), so it is safe to call from
    /// the host-event path before any snapshot is captured. Bursts coalesce per surface (R5/AE4).
    /// </summary>
    public void OnNotification(SurfaceId surface, SurfaceNotification payload, bool coalesce = true) =>
        _queue.Enqueue(surface, payload, coalesce);

    /// <summary>Return unread notifications from newest to oldest as a direct UI payload.</summary>
    public IReadOnlyList<TerminalNotification> ListNotifications() => _store.Items;

    /// <summary>Mutations requested by socket actions.</summary>
    public bool JumpToUnreadTarget(out PaneId pane, out SurfaceId surface)
    {
        foreach (TerminalNotification entry in _store.Items)
        {
            if (!entry.IsRead)
            {
                pane = entry.PaneId;
                surface = entry.SurfaceId;
                return true;
            }
        }

        pane = default;
        surface = default;
        return false;
    }

    public (PaneId Pane, SurfaceId Surface)? JumpToUnreadTarget()
    {
        if (JumpToUnreadTarget(out PaneId pane, out SurfaceId surface))
        {
            return (pane, surface);
        }
        return null;
    }

    public void MarkRead(Guid notificationId) => _store.MarkRead(notificationId);
    public void MarkRead(SurfaceId surface) => _store.MarkRead(surface);
    public void MarkAllRead()
    {
        foreach (Guid id in _store.Items.Where(n => !n.IsRead).Select(n => n.Id).ToArray())
        {
            _store.MarkRead(id);
        }
    }

    public void DismissNotification(Guid id) => _store.Remove(id);
    public void DismissNotificationForSurface(SurfaceId surface) => _store.ClearForSurface(surface);
    public void DismissAllRead()
    {
        foreach (Guid id in _store.Items.Where(n => n.IsRead).Select(n => n.Id).ToArray())
        {
            _store.Remove(id);
        }
    }

    public void ClearNotifications() => _store.ClearAll();

    /// <summary>
    /// Deliver the pending queue against <paramref name="snapshot"/>. Orphaned entries (surface no
    /// longer in the tree) are dropped; each survivor is delivered through <see cref="Deliver"/>.
    /// Returns whether entries remain pending (cap hit), so the view can reschedule another drain
    /// (KTD4). <paramref name="appFocused"/> is whether the app window is foreground right now.
    /// </summary>
    public bool Drain(TreeSnapshot snapshot, bool appFocused) =>
        _queue.Drain(
            isLive: surface => snapshot.Root.FindContaining(surface) is not null,
            deliver: entry => Deliver(entry, snapshot, appFocused));

    /// <summary>
    /// Mark the now-focused surface read and clear its pane flash. Called by the WorkspaceView on every
    /// focus change (R6/AE6). No-op when the focused pane has no unread notification, so it is safe to
    /// call unconditionally.
    /// </summary>
    public void OnFocusChanged(TreeSnapshot snapshot)
    {
        if (snapshot.FocusedSurface is SurfaceId surface && _store.IsSurfaceUnread(surface))
        {
            _store.MarkRead(surface);
            FlashCleared?.Invoke(snapshot.FocusedPane);
        }
    }

    private void Deliver(NotificationEntry entry, TreeSnapshot snapshot, bool appFocused)
    {
        // The surface is guaranteed live here (the queue's liveness probe dropped orphans), so the
        // leaf lookup cannot be null.
        PaneLeaf leaf = snapshot.Root.FindContaining(entry.Surface)!;
        DeliveryContext context = DeriveContext(snapshot, leaf, entry.Surface, appFocused);
        NotificationEffects effects = _policy.Decide(entry.Payload, context);

        TerminalNotification n = TerminalNotification.Create(
            entry.Surface,
            leaf.Id,
            entry.Payload.Title,
            entry.Payload.Subtitle,
            entry.Payload.Body,
            paneFlash: effects.PaneFlash,
            isRead: !effects.MarkUnread);

        if (effects.Record)
        {
            _store.Add(n);
        }

        Surfaced?.Invoke(new SurfacedNotification(n, ShowToast: effects.Desktop, Flash: effects.PaneFlash));
    }

    // Whether the notification's surface is in front of the user right now: app foreground, the surface
    // is the focused one, and it is actually visible (its tab is selected and no other pane is zoomed
    // over it). Re-derived from the live snapshot at delivery time (KTD6).
    private static DeliveryContext DeriveContext(TreeSnapshot snapshot, PaneLeaf leaf, SurfaceId surface, bool appFocused)
    {
        bool focused = snapshot.FocusedSurface == surface;
        bool visible = leaf.Selected == surface
            && (snapshot.ZoomedPane is null || snapshot.ZoomedPane == leaf.Id);
        return new DeliveryContext(appFocused, focused, visible);
    }
}
