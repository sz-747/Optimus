using System;
using System.Collections.Generic;
using System.Linq;

namespace Cmux.Core;

/// <summary>
/// The single owner of workspaces (plan Phase 5): an ordered list with one selected, the workspace
/// analog of <see cref="SurfaceManager"/>. All workspace controllers share one
/// <see cref="IdAllocator"/>, so a bare <see cref="SurfaceId"/> (e.g. <c>CMUX_SURFACE_ID</c> from
/// the Phase-4 pipe) resolves to exactly one workspace via <see cref="FindBySurface"/> — that is
/// how surface-keyed <c>report_*</c> commands land on the right sidebar row.
///
/// <para>Invariant: never empty (the workspace analog of R6) — closing the last workspace
/// immediately seeds a fresh one, so the window never goes contentless. Pure Core, UI-thread only
/// (KTD3).</para>
/// </summary>
public sealed class WorkspaceManager
{
    private readonly IdAllocator _ids = new();
    private readonly List<Workspace> _workspaces = new();
    private int _nextWorkspace;

    /// <summary>Raised when the list or selection changes; workspace metadata changes re-raise it too.</summary>
    public event Action? Changed;

    /// <summary>Raised when a workspace is created (list + selection already updated). The host builds its view.</summary>
    public event Action<Workspace>? WorkspaceCreated;

    /// <summary>Raised when a workspace is removed. The host tears down its view and engines.</summary>
    public event Action<Workspace>? WorkspaceClosed;

    /// <summary>Seed with one workspace, selected — mirrors the controller's seeded first pane.</summary>
    public WorkspaceManager()
    {
        Workspace seed = NewWorkspaceCore();
        SelectedId = seed.Id;
    }

    /// <summary>The workspaces in sidebar order.</summary>
    public IReadOnlyList<Workspace> Workspaces => _workspaces;

    /// <summary>The selected workspace's id.</summary>
    public WorkspaceId SelectedId { get; private set; }

    /// <summary>The selected workspace (always present — the list is never empty).</summary>
    public Workspace Selected => _workspaces.First(w => w.Id == SelectedId);

    /// <summary>Create a workspace at the end of the list and select it.</summary>
    public Workspace NewWorkspace()
    {
        Workspace workspace = NewWorkspaceCore();
        SelectedId = workspace.Id;
        WorkspaceCreated?.Invoke(workspace);
        Changed?.Invoke();
        return workspace;
    }

    /// <summary>
    /// Close <paramref name="id"/>. Selection moves to the nearest neighbor; closing the last
    /// workspace seeds a replacement first (never-empty invariant). No-op for unknown ids.
    /// </summary>
    public void CloseWorkspace(WorkspaceId id)
    {
        int index = _workspaces.FindIndex(w => w.Id == id);
        if (index < 0)
        {
            return;
        }

        Workspace closing = _workspaces[index];

        Workspace? seeded = null;
        if (_workspaces.Count == 1)
        {
            seeded = NewWorkspaceCore();
        }

        _workspaces.RemoveAt(index);
        if (SelectedId == id)
        {
            SelectedId = _workspaces[Math.Min(index, _workspaces.Count - 1)].Id;
        }

        if (seeded is not null)
        {
            WorkspaceCreated?.Invoke(seeded);
        }
        WorkspaceClosed?.Invoke(closing);
        Changed?.Invoke();
    }

    /// <summary>Select <paramref name="id"/>. No-op for unknown ids.</summary>
    public void SelectWorkspace(WorkspaceId id)
    {
        if (SelectedId == id || _workspaces.All(w => w.Id != id))
        {
            return;
        }
        SelectedId = id;
        Changed?.Invoke();
    }

    /// <summary>The workspace with <paramref name="id"/>, or null.</summary>
    public Workspace? Find(WorkspaceId id) => _workspaces.FirstOrDefault(w => w.Id == id);

    /// <summary>
    /// The workspace whose tree currently holds <paramref name="surface"/>, or null. Unambiguous
    /// because every controller mints from the shared allocator (ids are never reused).
    /// </summary>
    public Workspace? FindBySurface(SurfaceId surface) =>
        _workspaces.FirstOrDefault(w => w.Controller.AllSurfaces.Contains(surface));

    // ---- Surface-keyed report routing (the Phase-4 → sidebar seam) ----------------------------

    /// <summary>Route <c>report_git_branch</c> to the workspace owning <paramref name="surface"/>.</summary>
    public void ReportGitBranch(SurfaceId surface, string branch, bool isDirty) =>
        FindBySurface(surface)?.ReportGitBranch(new GitBranchInfo(branch, isDirty));

    /// <summary>Route <c>report_pr</c> to the workspace owning <paramref name="surface"/>.</summary>
    public void ReportPr(SurfaceId surface, string number, string label, string status, string? branch, bool isStale) =>
        FindBySurface(surface)?.ReportPullRequest(new PullRequestInfo(number, label, status, branch, isStale));

    /// <summary>Route <c>report_pwd</c> to the workspace owning <paramref name="surface"/>.</summary>
    public void ReportPwd(SurfaceId surface, string path) =>
        FindBySurface(surface)?.ReportWorkingDirectory(path);

    private Workspace NewWorkspaceCore()
    {
        var workspace = new Workspace(new WorkspaceId(++_nextWorkspace), new SplitTreeController(_ids));
        workspace.Changed += () => Changed?.Invoke();
        _workspaces.Add(workspace);
        return workspace;
    }
}
