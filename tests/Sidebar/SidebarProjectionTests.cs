using System;
using System.Collections.Immutable;
using System.Linq;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Phase 5 U3: the sidebar row projection. The first test pins the issue-#2586 discipline — rows
/// must be value types so the view can never end up bound to a live workspace object.
/// </summary>
public sealed class SidebarProjectionTests
{
    [Fact] // The snapshot-boundary rule (plan risk #8): DTO rows, never observables.
    public void Rows_are_immutable_value_types()
    {
        Assert.True(typeof(SidebarRowDto).IsValueType);
        foreach (System.Reflection.PropertyInfo p in typeof(SidebarRowDto).GetProperties())
        {
            bool initOnly = p.SetMethod is null || p.SetMethod.ReturnParameter
                .GetRequiredCustomModifiers()
                .Contains(typeof(System.Runtime.CompilerServices.IsExternalInit));
            Assert.True(initOnly, $"{p.Name} must be init-only (no mutable row state — #2586)");
        }
    }

    [Fact]
    public void Projects_one_row_per_workspace_marking_the_selected_one()
    {
        var m = new WorkspaceManager();
        Workspace a = m.Selected;
        Workspace b = m.NewWorkspace();

        ImmutableArray<SidebarRowDto> rows = SidebarProjection.Project(m);

        Assert.Equal(2, rows.Length);
        Assert.Equal(a.Id, rows[0].Id);
        Assert.False(rows[0].IsSelected);
        Assert.Equal(b.Id, rows[1].Id);
        Assert.True(rows[1].IsSelected);
    }

    [Fact]
    public void Row_carries_git_pr_cwd_and_progress_metadata()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;
        SurfaceId surface = w.Controller.AllSurfaces[0];
        m.ReportGitBranch(surface, "feat/sidebar", isDirty: true);
        m.ReportPr(surface, "42", "Add sidebar", "open", "feat/sidebar", isStale: false);
        m.ReportPwd(surface, @"C:\dev\x");
        w.SetProgress("3/5");
        w.SetStatus("claude:busy");

        SidebarRowDto row = SidebarProjection.Project(m)[0];

        Assert.Equal("feat/sidebar", row.GitBranch);
        Assert.True(row.GitDirty);
        Assert.Equal("#42 open", row.PrBadge);
        Assert.Equal("open", row.PrStatus);
        Assert.Equal(@"C:\dev\x", row.Cwd);
        Assert.Equal("3/5", row.Progress);
        Assert.Equal("claude busy", row.Status);
    }

    [Fact]
    public void Unreported_metadata_projects_as_null()
    {
        var m = new WorkspaceManager();

        SidebarRowDto row = SidebarProjection.Project(m)[0];

        Assert.Null(row.GitBranch);
        Assert.False(row.GitDirty);
        Assert.Null(row.PrBadge);
        Assert.Null(row.Cwd);
        Assert.Null(row.Status);
        Assert.Null(row.Progress);
        Assert.Null(row.LatestText);
        Assert.Equal(0, row.UnreadCount);
    }

    [Fact]
    public void Unread_and_latest_come_from_the_supplied_callbacks()
    {
        var m = new WorkspaceManager();
        Workspace a = m.Selected;
        Workspace b = m.NewWorkspace();

        ImmutableArray<SidebarRowDto> rows = SidebarProjection.Project(
            m,
            unreadOf: id => id == a.Id ? 3 : 0,
            latestOf: id => id == a.Id ? "Build finished" : null);

        Assert.Equal(3, rows[0].UnreadCount);
        Assert.Equal("Build finished", rows[0].LatestText);
        Assert.Equal(0, rows[1].UnreadCount);
        Assert.Null(rows[1].LatestText);
    }

    [Fact]
    public void Title_falls_back_from_custom_to_derived_to_id()
    {
        var m = new WorkspaceManager();
        Workspace w = m.Selected;

        Assert.Equal(w.Id.ToString(), SidebarProjection.Project(m)[0].Title);

        w.SetTitle("pwsh");
        Assert.Equal("pwsh", SidebarProjection.Project(m)[0].Title);

        w.SetCustomTitle("backend");
        Assert.Equal("backend", SidebarProjection.Project(m)[0].Title);
    }

    [Fact]
    public void Home_directory_paths_abbreviate_to_tilde()
    {
        var m = new WorkspaceManager();
        string home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        m.ReportPwd(m.Selected.Controller.AllSurfaces[0], System.IO.Path.Combine(home, "dev"));

        Assert.Equal(@"~\dev", SidebarProjection.Project(m)[0].Cwd);
    }

    [Fact]
    public void Pr_number_keeps_an_existing_hash_prefix()
    {
        var m = new WorkspaceManager();
        m.ReportPr(m.Selected.Controller.AllSurfaces[0], "#7", "", "merged", null, false);

        Assert.Equal("#7 merged", SidebarProjection.Project(m)[0].PrBadge);
    }
}
