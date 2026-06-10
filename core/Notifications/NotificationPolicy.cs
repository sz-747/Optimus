using System;

namespace Optimus.Core;

/// <summary>
/// What a notification should <i>do</i> (plan Phase 3 U5, R7). A <c>record</c> (class, deliberately —
/// a positional <c>record struct</c> with defaulted bools would zero-init to all-false via
/// <c>new()</c>/<c>default</c>, the opposite of intent) so <c>new NotificationEffects()</c> yields the
/// all-enabled default and <c>with</c> rewrites express the suppression rule.
/// </summary>
public sealed record NotificationEffects(
    bool Record = true,
    bool MarkUnread = true,
    bool Desktop = true,
    bool Sound = true,
    bool PaneFlash = true,
    bool ReorderWorkspace = true);

/// <summary>
/// Whether the originating surface is in front of the user right now (plan Phase 3 U5, KTD6). All
/// three must hold for the toast to be suppressed: the app window is foreground, the surface is the
/// focused one, and it is actually visible (its tab is selected and no other pane is zoomed over it).
/// Re-derived from the live snapshot at delivery time by the coordinator (U6).
/// </summary>
public readonly record struct DeliveryContext(bool AppFocused, bool SurfaceFocused, bool SurfaceVisible);

/// <summary>
/// Decides the <see cref="NotificationEffects"/> for a notification (plan Phase 3 U5). This is the
/// <b>reduced</b> port of the macOS policy: static defaults plus the suppress-when-in-front rule. The
/// macOS external shell-hook pipeline (process spawn, JSON patch/merge, trust authorization) is out
/// of scope (KTD9); the <paramref name="overrides"/> seam is the single hook a Phase 4
/// hook-derived-effects implementation will plug into without reshaping callers.
/// </summary>
public sealed class NotificationPolicy
{
    private readonly NotificationEffects _defaults;
    private readonly Func<SurfaceNotification, NotificationEffects>? _overrides;

    public NotificationPolicy(
        NotificationEffects? defaults = null,
        Func<SurfaceNotification, NotificationEffects>? overrides = null)
    {
        _defaults = defaults ?? new NotificationEffects();
        _overrides = overrides;
    }

    /// <summary>
    /// Resolve the effects for <paramref name="request"/> given the current delivery context. Starts
    /// from the override seam (if any) or the defaults, then forces <c>Desktop</c> and
    /// <c>MarkUnread</c> off when the surface is already in front of the user (R4): the notification
    /// is still recorded, but as already-read and with no OS toast.
    /// </summary>
    public NotificationEffects Decide(SurfaceNotification request, DeliveryContext context)
    {
        NotificationEffects effects = _overrides?.Invoke(request) ?? _defaults;
        if (IsInFrontOfUser(context))
        {
            effects = effects with { Desktop = false, MarkUnread = false };
        }
        return effects;
    }

    private static bool IsInFrontOfUser(DeliveryContext c)
        => c.AppFocused && c.SurfaceFocused && c.SurfaceVisible;
}
