using System;

namespace Cmux.Core;

/// <summary>
/// What a notification click should do. Phase 3 only ever focuses the originating surface (which is
/// derived from <see cref="TerminalNotification.SurfaceId"/>, so a null <c>ClickAction</c> means
/// exactly that). The enum exists so a later phase can add richer actions (e.g. reveal-in-Explorer)
/// behind the same field without reshaping the record (see plan Scope Boundaries).
/// </summary>
public enum NotificationClickKind
{
    FocusSurface = 0,
    RevealInExplorer = 1,
}

/// <summary>An optional click behavior carried on a <see cref="TerminalNotification"/>.</summary>
public readonly record struct NotificationClickAction(NotificationClickKind Kind, string? Argument = null);

/// <summary>
/// One recorded terminal notification (plan Phase 3 U3): the raw engine
/// <see cref="SurfaceNotification"/> (title/body) enriched with identity, the owning pane/surface,
/// a timestamp, and read/effect state. A <c>readonly record struct</c> like the Phase 2 ids — value
/// semantics make the store trivially testable and let mutations be expressed as <c>with</c>
/// rewrites. Note the Phase 2 model has <b>no TabId</b>: a tab <i>is</i> a surface, so
/// <see cref="SurfaceId"/> is the tab key and <see cref="PaneId"/> is the owning pane (resolved from
/// the snapshot at record time by the coordinator, U6).
/// </summary>
public readonly record struct TerminalNotification(
    Guid Id,
    SurfaceId SurfaceId,
    PaneId PaneId,
    string Title,
    string Subtitle,
    string Body,
    DateTimeOffset CreatedAt,
    bool IsRead,
    bool PaneFlash,
    NotificationClickAction? ClickAction)
{
    /// <summary>
    /// Build a fresh, unread notification with a new id and a current timestamp. The store orders by
    /// insertion, not <see cref="CreatedAt"/>, so tests need not control the clock; callers that do
    /// care can pass <paramref name="createdAt"/>.
    /// </summary>
    public static TerminalNotification Create(
        SurfaceId surface,
        PaneId pane,
        string title,
        string subtitle = "",
        string body = "",
        bool paneFlash = true,
        bool isRead = false,
        NotificationClickAction? clickAction = null,
        DateTimeOffset? createdAt = null)
        => new(
            Guid.NewGuid(),
            surface,
            pane,
            title,
            subtitle,
            body,
            createdAt ?? DateTimeOffset.UtcNow,
            isRead,
            paneFlash,
            clickAction);
}
