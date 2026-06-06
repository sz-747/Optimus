using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Cmux.Core;
using Xunit;

namespace Cmux.Core.Tests;

/// <summary>
/// Coverage for <see cref="TabHeaderProjection"/> (plan Phase 2 U4): the pure model→header mapping
/// the tab strip renders. Exactly one header is selected, titles come from the live title provider
/// with an id placeholder fallback, and the row order matches the tab order (KTD5).
/// </summary>
public sealed class TabHeaderProjectionTests
{
    private static SurfaceId S(int n) => new(n);

    [Fact] // A pane with 3 tabs yields 3 DTOs with exactly one IsSelected (plan U4 test scenario).
    public void Project_marks_exactly_the_selected_tab()
    {
        var tabs = new List<SurfaceId> { S(1), S(2), S(3) };

        ImmutableArray<TabHeaderDto> headers = TabHeaderProjection.Project(tabs, S(2));

        Assert.Equal(3, headers.Length);
        Assert.Single(headers, h => h.IsSelected);
        Assert.True(headers.Single(h => h.IsSelected).Id == S(2));
        Assert.Equal(new[] { S(1), S(2), S(3) }, headers.Select(h => h.Id).ToArray()); // order preserved
    }

    [Fact]
    public void Project_uses_the_title_provider_when_it_returns_a_value()
    {
        var tabs = new List<SurfaceId> { S(1), S(2) };
        var titles = new Dictionary<SurfaceId, string> { [S(1)] = "pwsh", [S(2)] = "vim" };

        ImmutableArray<TabHeaderDto> headers =
            TabHeaderProjection.Project(tabs, S(1), id => titles.TryGetValue(id, out string? t) ? t : null);

        Assert.Equal("pwsh", headers[0].Title);
        Assert.Equal("vim", headers[1].Title);
    }

    [Theory] // Null provider, missing entry, and empty string all fall back to the id placeholder.
    [InlineData(null)]
    [InlineData("")]
    public void Project_falls_back_to_the_id_when_no_title(string? provided)
    {
        var tabs = new List<SurfaceId> { S(7) };

        ImmutableArray<TabHeaderDto> headers = TabHeaderProjection.Project(tabs, S(7), _ => provided);

        Assert.Equal(S(7).ToString(), headers[0].Title);
    }

    [Fact]
    public void Project_falls_back_to_the_id_when_provider_is_null()
    {
        var tabs = new List<SurfaceId> { S(9) };

        ImmutableArray<TabHeaderDto> headers = TabHeaderProjection.Project(tabs, S(9));

        Assert.Equal(S(9).ToString(), headers[0].Title);
    }

    [Fact]
    public void Project_of_empty_tab_list_is_empty()
    {
        ImmutableArray<TabHeaderDto> headers = TabHeaderProjection.Project(new List<SurfaceId>(), default);

        Assert.True(headers.IsEmpty);
    }

    [Fact] // Phase 3 U7: Unread is set on exactly the surfaces the lookup reports unread. Covers R2.
    public void Project_marks_unread_only_for_surfaces_the_lookup_reports()
    {
        var tabs = new List<SurfaceId> { S(1), S(2), S(3) };
        var unread = new HashSet<SurfaceId> { S(2) };

        ImmutableArray<TabHeaderDto> headers =
            TabHeaderProjection.Project(tabs, S(1), titleOf: null, isUnread: unread.Contains);

        Assert.False(headers[0].Unread);
        Assert.True(headers[1].Unread);
        Assert.False(headers[2].Unread);
    }

    [Fact] // Selected and unread are independent flags on the same tab.
    public void Project_sets_selected_and_unread_independently()
    {
        var tabs = new List<SurfaceId> { S(5) };

        ImmutableArray<TabHeaderDto> headers =
            TabHeaderProjection.Project(tabs, S(5), titleOf: null, isUnread: _ => true);

        Assert.True(headers[0].IsSelected);
        Assert.True(headers[0].Unread);
    }

    [Fact] // No lookup -> nothing is unread (back-compat with Phase 2 callers).
    public void Project_without_unread_lookup_marks_nothing_unread()
    {
        var tabs = new List<SurfaceId> { S(1), S(2) };

        ImmutableArray<TabHeaderDto> headers = TabHeaderProjection.Project(tabs, S(1));

        Assert.All(headers, h => Assert.False(h.Unread));
    }
}
