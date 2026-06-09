using System.Collections.Generic;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Coverage for <see cref="NotificationQueue"/> (plan Phase 3 U4) — the burst absorber: coalescing
/// per <c>(generation, surface)</c>, the max-16-per-drain cap, orphan-dropping via a caller-supplied
/// liveness probe, and the clear boundary (R5, AE4, AE5). The drain is a pure synchronous method, so
/// every branch is exercised here with no UI, dispatcher, or tree.
/// </summary>
public sealed class NotificationQueueTests
{
    private static SurfaceNotification P(string title) => new(title, "", "");

    [Fact] // Covers AE4.
    public void Coalesce_keeps_latest_for_same_surface()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("first"));
        q.Enqueue(new SurfaceId(1), P("second"));

        var delivered = new List<NotificationEntry>();
        bool more = q.Drain(_ => true, delivered.Add);

        Assert.False(more);
        Assert.Single(delivered);
        Assert.Equal("second", delivered[0].Payload.Title);
    }

    [Fact] // The CLI path (Phase 4) opts out of coalescing.
    public void No_coalesce_keeps_both()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("first"), coalesce: false);
        q.Enqueue(new SurfaceId(1), P("second"), coalesce: false);

        var delivered = new List<NotificationEntry>();
        q.Drain(_ => true, delivered.Add);

        Assert.Equal(2, delivered.Count);
    }

    [Fact] // Covers R5.
    public void Drain_caps_at_16_and_reports_more()
    {
        var q = new NotificationQueue();
        for (int i = 1; i <= 17; i++)
        {
            q.Enqueue(new SurfaceId(i), P($"n{i}"), coalesce: false);
        }

        var first = new List<NotificationEntry>();
        bool more = q.Drain(_ => true, first.Add);
        Assert.Equal(16, first.Count);
        Assert.True(more);

        var second = new List<NotificationEntry>();
        more = q.Drain(_ => true, second.Add);
        Assert.Single(second);
        Assert.False(more);
    }

    [Fact] // Covers AE5.
    public void Orphan_is_dropped_not_delivered()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("dead"));

        var delivered = new List<NotificationEntry>();
        bool more = q.Drain(_ => false, delivered.Add);  // nothing live

        Assert.Empty(delivered);
        Assert.False(more);
    }

    [Fact] // Covers R5.
    public void Mixed_live_and_orphan_delivers_only_live()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("live"));
        q.Enqueue(new SurfaceId(2), P("dead"));
        q.Enqueue(new SurfaceId(3), P("live"));

        var dead = new SurfaceId(2);
        var delivered = new List<NotificationEntry>();
        q.Drain(s => s != dead, delivered.Add);

        Assert.Equal(2, delivered.Count);
        Assert.DoesNotContain(delivered, e => e.Surface == dead);
    }

    [Fact] // Covers R5 — a clear is a true coalescing boundary.
    public void Clear_drops_pre_boundary_pending_but_keeps_post_boundary()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("old"));   // gen 0
        q.MarkClearBoundary();                    // gen -> 1
        q.Enqueue(new SurfaceId(1), P("new"));   // gen 1
        q.ClearPending(_ => true);                // drops only pre-boundary (gen < 1)

        var delivered = new List<NotificationEntry>();
        q.Drain(_ => true, delivered.Add);

        Assert.Single(delivered);
        Assert.Equal("new", delivered[0].Payload.Title);
    }

    [Fact]
    public void Clear_scope_drops_only_matching_surface()
    {
        var q = new NotificationQueue();
        q.Enqueue(new SurfaceId(1), P("s1"));
        q.Enqueue(new SurfaceId(2), P("s2"));
        q.MarkClearBoundary();
        q.ClearPending(s => s == new SurfaceId(1));

        var delivered = new List<NotificationEntry>();
        q.Drain(_ => true, delivered.Add);

        Assert.Single(delivered);
        Assert.Equal(new SurfaceId(2), delivered[0].Surface);
    }
}
