using System;
using System.Collections.Generic;

namespace Cmux.Core;

/// <summary>
/// Identity of a workspace — one sidebar row, owning one split tree of tabbed terminal panes.
/// Monotonic and never reused within a session, like the Phase-2 ids (KTD6).
/// </summary>
public readonly record struct WorkspaceId(int Value)
{
    public override string ToString() => $"W{Value}";
}

/// <summary>Git state reported for a workspace via the Phase-4 pipe (<c>report_git_branch</c>).</summary>
public readonly record struct GitBranchInfo(string Branch, bool IsDirty);

/// <summary>
/// Pull-request state reported via the Phase-4 pipe (<c>report_pr</c>). Mirrors what crosses the
/// <see cref="ISocketEffects"/> boundary (the wire's <c>url</c> param is dropped there today; add it
/// here when a row gains click-through).
/// </summary>
public readonly record struct PullRequestInfo(
    string Number,
    string Label,
    string Status,
    string? Branch,
    bool IsStale);

/// <summary>
/// One workspace (plan Phase 5 U1, ported from macOS <c>Workspace</c>): owns its split tree (the
/// Phase-2 controller) plus the sidebar-facing metadata — title, working directory, git branch, PR
/// status, agent status entries, and progress. The metadata is <b>not computed by the app</b>: it
/// arrives over the Phase-4 socket (<c>report_git_branch</c> / <c>report_pr</c> / <c>report_pwd</c> /
/// <c>set-status</c>) from shell integration and agent hooks, and from engine title events.
///
/// <para>Pure Core, single-threaded by convention (KTD3): all mutation happens on the UI thread.
/// Mutators raise <see cref="Changed"/> so the sidebar can recompute its row snapshot; the view
/// never binds to this object directly (issue-#2586 discipline — see <c>SidebarProjection</c>).</para>
/// </summary>
public sealed class Workspace
{
    private readonly Dictionary<string, string> _statusEntries = new(StringComparer.Ordinal);

    public Workspace(WorkspaceId id, SplitTreeController controller)
    {
        Id = id;
        Controller = controller ?? throw new ArgumentNullException(nameof(controller));
    }

    public WorkspaceId Id { get; }

    /// <summary>The workspace's split tree (Phase 2). The view plane renders it; the manager routes to it.</summary>
    public SplitTreeController Controller { get; }

    /// <summary>Raised after any metadata mutation (never by controller/tree changes — those have their own events).</summary>
    public event Action? Changed;

    /// <summary>Derived title — the focused surface's last-seen shell title (pushed in by the view plane).</summary>
    public string Title { get; private set; } = "";

    /// <summary>User-assigned title; when set it wins over <see cref="Title"/> in the sidebar.</summary>
    public string? CustomTitle { get; private set; }

    /// <summary>The title the sidebar shows: custom &gt; derived &gt; the workspace id.</summary>
    public string DisplayTitle =>
        !string.IsNullOrWhiteSpace(CustomTitle) ? CustomTitle!
        : !string.IsNullOrWhiteSpace(Title) ? Title
        : Id.ToString();

    /// <summary>Working directory, reported via <c>report_pwd</c>.</summary>
    public string? CurrentDirectory { get; private set; }

    /// <summary>Git branch + dirty flag, reported via <c>report_git_branch</c>. Null until first report.</summary>
    public GitBranchInfo? GitBranch { get; private set; }

    /// <summary>PR state, reported via <c>report_pr</c>. Null until first report.</summary>
    public PullRequestInfo? PullRequest { get; private set; }

    /// <summary>Agent progress string (<c>set-progress</c>), e.g. "3/5". Null when idle.</summary>
    public string? Progress { get; private set; }

    /// <summary>
    /// The "watch git status" gate (plan Phase 5 U2): when off, git/PR reports for this workspace are
    /// dropped, letting a user silence a noisy repo without uninstalling shell integration.
    /// </summary>
    public bool WatchGitStatus { get; set; } = true;

    /// <summary>Agent status entries keyed by agent (<c>set-status</c> "key:value", e.g. "claude" → "busy").</summary>
    public IReadOnlyDictionary<string, string> StatusEntries => _statusEntries;

    public void SetTitle(string title)
    {
        if (Title == title)
        {
            return;
        }
        Title = title;
        Changed?.Invoke();
    }

    public void SetCustomTitle(string? customTitle)
    {
        string? normalized = string.IsNullOrWhiteSpace(customTitle) ? null : customTitle;
        if (CustomTitle == normalized)
        {
            return;
        }
        CustomTitle = normalized;
        Changed?.Invoke();
    }

    public void ReportWorkingDirectory(string path)
    {
        if (CurrentDirectory == path)
        {
            return;
        }
        CurrentDirectory = path;
        Changed?.Invoke();
    }

    /// <summary>Record a git report; dropped when <see cref="WatchGitStatus"/> is off.</summary>
    public void ReportGitBranch(GitBranchInfo info)
    {
        if (!WatchGitStatus || GitBranch == info)
        {
            return;
        }
        GitBranch = info;
        Changed?.Invoke();
    }

    /// <summary>Record a PR report; dropped when <see cref="WatchGitStatus"/> is off.</summary>
    public void ReportPullRequest(PullRequestInfo info)
    {
        if (!WatchGitStatus || PullRequest == info)
        {
            return;
        }
        PullRequest = info;
        Changed?.Invoke();
    }

    /// <summary>
    /// Apply a <c>set-status</c> payload. The hook convention is <c>key:value</c> (e.g.
    /// "claude:busy"); a bare value uses an empty key. An empty value clears the entry.
    /// </summary>
    public void SetStatus(string status)
    {
        int sep = status.IndexOf(':');
        string key = sep >= 0 ? status[..sep] : "";
        string value = sep >= 0 ? status[(sep + 1)..] : status;

        if (string.IsNullOrEmpty(value))
        {
            if (!_statusEntries.Remove(key))
            {
                return;
            }
        }
        else
        {
            if (_statusEntries.TryGetValue(key, out string? existing) && existing == value)
            {
                return;
            }
            _statusEntries[key] = value;
        }
        Changed?.Invoke();
    }

    public void SetProgress(string? progress)
    {
        string? normalized = string.IsNullOrEmpty(progress) ? null : progress;
        if (Progress == normalized)
        {
            return;
        }
        Progress = normalized;
        Changed?.Invoke();
    }
}
