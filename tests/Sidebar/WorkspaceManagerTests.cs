using System.Linq;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Phase 5: workspace list/selection lifecycle, the never-empty invariant, surface→workspace
/// resolution over the shared id allocator, and routing of the Phase-4 surface-keyed reports
/// (git branch / PR / pwd) onto workspace metadata.
/// </summary>
public sealed class WorkspaceManagerTests
{
    // ---- Lifecycle -----------------------------------------------------------------------------

    [Fact]
    public void Manager_seeds_one_selected_workspace()
    {
        var m = new WorkspaceManager();

        Workspace seed = Assert.Single(m.Workspaces);
        Assert.Equal(seed.Id, m.SelectedId);
        Assert.Same(seed, m.Selected);
        Assert.Single(seed.Controller.AllSurfaces);
    }

    [Fact]
    public void NewWorkspace_appends_and_selects()
    {
        var m = new WorkspaceManager();
        Workspace first = m.Selected;

        Workspace second = m.NewWorkspace();

        Assert.Equal(new[] { first.Id, second.Id }, m.Workspaces.Select(w => w.Id));
        Assert.Equal(second.Id, m.SelectedId);
    }

    [Fact]
    public void Surface_ids_are_globally_unique_across_workspaces()
    {
        var m = new WorkspaceManager();
        Workspace a = m.Selected;
        Workspace b = m.NewWorkspace();
        a.Controller.NewTab(a.Controller.FocusedPane);
        b.Controller.NewTab(b.Controller.FocusedPane);

        var all = a.Controller.AllSurfaces.Concat(b.Controller.AllSurfaces).ToList();

        Assert.Equal(4, all.Count);
        Assert.Equal(all.Count, all.Distinct().Count());
    }

    [Fact]
    public void CloseWorkspace_moves_selection_to_neighbor()
    {
        var m = new WorkspaceManager();
        Workspace first = m.Selected;
        Workspace second = m.NewWorkspace();

        m.CloseWorkspace(second.Id);

        Workspace remaining = Assert.Single(m.Workspaces);
        Assert.Equal(first.Id, remaining.Id);
        Assert.Equal(first.Id, m.SelectedId);
    }

    [Fact]
    public void Closing_a_non_selected_workspace_keeps_selection()
    {
        var m = new WorkspaceManager();
        Workspace first = m.Selected;
        Workspace second = m.NewWorkspace();

        m.CloseWorkspace(first.Id);

        Assert.Equal(second.Id, m.SelectedId);
    }

    [Fact] // The workspace analog of R6: the sidebar is never empty.
    public void Closing_the_last_workspace_seeds_a_replacement()
    {
        var m = new WorkspaceManager();
        WorkspaceId original = m.SelectedId;

        m.CloseWorkspace(original);

        Workspace replacement = Assert.Single(m.Workspaces);
        Assert.NotEqual(original, replacement.Id);
        Assert.Equal(replacement.Id, m.SelectedId);
        Assert.Single(replacement.Controller.AllSurfaces);
    }

    [Fact]
    public void Lifecycle_events_fire_for_host_view_wiring()
    {
        var m = new WorkspaceManager();
        Workspace? created = null;
        Workspace? closed = null;
        m.WorkspaceCreated += w => created = w;
        m.WorkspaceClosed += w => closed = w;

        Workspace second = m.NewWorkspace();
        Assert.Same(second, created);

        m.CloseWorkspace(second.Id);
        Assert.Same(second, closed);
    }

    [Fact]
    public void SelectWorkspace_ignores_unknown_ids()
    {
        var m = new WorkspaceManager();
        WorkspaceId selected = m.SelectedId;

        m.SelectWorkspace(new WorkspaceId(999));

        Assert.Equal(selected, m.SelectedId);
    }

    // ---- Surface→workspace resolution + report routing ------------------------------------------

