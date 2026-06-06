using System.Collections.Generic;
using System.Collections.Immutable;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Coverage for <see cref="NotificationCoordinator"/> (plan Phase 3 U6) — the integration seam that
/// makes the whole feature provable in Core (R2, R3, R4, R5, R6). Every branch (suppress, orphan
/// drop, coalesce, read-on-focus, record-off) is exercised against value <see cref="TreeSnapshot"/>s,
/// with no UI, dispatcher, or live window.
/// </summary>
public sealed class NotificationCoordinatorTests
{
    private static readonly SurfaceId SA = new(1);
    private static readonly SurfaceId SB = new(2);
    private static readonly PaneId PA = new(1);
    private static readonly PaneId PB = new(2);

    // Two side-by-side panes, each with one surface; pane A focused, nothing zoomed.
    private static TreeSnapshot TwoPanes(PaneId focused, PaneId? zoomed = null)
    {
        var paneA = new PaneLeaf(PA, ImmutableList.Create(SA), SA);
        var paneB = new PaneLeaf(PB, ImmutableList.Create(SB), SB);
        var root = new SplitBranch(new BranchId(1), Orientation.Vertical, paneA, paneB, 0.5);
        return new TreeSnapshot(root, focused, zoomed, 1);
    }

    private static SurfaceNotification P(string title) => new(title, "");

    [Fact] // Covers R2, R3, AE1.
    public void Notification_on_unfocused_surface_records_unread_and_requests_flash_and_toast()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        TreeSnapshot snap = TwoPanes(focused: PA);
        coord.OnNotification(SB, P("build done"));   // SB is unfocused (A is focused)
        coord.Drain(snap, appFocused: true);

        Assert.True(coord.IsSurfaceUnread(SB));
        Assert.Single(surfaced);
        Assert.True(surfaced[0].ShowToast);
        Assert.True(surfaced[0].Flash);
        Assert.False(surfaced[0].Notification.IsRead);
    }

    [Fact] // Covers R4, AE2.
    public void Notification_on_focused_visible_surface_records_read_and_no_toast()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        TreeSnapshot snap = TwoPanes(focused: PA);
        coord.OnNotification(SA, P("done"));          // SA is the focused, visible surface
        coord.Drain(snap, appFocused: true);

        Assert.False(coord.IsSurfaceUnread(SA));       // recorded as read
        Assert.Single(coord.Store.Items);
        Assert.True(coord.Store.Items[0].IsRead);
        Assert.Single(surfaced);
        Assert.False(surfaced[0].ShowToast);           // suppressed
    }

    [Fact] // Covers R4 boundary.
    public void Notification_while_app_backgrounded_requests_toast_even_on_focused_surface()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        TreeSnapshot snap = TwoPanes(focused: PA);
        coord.OnNotification(SA, P("done"));
        coord.Drain(snap, appFocused: false);          // app not foreground

        Assert.True(surfaced[0].ShowToast);
        Assert.True(coord.IsSurfaceUnread(SA));
    }

    [Fact] // Covers AE4.
    public void Rapid_repeats_for_one_surface_coalesce_to_one()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        TreeSnapshot snap = TwoPanes(focused: PA);
        coord.OnNotification(SB, P("a"));
        coord.OnNotification(SB, P("b"));
        coord.OnNotification(SB, P("c"));
        coord.Drain(snap, appFocused: true);

        Assert.Single(surfaced);
        Assert.Equal("c", surfaced[0].Notification.Title);
        Assert.Single(coord.Store.Items);
    }

    [Fact] // Covers AE5.
    public void Notification_for_removed_surface_is_dropped_before_delivery()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        coord.OnNotification(SB, P("orphan"));
        // SB's tab closed before the drain: a snapshot with only pane A / surface A.
        var onlyA = new PaneLeaf(PA, ImmutableList.Create(SA), SA);
        var snap = new TreeSnapshot(onlyA, PA, null, 2);
        bool more = coord.Drain(snap, appFocused: true);

        Assert.Empty(surfaced);
        Assert.Empty(coord.Store.Items);
        Assert.False(more);
    }

    [Fact] // Covers R6, AE6.
    public void Focus_change_marks_focused_surface_read_and_clears_its_flash()
    {
        var coord = new NotificationCoordinator();
        var flashCleared = new List<PaneId>();
        coord.FlashCleared += flashCleared.Add;

        // Notification on unfocused SB while A is focused -> unread.
        coord.OnNotification(SB, P("done"));
        coord.Drain(TwoPanes(focused: PA), appFocused: true);
        Assert.True(coord.IsSurfaceUnread(SB));

        // Now focus pane B (its selected surface is SB).
        coord.OnFocusChanged(TwoPanes(focused: PB));

        Assert.False(coord.IsSurfaceUnread(SB));
        Assert.Contains(PB, flashCleared);
    }

    [Fact] // Record == false: nothing stored, but flash/toast still surface.
    public void Record_off_policy_surfaces_effects_without_recording()
    {
        var policy = new NotificationPolicy(overrides: _ => new NotificationEffects(Record: false));
        var coord = new NotificationCoordinator(policy);
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        coord.OnNotification(SB, P("ephemeral"));
        coord.Drain(TwoPanes(focused: PA), appFocused: true);

        Assert.Empty(coord.Store.Items);     // not recorded
        Assert.Single(surfaced);             // but still surfaced
        Assert.True(surfaced[0].ShowToast);
        Assert.True(surfaced[0].Flash);
    }

    [Fact] // KTD6 — a notification for a surface hidden behind a zoom on another pane is not "visible".
    public void Surface_hidden_by_zoom_on_another_pane_is_not_suppressed()
    {
        var coord = new NotificationCoordinator();
        var surfaced = new List<SurfacedNotification>();
        coord.Surfaced += surfaced.Add;

        // Pane A focused but pane B is zoomed full-screen, hiding A.
        TreeSnapshot snap = TwoPanes(focused: PA, zoomed: PB);
        coord.OnNotification(SA, P("done"));
        coord.Drain(snap, appFocused: true);

        Assert.True(surfaced[0].ShowToast);  // A not visible -> not suppressed
        Assert.True(coord.IsSurfaceUnread(SA));
    }
}
