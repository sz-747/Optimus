using System.Linq;
using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="NotificationStore"/> (plan Phase 3 U3): the ordered, newest-first record
/// list with at-most-one-live-notification-per-surface de-dupe and the derived indexes the UI and
/// the Phase 5 sidebar read (R2, R6). Pure Core — no UI, no engine.
/// </summary>
public sealed class NotificationStoreTests
{
    private static TerminalNotification N(int surface, int pane, string title = "t")
        => TerminalNotification.Create(new SurfaceId(surface), new PaneId(pane), title);

    [Fact] // Covers R6.
    public void Add_one_indexes_unread_and_latest()
    {
        var store = new NotificationStore();
        var n = N(1, 10, "build done");
        store.Add(n);

        Assert.Equal(1, store.UnreadCount);
        Assert.Equal(1, store.UnreadCountByPane[new PaneId(10)]);
        Assert.Equal(n.Id, store.LatestByPane[new PaneId(10)].Id);
        Assert.True(store.IsSurfaceUnread(new SurfaceId(1)));
        Assert.Contains((new PaneId(10), new SurfaceId(1)), store.UnreadByPaneSurface);
    }

    [Fact] // At most one live notification per surface (macOS parity).
    public void Add_second_for_same_surface_replaces_first()
    {
        var store = new NotificationStore();
        store.Add(N(1, 10, "first"));
        store.Add(N(1, 10, "second"));

        Assert.Single(store.Items);
        Assert.Equal("second", store.Items[0].Title);
        Assert.Equal(1, store.UnreadCount);
    }

    [Fact] // Covers R6.
    public void Add_two_surfaces_same_pane_counts_both_latest_is_newer()
    {
        var store = new NotificationStore();
        store.Add(N(1, 10, "older"));
        var newer = N(2, 10, "newer");
        store.Add(newer);

        Assert.Equal(2, store.UnreadCountByPane[new PaneId(10)]);
        Assert.Equal(newer.Id, store.LatestByPane[new PaneId(10)].Id);
    }

    [Fact] // Covers R6.
    public void MarkRead_by_id_drops_unread_but_keeps_in_latest()
    {
        var store = new NotificationStore();
        var n = N(1, 10);
        store.Add(n);
        store.MarkRead(n.Id);

        Assert.Equal(0, store.UnreadCount);
        Assert.True(store.LatestByPane.ContainsKey(new PaneId(10)));
        Assert.True(store.LatestByPane[new PaneId(10)].IsRead);
        Assert.False(store.IsSurfaceUnread(new SurfaceId(1)));
    }

    [Fact] // Covers R6.
    public void MarkRead_by_surface_and_pane_clear_only_their_scope()
    {
        var store = new NotificationStore();
        store.Add(N(1, 10));
        store.Add(N(2, 10));
        store.Add(N(3, 20));

        store.MarkRead(new SurfaceId(1));
        Assert.False(store.IsSurfaceUnread(new SurfaceId(1)));
        Assert.True(store.IsSurfaceUnread(new SurfaceId(2)));  // same pane untouched
        Assert.True(store.IsSurfaceUnread(new SurfaceId(3)));

        store.MarkReadForPane(new PaneId(10));
        Assert.False(store.IsSurfaceUnread(new SurfaceId(2))); // pane 10 now fully read
        Assert.True(store.IsSurfaceUnread(new SurfaceId(3)));  // pane 20 untouched
    }

    [Fact] // Covers R6.
    public void UnreadByPaneSurface_tracks_membership()
    {
        var store = new NotificationStore();
        var n = N(1, 10);
        store.Add(n);
        Assert.Contains((new PaneId(10), new SurfaceId(1)), store.UnreadByPaneSurface);

        store.MarkRead(n.Id);
        Assert.DoesNotContain((new PaneId(10), new SurfaceId(1)), store.UnreadByPaneSurface);
    }

    [Fact]
    public void MarkUnread_restores_unread()
    {
        var store = new NotificationStore();
        var n = N(1, 10);
        store.Add(n);
        store.MarkRead(n.Id);
        store.MarkUnread(n.Id);

        Assert.Equal(1, store.UnreadCount);
        Assert.True(store.IsSurfaceUnread(new SurfaceId(1)));
    }

    [Fact]
    public void Remove_and_clear_scopes_prune()
    {
        var store = new NotificationStore();
        var a = N(1, 10);
        store.Add(a);
        store.Add(N(2, 10));
        store.Add(N(3, 20));

        store.Remove(a.Id);
        Assert.Equal(2, store.Items.Count);
        Assert.False(store.IsSurfaceUnread(new SurfaceId(1)));

        store.ClearForSurface(new SurfaceId(2));
        Assert.False(store.IsSurfaceUnread(new SurfaceId(2)));
        Assert.Single(store.Items);

        store.Add(N(4, 20));
        store.ClearForPane(new PaneId(20));
        Assert.Empty(store.Items);  // both pane-20 entries gone

        store.Add(N(5, 30));
        store.ClearAll();
        Assert.Empty(store.Items);
        Assert.Equal(0, store.UnreadCount);
    }

    [Fact]
    public void Newest_first_ordering_preserved_through_mutations()
    {
        var store = new NotificationStore();
        store.Add(N(1, 10, "a"));
        store.Add(N(2, 10, "b"));
        store.Add(N(3, 10, "c"));   // newest

        Assert.Equal(new[] { "c", "b", "a" }, store.Items.Select(i => i.Title).ToArray());

        store.MarkRead(store.Items[1].Id);  // mark "b" read — order unchanged
        Assert.Equal(new[] { "c", "b", "a" }, store.Items.Select(i => i.Title).ToArray());

        store.Remove(store.Items[0].Id);    // remove "c"
        Assert.Equal(new[] { "b", "a" }, store.Items.Select(i => i.Title).ToArray());
    }
}
