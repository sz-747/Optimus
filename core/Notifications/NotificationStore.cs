using System;
using System.Collections.Generic;
using System.Linq;

namespace Optimus.Core;

/// <summary>
/// The single owner of recorded notifications (plan Phase 3 U3) — the notification-plane analog of
/// <see cref="SurfaceManager"/>. Keeps one ordered, newest-first list and rebuilds its derived
/// indexes on every mutation. Two invariants mirror the macOS store: <b>at most one live
/// notification per surface</b> (a new one for a surface replaces the old), and a <b>latest-per-pane
/// </b> view that survives marking-read (it feeds the Phase 5 sidebar's "most recent" row).
///
/// <para>Pure Core: holds no WinUI/engine types and never touches a thread primitive — all callers
/// run on the UI thread (KTD3), so the recompute-on-mutation approach is both correct and cheap at
/// the low notification rate.</para>
/// </summary>
public sealed class NotificationStore
{
    // Backing list, newest first. Small (one entry per surface with a live notification), so a full
    // reindex on mutation is cheaper than maintaining incremental deltas and far easier to trust.
    private readonly List<TerminalNotification> _items = new();

    private Dictionary<PaneId, int> _unreadCountByPane = new();
    private Dictionary<PaneId, TerminalNotification> _latestByPane = new();
    private HashSet<(PaneId Pane, SurfaceId Surface)> _unreadByPaneSurface = new();
    private HashSet<SurfaceId> _unreadSurfaces = new();

    /// <summary>Every recorded notification, newest first.</summary>
    public IReadOnlyList<TerminalNotification> Items => _items;

    /// <summary>Total unread across all panes/surfaces.</summary>
    public int UnreadCount { get; private set; }

    /// <summary>Unread count per pane (panes with zero unread are absent).</summary>
    public IReadOnlyDictionary<PaneId, int> UnreadCountByPane => _unreadCountByPane;

    /// <summary>Newest notification per pane regardless of read state (the Phase 5 sidebar feed).</summary>
    public IReadOnlyDictionary<PaneId, TerminalNotification> LatestByPane => _latestByPane;

    /// <summary>The set of currently-unread <c>(pane, surface)</c> keys (badge source for U7).</summary>
    public IReadOnlySet<(PaneId Pane, SurfaceId Surface)> UnreadByPaneSurface => _unreadByPaneSurface;

    /// <summary>Whether <paramref name="surface"/> currently has an unread notification (U7 badge probe).</summary>
    public bool IsSurfaceUnread(SurfaceId surface) => _unreadSurfaces.Contains(surface);

    /// <summary>
    /// Record <paramref name="n"/>, replacing any existing notification for the same surface so each
    /// surface holds at most one (macOS parity). The new entry becomes the newest.
    /// </summary>
    public void Add(TerminalNotification n)
    {
        _items.RemoveAll(x => x.SurfaceId == n.SurfaceId);
        _items.Insert(0, n);
        Reindex();
    }

    /// <summary>Mark the notification with <paramref name="id"/> read, if present.</summary>
    public void MarkRead(Guid id) => Mutate(id, n => n with { IsRead = true });

    /// <summary>Mark <paramref name="surface"/>'s notification (if any) read.</summary>
    public void MarkRead(SurfaceId surface) => MutateWhere(n => n.SurfaceId == surface, n => n with { IsRead = true });

    /// <summary>Mark every notification owned by <paramref name="pane"/> read.</summary>
    public void MarkReadForPane(PaneId pane) => MutateWhere(n => n.PaneId == pane, n => n with { IsRead = true });

    /// <summary>Mark the notification with <paramref name="id"/> unread, if present.</summary>
    public void MarkUnread(Guid id) => Mutate(id, n => n with { IsRead = false });

    /// <summary>Remove the notification with <paramref name="id"/>, if present.</summary>
    public void Remove(Guid id)
    {
        if (_items.RemoveAll(x => x.Id == id) > 0)
        {
            Reindex();
        }
    }

    /// <summary>Drop <paramref name="surface"/>'s notification (if any).</summary>
    public void ClearForSurface(SurfaceId surface)
    {
        if (_items.RemoveAll(x => x.SurfaceId == surface) > 0)
        {
            Reindex();
        }
    }

    /// <summary>Drop every notification owned by <paramref name="pane"/>.</summary>
    public void ClearForPane(PaneId pane)
    {
        if (_items.RemoveAll(x => x.PaneId == pane) > 0)
        {
            Reindex();
        }
    }

    /// <summary>Drop every notification.</summary>
    public void ClearAll()
    {
        if (_items.Count == 0)
        {
            return;
        }
        _items.Clear();
        Reindex();
    }

    private void Mutate(Guid id, Func<TerminalNotification, TerminalNotification> change)
    {
        int i = _items.FindIndex(x => x.Id == id);
        if (i >= 0)
        {
            _items[i] = change(_items[i]);
            Reindex();
        }
    }

    private void MutateWhere(Func<TerminalNotification, bool> match, Func<TerminalNotification, TerminalNotification> change)
    {
        bool any = false;
        for (int i = 0; i < _items.Count; i++)
        {
            if (match(_items[i]))
            {
                _items[i] = change(_items[i]);
                any = true;
            }
        }
        if (any)
        {
            Reindex();
        }
    }

    // Rebuild all derived indexes from the ordered list. _items is newest-first, so the first entry
    // seen for a pane is its latest.
    private void Reindex()
    {
        var unreadCountByPane = new Dictionary<PaneId, int>();
        var latestByPane = new Dictionary<PaneId, TerminalNotification>();
        var unreadByPaneSurface = new HashSet<(PaneId, SurfaceId)>();
        var unreadSurfaces = new HashSet<SurfaceId>();
        int unread = 0;

        foreach (TerminalNotification n in _items)
        {
            if (!latestByPane.ContainsKey(n.PaneId))
            {
                latestByPane[n.PaneId] = n;
            }
            if (!n.IsRead)
            {
                unread++;
                unreadCountByPane[n.PaneId] = unreadCountByPane.GetValueOrDefault(n.PaneId) + 1;
                unreadByPaneSurface.Add((n.PaneId, n.SurfaceId));
                unreadSurfaces.Add(n.SurfaceId);
            }
        }

        UnreadCount = unread;
        _unreadCountByPane = unreadCountByPane;
        _latestByPane = latestByPane;
        _unreadByPaneSurface = unreadByPaneSurface;
        _unreadSurfaces = unreadSurfaces;
    }
}
