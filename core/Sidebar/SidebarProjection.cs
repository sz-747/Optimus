using System;
using System.Collections.Immutable;
using System.Linq;

namespace Cmux.Core;

/// <summary>
/// A value-type snapshot of one sidebar row (plan Phase 5 U3 — the issue-#2586 snapshot-boundary
/// rule): everything the sidebar needs to draw a workspace row, captured by value so the view never
/// holds a reference to a live, mutating <see cref="Workspace"/> (binding rows to observables caused
/// a 100% CPU spin on macOS). The Windows analog of <c>SidebarWorkspaceRenderItem</c>; recomputed on
/// the UI thread on every change, like <see cref="TabHeaderDto"/>.
/// </summary>
public readonly record struct SidebarRowDto(
    WorkspaceId Id,
    string Title,
    bool IsSelected,
    string? GitBranch,
    bool GitDirty,
    string? PrBadge,
    string? PrStatus,
    string? Cwd,
    string? Status,
    string? Progress,
    string? LatestText,
    int UnreadCount);

/// <summary>
/// Pure projection from the workspace list to the immutable rows the sidebar renders. Kept in
/// <c>Cmux.Core</c> so it is unit-testable without WinUI, like <see cref="TabHeaderProjection"/>.
/// </summary>
public static class SidebarProjection
{
    /// <summary>
    /// Project every workspace into a row. <paramref name="unreadOf"/> /
    /// <paramref name="latestOf"/> supply the per-workspace notification feed (each workspace's
    /// coordinator lives in the view plane); when null, rows project zero unread / no latest text.
    /// </summary>
    public static ImmutableArray<SidebarRowDto> Project(
        WorkspaceManager manager,
        Func<WorkspaceId, int>? unreadOf = null,
        Func<WorkspaceId, string?>? latestOf = null)
    {
        ImmutableArray<SidebarRowDto>.Builder builder =
            ImmutableArray.CreateBuilder<SidebarRowDto>(manager.Workspaces.Count);

        foreach (Workspace w in manager.Workspaces)
        {
            builder.Add(new SidebarRowDto(
                w.Id,
                w.DisplayTitle,
                IsSelected: w.Id == manager.SelectedId,
                GitBranch: w.GitBranch?.Branch,
                GitDirty: w.GitBranch?.IsDirty ?? false,
                PrBadge: PrBadge(w.PullRequest),
                PrStatus: w.PullRequest?.Status,
                Cwd: CompactPath(w.CurrentDirectory),
                Status: StatusSummary(w),
                Progress: w.Progress,
                LatestText: latestOf?.Invoke(w.Id),
                UnreadCount: unreadOf?.Invoke(w.Id) ?? 0));
        }

        return builder.MoveToImmutable();
    }

    /// <summary>"#42 open", or null when nothing was reported.</summary>
    private static string? PrBadge(PullRequestInfo? pr)
    {
        if (pr is not PullRequestInfo p || string.IsNullOrEmpty(p.Number))
        {
            return null;
        }
        string number = p.Number.StartsWith('#') ? p.Number : "#" + p.Number;
        return string.IsNullOrEmpty(p.Status) ? number : $"{number} {p.Status}";
    }

    /// <summary>"claude busy · codex idle", or null when no agent reported a status.</summary>
    private static string? StatusSummary(Workspace w)
    {
        if (w.StatusEntries.Count == 0)
        {
            return null;
        }
        return string.Join(" · ", w.StatusEntries
            .OrderBy(e => e.Key, StringComparer.Ordinal)
            .Select(e => string.IsNullOrEmpty(e.Key) ? e.Value : $"{e.Key} {e.Value}"));
    }

    /// <summary>Abbreviate the user-profile prefix to "~" so rows stay short (macOS parity).</summary>
    private static string? CompactPath(string? path)
    {
        if (string.IsNullOrEmpty(path))
        {
            return null;
        }
        string home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (home.Length > 0 && path.StartsWith(home, StringComparison.OrdinalIgnoreCase))
        {
            string rest = path[home.Length..];
            return rest.Length == 0 ? "~" : "~" + rest;
        }
        return path;
    }
}
