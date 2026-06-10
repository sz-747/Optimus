using Optimus.Core;
using Xunit;

namespace Optimus.Core.Tests;

/// <summary>
/// Coverage for <see cref="NotificationPolicy"/> (plan Phase 3 U5): the default effect set, the
/// suppress-when-in-front-of-the-user rule (R4), and the Phase 4 override seam (R7). This is the
/// reduced port — the macOS external shell-hook pipeline is out of scope (KTD9); the seam is the
/// only hook into that future.
/// </summary>
public sealed class NotificationPolicyTests
{
    private static readonly SurfaceNotification Req = new("title", "", "body");

    [Fact] // Covers R7.
    public void Default_policy_enables_all_effects_when_not_in_front()
    {
        var policy = new NotificationPolicy();
        NotificationEffects effects = policy.Decide(
            Req, new DeliveryContext(AppFocused: true, SurfaceFocused: false, SurfaceVisible: true));

        Assert.True(effects.Record);
        Assert.True(effects.MarkUnread);
        Assert.True(effects.Desktop);
        Assert.True(effects.Sound);
        Assert.True(effects.PaneFlash);
        Assert.True(effects.ReorderWorkspace);
    }

    [Fact] // Covers R4, AE2.
    public void In_front_of_user_suppresses_toast_and_marks_read()
    {
        var policy = new NotificationPolicy();
        NotificationEffects effects = policy.Decide(
            Req, new DeliveryContext(AppFocused: true, SurfaceFocused: true, SurfaceVisible: true));

        Assert.False(effects.Desktop);   // no OS toast
        Assert.False(effects.MarkUnread); // recorded as already-read
        Assert.True(effects.Record);      // but still recorded
        Assert.True(effects.PaneFlash);   // flash is still allowed
    }

    [Fact] // Covers R4 boundary.
    public void Backgrounded_app_still_toasts_even_if_surface_focused()
    {
        var policy = new NotificationPolicy();
        NotificationEffects effects = policy.Decide(
            Req, new DeliveryContext(AppFocused: false, SurfaceFocused: true, SurfaceVisible: true));

        Assert.True(effects.Desktop);
        Assert.True(effects.MarkUnread);
    }

    [Fact] // Covers R4 boundary.
    public void Focused_but_not_visible_is_not_suppressed()
    {
        var policy = new NotificationPolicy();
        NotificationEffects effects = policy.Decide(
            Req, new DeliveryContext(AppFocused: true, SurfaceFocused: true, SurfaceVisible: false));

        Assert.True(effects.Desktop);
    }

    [Fact] // The Phase 4 hook-injection seam.
    public void Override_seam_can_disable_record()
    {
        var policy = new NotificationPolicy(overrides: _ => new NotificationEffects(Record: false));
        NotificationEffects effects = policy.Decide(Req, new DeliveryContext(false, false, false));

        Assert.False(effects.Record);
    }

    [Fact] // Covers R7.
    public void Custom_defaults_flow_through_when_not_suppressed()
    {
        var policy = new NotificationPolicy(new NotificationEffects(Sound: false, PaneFlash: false));
        NotificationEffects effects = policy.Decide(Req, new DeliveryContext(false, false, false));

        Assert.False(effects.Sound);
        Assert.False(effects.PaneFlash);
        Assert.True(effects.Desktop);
    }
}