    [Fact]
    public void FindBySurface_resolves_the_owning_workspace()
    {
        var m = new WorkspaceManager();
        Workspace a = m.Selected;
        Workspace b = m.NewWorkspace();
        SurfaceId surfaceInB = b.Controller.AllSurfaces[0];

        Assert.Same(b, m.FindBySurface(surfaceInB));
        Assert.Same(a, m.FindBySurface(a.Controller.AllSurfaces[0]));
        Assert.Null(m.FindBySurface(new SurfaceId(999)));
    }

    [Fact]
    public void ReportGitBranch_lands_on_the_owning_workspace_only()
    {
        var m = new WorkspaceManager();
        Workspace a = m.Selected;
        Workspace b = m.NewWorkspace();

        m.ReportGitBranch(b.Controller.AllSurfaces[0], "feat/sidebar", isDirty: true);

        Assert.Null(a.GitBranch);
        Assert.Equal(new GitBranchInfo("feat/sidebar", true), b.GitBranch);
    }

    [Fact]
    public void ReportPr_and_pwd_update_workspace_metadata()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;
        SurfaceId surface = w.Controller.AllSurfaces[0];

        m.ReportPr(surface, "42", "Add sidebar", "open", "feat/sidebar", isStale: false);
        m.ReportPwd(surface, @"C:\dev\x");

        Assert.Equal(new PullRequestInfo("42", "Add sidebar", "open", "feat/sidebar", false), w.PullRequest);
        Assert.Equal(@"C:\dev\x", w.CurrentDirectory);
    }

    [Fact]
    public void Reports_for_unknown_surfaces_are_dropped()
    {
        var m = new WorkspaceManager();

        m.ReportGitBranch(new SurfaceId(999), "main", isDirty: false);

        Assert.Null(m.Selected.GitBranch);
    }

    [Fact] // The "watch git status" gate (plan Phase 5 U2).
    public void WatchGitStatus_off_suppresses_git_and_pr_reports()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;
        w.WatchGitStatus = false;
        SurfaceId surface = w.Controller.AllSurfaces[0];

        m.ReportGitBranch(surface, "main", isDirty: false);
        m.ReportPr(surface, "42", "x", "open", null, false);

        Assert.Null(w.GitBranch);
        Assert.Null(w.PullRequest);
    }

    // ---- Workspace metadata --------------------------------------------------------------------

    [Fact]
    public void DisplayTitle_prefers_custom_then_derived_then_id()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;

        Assert.Equal(w.Id.ToString(), w.DisplayTitle);

        w.SetTitle("pwsh — C:\\dev");
        Assert.Equal("pwsh — C:\\dev", w.DisplayTitle);

        w.SetCustomTitle("backend");
        Assert.Equal("backend", w.DisplayTitle);

        w.SetCustomTitle(null);
        Assert.Equal("pwsh — C:\\dev", w.DisplayTitle);
    }

    [Fact]
    public void SetStatus_parses_key_value_and_clears_on_empty_value()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;

        w.SetStatus("claude:busy");
        Assert.Equal("busy", w.StatusEntries["claude"]);

        w.SetStatus("claude:idle");
        Assert.Equal("idle", w.StatusEntries["claude"]);

        w.SetStatus("claude:");
        Assert.False(w.StatusEntries.ContainsKey("claude"));
    }

    [Fact]
    public void Metadata_changes_raise_the_manager_changed_event()
    {
        var m = new WorkspaceManager();
        int raised = 0;
        m.Changed += () => raised++;

        m.Selected.SetTitle("t");
        m.ReportPwd(m.Selected.Controller.AllSurfaces[0], @"C:\x");
        m.Selected.SetProgress("3/5");

        Assert.Equal(3, raised);
    }

    [Fact] // Idempotent mutations must not spam the sidebar with re-renders.
    public void Redundant_mutations_do_not_raise_changed()
    {
        var m = new WorkspaceManager();
        m.Selected.SetTitle("t");
        int raised = 0;
        m.Changed += () => raised++;

        m.Selected.SetTitle("t");
        m.Selected.SetProgress(null);
        m.Selected.SetCustomTitle("");

        Assert.Equal(0, raised);
    }
}
