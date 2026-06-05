using System;
using System.Collections.Generic;
using System.Collections.Immutable;

namespace Cmux.Core;

/// <summary>
/// A value-type snapshot of one tab's header (KTD5): everything the tab strip needs to draw a chip,
/// captured by value so the view never holds a reference to a live, mutating surface object
/// (issue-#2586 discipline). Recomputed on the UI thread from the controller snapshot plus the
/// surface's last-seen title.
/// </summary>
public readonly record struct TabHeaderDto(SurfaceId Id, string Title, bool IsSelected);

/// <summary>
/// Pure projection from model state (a pane's tab list + selected surface) to the immutable header
/// row the tab strip renders. Kept in <c>Cmux.Core</c> so it is unit-testable without WinUI.
/// </summary>
public static class TabHeaderProjection
{
    /// <summary>
    /// Project <paramref name="tabs"/> into headers, marking exactly the one equal to
    /// <paramref name="selected"/>. <paramref name="titleOf"/> supplies the live title for a surface
    /// (from engine OSC events); when it returns null/empty — or is itself null — the surface id is
    /// used as a stable placeholder so a freshly-spawned tab still shows a label.
    /// </summary>
    public static ImmutableArray<TabHeaderDto> Project(
        IReadOnlyList<SurfaceId> tabs,
        SurfaceId selected,
        Func<SurfaceId, string?>? titleOf = null)
    {
        if (tabs.Count == 0)
        {
            return ImmutableArray<TabHeaderDto>.Empty;
        }

        ImmutableArray<TabHeaderDto>.Builder builder = ImmutableArray.CreateBuilder<TabHeaderDto>(tabs.Count);
        foreach (SurfaceId id in tabs)
        {
            string? title = titleOf?.Invoke(id);
            if (string.IsNullOrEmpty(title))
            {
                title = id.ToString();
            }
            builder.Add(new TabHeaderDto(id, title, id == selected));
        }
        return builder.MoveToImmutable();
    }
}
