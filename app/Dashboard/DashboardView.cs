using System;
using System.Collections.Immutable;
using Optimus.Design;
using Optimus.Sidebar;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using CapacityLevel = Optimus.Core.CapacityLevel;
using CapacityState = Optimus.Core.CapacityState;
using WorkspaceRowDto = Optimus.Core.SidebarRowDto;

namespace Optimus.Dashboard;

/// <summary>
/// Frontend-only rendition of the Optimus mission-control reference. It deliberately owns sample
/// presentation data rather than reaching into the terminal/workspace backend: this makes visual
/// layout, hierarchy, and interaction testable before the dashboard's live-data contract is set.
/// </summary>
public sealed class DashboardView : Grid
{
    private readonly StackPanel _workspaceRows = new();
    private readonly StackPanel _webWorkspaceRows = new();
    private readonly StackPanel _infraWorkspaceRows = new();
    private readonly FrameworkElement _titleBar;
    private TextBlock? _capacityMetric;
    private Border? _capacityFill;
    private int _selectedWorkspace;
    private WorkspaceHost? _backend;
    private ImmutableArray<WorkspaceRowDto> _backendWorkspaces = ImmutableArray<WorkspaceRowDto>.Empty;

    private readonly DashboardWorkspace[] _workspaces =
    {
        new("feat/parallel-scheduler", "In progress", "Implement parallel scheduling", Tokens.StatusGreen),
        new("fix/resource-leak", "Needs review", "Resolve memory leak in worker", Tokens.StatusAmber),
        new("feature/timeline", "Working", "Improve timeline experience", Tokens.StatusBlue),
        new("chore/deps-bump", "Done", "Update dependencies", Tokens.StatusGreen),
    };

    public DashboardView()
    {
        Background = Tokens.DashboardCanvas;
        RequestedTheme = ElementTheme.Dark;
        RowDefinitions.Add(new RowDefinition { Height = new GridLength(54) });
        RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

        _titleBar = BuildTopBar();
        Children.Add(_titleBar);
        FrameworkElement shell = BuildShell();
        Grid.SetRow(shell, 1);
        Children.Add(shell);
    }

    /// <summary>
    /// Connect the completed visual shell to the existing workspace host. The rich agent/activity
    /// content stays presentation data until those server-side concepts exist; workspace creation
    /// and selection already travel through the real capacity-aware backend.
    /// </summary>
    public void AttachBackend(WorkspaceHost backend)
    {
        _backend = backend ?? throw new ArgumentNullException(nameof(backend));
        _backend.DashboardStateChanged += SyncSelectionFromBackend;
        if (App.Capacity is { } capacity)
        {
            capacity.StateChanged += OnCapacityChanged;
            RenderCapacity(capacity.State);
        }
        SyncSelectionFromBackend();
    }

    /// <summary>Drag region adopted by <see cref="MainWindow"/> as its native title bar.</summary>
    public UIElement TitleBarElement => _titleBar;

    private FrameworkElement BuildTopBar()
    {
        var bar = new Grid
        {
            Background = Tokens.DashboardCanvas,
            Padding = new Thickness(24, 0, 0, 0),
        };

        var brand = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, VerticalAlignment = VerticalAlignment.Center };
        brand.Children.Add(new Border
        {
            Width = 28,
            Height = 28,
            CornerRadius = new CornerRadius(5),
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(1),
            Child = CenteredText("◇", Tokens.TextPrimary, Tokens.FontHeading),
        });
        brand.Children.Add(new TextBlock
        {
            Text = "Optimus",
            Foreground = Tokens.TextPrimary,
            FontSize = Tokens.FontHeading,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
        });
        bar.Children.Add(brand);

        bar.Children.Add(new Rectangle { Height = 1, Fill = Tokens.DashboardBorder, VerticalAlignment = VerticalAlignment.Bottom });
        return bar;
    }

    private FrameworkElement BuildShell()
    {
        var shell = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(338) },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
            },
        };
        shell.Children.Add(BuildSidebar());
        FrameworkElement content = BuildContent();
        Grid.SetColumn(content, 1);
        shell.Children.Add(content);
        return shell;
    }

    private FrameworkElement BuildSidebar()
    {
        var sidebar = new Grid
        {
            Background = Tokens.DashboardSidebar,
            Padding = new Thickness(26, 28, 18, 18),
            RowDefinitions =
            {
                new RowDefinition { Height = GridLength.Auto },
                new RowDefinition { Height = new GridLength(28) },
                new RowDefinition { Height = GridLength.Auto },
                new RowDefinition { Height = new GridLength(26) },
                new RowDefinition { Height = new GridLength(1, GridUnitType.Star) },
                new RowDefinition { Height = GridLength.Auto },
            },
        };
        sidebar.Children.Add(BuildCapacity());

        Button newWorkspace = OutlinedButton("＋  New workspace", "N");
        newWorkspace.Click += (_, _) => AddDemoWorkspace();
        Grid.SetRow(newWorkspace, 2);
        sidebar.Children.Add(newWorkspace);

        var list = new StackPanel { Orientation = Orientation.Vertical, Spacing = 0 };
        list.Children.Add(SectionLabel("WORKSPACES", new Thickness(0, 0, 0, 12)));
        list.Children.Add(RepoHeader("⌄  Optimus Core"));
        list.Children.Add(_workspaceRows);
        list.Children.Add(RepoHeader("›  Optimus Web"));
        list.Children.Add(_webWorkspaceRows);
        list.Children.Add(RepoHeader("›  Infra"));
        list.Children.Add(_infraWorkspaceRows);
        Grid.SetRow(list, 4);
        sidebar.Children.Add(list);
        RenderWorkspaceRows();

        var settings = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Padding = new Thickness(0, 16, 0, 0) };
        settings.Children.Add(new TextBlock { Text = "⚙", Foreground = Tokens.TextPrimary, FontSize = Tokens.FontHeading });
        settings.Children.Add(new TextBlock { Text = "Settings", Foreground = Tokens.TextSecondary, FontSize = Tokens.FontHeading });
        Grid.SetRow(settings, 5);
        sidebar.Children.Add(settings);
        sidebar.Children.Add(new Rectangle { Width = 1, Fill = Tokens.DashboardBorder, HorizontalAlignment = HorizontalAlignment.Right });
        return sidebar;
    }

    private FrameworkElement BuildCapacity()
    {
        var capacity = new StackPanel { Orientation = Orientation.Vertical, Spacing = 7 };
        capacity.Children.Add(SectionLabel("SAFE CAPACITY"));

        var metric = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        _capacityMetric = new TextBlock { Text = "8 / 12", Foreground = Tokens.TextPrimary, FontSize = Tokens.FontMetric, FontFamily = Tokens.Mono };
        metric.Children.Add(_capacityMetric);
        metric.Children.Add(new TextBlock
        {
            Text = "safe slots",
            Foreground = Tokens.TextSecondary,
            FontSize = Tokens.FontHeading,
            VerticalAlignment = VerticalAlignment.Bottom,
            Margin = new Thickness(0, 0, 0, 3),
        });
        capacity.Children.Add(metric);
        var track = new Grid { Height = 7, Background = Tokens.DashboardBorder };
        _capacityFill = new Border
        {
            Background = Tokens.StatusGreen,
            CornerRadius = new CornerRadius(4),
            HorizontalAlignment = HorizontalAlignment.Left,
            Width = 282 * (8d / 12),
        };
        track.Children.Add(_capacityFill);
        capacity.Children.Add(track);
        capacity.Children.Add(new TextBlock { Text = "Recommended parallel capacity", Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody });
        return capacity;
    }

    private FrameworkElement BuildContent()
    {
        var content = new Grid
        {
            RowDefinitions =
            {
                new RowDefinition { Height = new GridLength(126) },
                new RowDefinition { Height = new GridLength(140) },
                new RowDefinition { Height = new GridLength(1, GridUnitType.Star) },
                new RowDefinition { Height = new GridLength(106) },
                new RowDefinition { Height = new GridLength(50) },
            },
        };
        content.Children.Add(BuildWorkspaceHeading());
        FrameworkElement metrics = BuildMetricCards();
        Grid.SetRow(metrics, 1);
        content.Children.Add(metrics);
        FrameworkElement work = BuildWorkArea();
        Grid.SetRow(work, 2);
        content.Children.Add(work);
        FrameworkElement timeline = BuildTimeline();
        Grid.SetRow(timeline, 3);
        content.Children.Add(timeline);
        FrameworkElement details = BuildDetailsBar();
        Grid.SetRow(details, 4);
        content.Children.Add(details);
        return content;
    }

    private FrameworkElement BuildWorkspaceHeading()
    {
        var header = new Grid
        {
            Padding = new Thickness(40, 28, 32, 12),
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };
        var text = new StackPanel { Orientation = Orientation.Vertical, Spacing = 8 };
        text.Children.Add(new TextBlock { Text = "feat/parallel-scheduler", Foreground = Tokens.TextPrimary, FontSize = Tokens.FontDisplay, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        text.Children.Add(new TextBlock
        {
            Text = "Mission: Implement parallel scheduling to run multiple agents safely and efficiently.",
            Foreground = Tokens.TextSecondary,
            FontSize = Tokens.FontHeading,
            TextTrimming = TextTrimming.CharacterEllipsis,
        });
        header.Children.Add(text);
        Button menu = BuildIconButton("•••");
        Grid.SetColumn(menu, 1);
        header.Children.Add(menu);
        return header;
    }

    private FrameworkElement BuildMetricCards()
    {
        var cards = new Grid { Margin = new Thickness(40, 0, 32, 12) };
        for (int i = 0; i < 4; i++) cards.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        (string icon, string label, string value, string status, SolidColorBrush color)[] metrics =
        {
            ("♧", "Active agents", "4", "Running now", Tokens.StatusGreen),
            ("⌁", "Progress", "68%", "On track", Tokens.StatusBlue),
            ("☑", "Open items", "3", "Needs attention", Tokens.StatusAmber),
            ("$", "Spend today", "$1.57", "Within budget", Tokens.StatusPurple),
        };
        for (int i = 0; i < metrics.Length; i++)
        {
            Border card = MetricCard(metrics[i]);
            if (i > 0) card.Margin = new Thickness(18, 0, 0, 0);
            Grid.SetColumn(card, i);
            cards.Children.Add(card);
        }
        return cards;
    }

    private FrameworkElement BuildWorkArea()
    {
        var area = new Grid
        {
            Margin = new Thickness(40, 14, 32, 24),
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = new GridLength(390) },
            },
        };
        area.Children.Add(BuildAgentGraph());
        FrameworkElement activity = BuildActivity();
        Grid.SetColumn(activity, 1);
        area.Children.Add(activity);
        return area;
    }

    private FrameworkElement BuildAgentGraph()
    {
        var graph = new Border
        {
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Margin = new Thickness(0, 0, 28, 0),
            Padding = new Thickness(16),
        };
        var layout = new Grid
        {
            RowDefinitions =
            {
                new RowDefinition { Height = new GridLength(1, GridUnitType.Star) },
                new RowDefinition { Height = GridLength.Auto },
            },
        };
        var agents = new Grid
        {
            VerticalAlignment = VerticalAlignment.Center,
            RowDefinitions =
            {
                new RowDefinition { Height = GridLength.Auto },
                new RowDefinition { Height = new GridLength(34) },
                new RowDefinition { Height = GridLength.Auto },
            },
        };
        var lead = AgentCard("◇", "Lead Agent", "Orchestrator", "Planning", "Breaking down work and coordinating agents.", Tokens.StatusGreen);
        lead.Width = 310;
        lead.HorizontalAlignment = HorizontalAlignment.Center;
        agents.Children.Add(lead);
        var connector = new Grid { Height = 34 };
        connector.Children.Add(new Rectangle { Width = 1, Fill = Tokens.DashboardBorder, Height = 16, VerticalAlignment = VerticalAlignment.Top, HorizontalAlignment = HorizontalAlignment.Center });
        connector.Children.Add(new Rectangle { Height = 1, Fill = Tokens.DashboardBorder, Margin = new Thickness(72, 16, 72, 0), VerticalAlignment = VerticalAlignment.Top });
        Grid.SetRow(connector, 1);
        agents.Children.Add(connector);
        var workers = new Grid { ColumnSpacing = 20 };
        for (int i = 0; i < 4; i++) workers.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        (string icon, string title, string status, string text, SolidColorBrush color)[] people =
        {
            ("☷", "Planner", "Working", "Designing the approach and task list.", Tokens.StatusBlue),
            ("⌘", "Executor", "Working", "Implementing parallel scheduling.", Tokens.StatusAmber),
            ("⌕", "Reviewer", "Review", "Reviewing changes and validating quality.", Tokens.StatusPurple),
            ("✓", "Tester", "Done", "Tests added and passing.", Tokens.StatusGreen),
        };
        for (int i = 0; i < people.Length; i++)
        {
            var person = AgentCard(people[i].icon, people[i].title, null, people[i].status, people[i].text, people[i].color);
            Grid.SetColumn(person, i);
            workers.Children.Add(person);
        }
        Grid.SetRow(workers, 2);
        agents.Children.Add(workers);
        layout.Children.Add(agents);

        var legend = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Center, Spacing = 28, Margin = new Thickness(0, 14, 0, 4) };
        foreach ((string label, SolidColorBrush color) in new[]
        {
            ("Planning", Tokens.StatusGreen), ("Working", Tokens.StatusBlue), ("Review", Tokens.StatusPurple),
            ("Waiting", Tokens.StatusAmber), ("Done", Tokens.StatusGreen),
        }) legend.Children.Add(StatusLabel(label, color));
        Grid.SetRow(legend, 1);
        layout.Children.Add(legend);
        graph.Child = layout;
        return graph;
    }

    private FrameworkElement BuildActivity()
    {
        var panel = new StackPanel { Orientation = Orientation.Vertical, Spacing = 16, Padding = new Thickness(8, 6, 0, 0) };
        var title = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };
        title.Children.Add(SectionLabel("ACTIVITY"));
        var live = StatusLabel("Live", Tokens.StatusGreen);
        Grid.SetColumn(live, 1);
        title.Children.Add(live);
        panel.Children.Add(title);
        foreach ((string time, string icon, string name, string text, SolidColorBrush color) in new[]
        {
            ("10:42 AM", "◇", "Lead Agent", "Created task plan and assigned work.", Tokens.StatusGreen),
            ("10:40 AM", "☷", "Planner", "Completed design outline.", Tokens.StatusBlue),
            ("10:38 AM", "⌘", "Executor", "Implemented task queue.", Tokens.StatusAmber),
            ("10:36 AM", "⌕", "Reviewer", "Reviewed API changes.\n1 comment", Tokens.StatusPurple),
            ("10:33 AM", "✓", "Tester", "All tests passing.", Tokens.StatusGreen),
            ("10:30 AM", "◷", "Planner", "Waiting for product approval.", Tokens.StatusAmber),
        }) panel.Children.Add(ActivityRow(time, icon, name, text, color));
        panel.Children.Add(OutlinedButton("View all activity                                      ↗", null));
        return panel;
    }

    private FrameworkElement BuildTimeline()
    {
        var timeline = new Grid
        {
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(0, 1, 0, 1),
            Padding = new Thickness(40, 14, 32, 12),
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(148) },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = new GridLength(120) },
            },
        };
        var labels = new StackPanel { Orientation = Orientation.Vertical, Spacing = 10 };
        labels.Children.Add(new TextBlock { Text = "TIMELINE", Foreground = Tokens.TextPrimary, FontSize = Tokens.FontBody, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        labels.Children.Add(new TextBlock { Text = "Today", Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody });
        timeline.Children.Add(labels);
        FrameworkElement track = BuildTimelineTrack();
        Grid.SetColumn(track, 1);
        timeline.Children.Add(track);
        var goLive = OutlinedButton("◉  Go live", null);
        goLive.HorizontalAlignment = HorizontalAlignment.Right;
        goLive.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(goLive, 2);
        timeline.Children.Add(goLive);
        return timeline;
    }

    private FrameworkElement BuildDetailsBar() => new Grid
    {
        Padding = new Thickness(40, 0, 0, 0),
        Children =
        {
            new TextBlock
            {
                Text = "›   Technical details (optional)",
                Foreground = Tokens.TextSecondary,
                FontSize = Tokens.FontBody,
                VerticalAlignment = VerticalAlignment.Center,
            },
        },
    };

    private void RenderWorkspaceRows()
    {
        _workspaceRows.Children.Clear();
        _webWorkspaceRows.Children.Clear();
        _infraWorkspaceRows.Children.Clear();

        // Preserve the reference dashboard's four-row information density on a fresh session;
        // later real workspaces append naturally as the backend creates them.
        int count = Math.Max(_workspaces.Length, _backendWorkspaces.Length);
        for (int index = 0; index < count; index++)
        {
            StackPanel group = index < 2 ? _workspaceRows : index == 2 ? _webWorkspaceRows : _infraWorkspaceRows;
            group.Children.Add(BuildWorkspaceRow(index));
        }
    }

    private FrameworkElement BuildWorkspaceRow(int index)
    {
        DashboardWorkspace workspace = WorkspaceAt(index);
        var row = new Button
        {
            Background = index == _selectedWorkspace ? Tokens.DashboardSelected : Tokens.Transparent,
            BorderBrush = Tokens.Transparent,
            BorderThickness = new Thickness(0),
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Padding = new Thickness(18, 12, 12, 12),
            Margin = new Thickness(0, 0, 0, 2),
        };
        row.Click += (_, _) =>
        {
            _selectedWorkspace = index;
            _backend?.SelectWorkspaceFromDashboard(index);
            RenderWorkspaceRows();
        };
        var content = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };
        var copy = new StackPanel { Orientation = Orientation.Vertical, Spacing = 5 };
        copy.Children.Add(new TextBlock { Text = workspace.Name, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontHeading, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis });
        copy.Children.Add(new TextBlock { Text = workspace.Status, Foreground = workspace.Color, FontSize = Tokens.FontBody });
        copy.Children.Add(new TextBlock { Text = workspace.Description, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody, TextTrimming = TextTrimming.CharacterEllipsis });
        content.Children.Add(copy);
        var dot = new Ellipse { Width = 7, Height = 7, Fill = workspace.Color, Margin = new Thickness(10, 4, 0, 0) };
        Grid.SetColumn(dot, 1);
        content.Children.Add(dot);
        row.Content = content;
        return row;
    }

    private void AddDemoWorkspace()
    {
        _backend?.CreateWorkspaceFromDashboard();
    }

    private void SyncSelectionFromBackend()
    {
        _backendWorkspaces = _backend?.DashboardRows ?? ImmutableArray<WorkspaceRowDto>.Empty;
        int selected = -1;
        for (int index = 0; index < _backendWorkspaces.Length; index++)
        {
            if (_backendWorkspaces[index].IsSelected)
            {
                selected = index;
                break;
            }
        }
        _selectedWorkspace = selected >= 0 ? selected : 0;
        RenderWorkspaceRows();
    }

    private void OnCapacityChanged(CapacityState state)
    {
        // Capacity ticks originate from the provider/ticker path, not the UI thread.
        if (DispatcherQueue.TryEnqueue(() => RenderCapacity(state)))
        {
            return;
        }
    }

    private void RenderCapacity(CapacityState state)
    {
        if (_capacityMetric is null || _capacityFill is null)
        {
            return;
        }

        int occupied = state.Used + state.Reserved;
        _capacityMetric.Text = $"{occupied} / {state.Max}";
        _capacityFill.Width = state.Max <= 0 ? 0 : 282 * Math.Clamp((double)occupied / state.Max, 0, 1);
        _capacityFill.Background = state.Level switch
        {
            CapacityLevel.Cap => Tokens.CapacityCap,
            CapacityLevel.Warn => Tokens.CapacityWarn,
            _ => Tokens.StatusGreen,
        };
    }

    private DashboardWorkspace WorkspaceAt(int index)
    {
        DashboardWorkspace template = index < _workspaces.Length
            ? _workspaces[index]
            : new DashboardWorkspace($"workspace-{index + 1}", "Ready", "New terminal workspace", Tokens.StatusBlue);

        if (_backendWorkspaces.IsDefaultOrEmpty || index >= _backendWorkspaces.Length)
        {
            return template;
        }

        WorkspaceRowDto backend = _backendWorkspaces[index];
        // A fresh terminal has only its generated id. Keep the visual reference's legible sample
        // name until shell integration reports a real title or status, then prefer live backend data.
        string generatedId = $"workspace-{index + 1}";
        string name = string.Equals(backend.Title, generatedId, StringComparison.OrdinalIgnoreCase) ? template.Name : backend.Title;
        string status = string.IsNullOrWhiteSpace(backend.Status) ? template.Status : backend.Status;
        string description = backend.LatestText ?? backend.Cwd ?? template.Description;
        return new DashboardWorkspace(name, status, description, StatusBrush(status, template.Color));
    }

    private static SolidColorBrush StatusBrush(string status, SolidColorBrush fallback) =>
        status.Contains("review", StringComparison.OrdinalIgnoreCase) ? Tokens.StatusAmber :
        status.Contains("done", StringComparison.OrdinalIgnoreCase) ? Tokens.StatusGreen :
        status.Contains("work", StringComparison.OrdinalIgnoreCase) ? Tokens.StatusBlue : fallback;

    private static Border MetricCard((string icon, string label, string value, string status, SolidColorBrush color) metric)
    {
        var card = new Border
        {
            Background = Tokens.DashboardCard,
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Padding = new Thickness(14),
        };
        var rows = new StackPanel { Orientation = Orientation.Vertical, Spacing = 6 };
        var top = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        top.Children.Add(new Border { Width = 42, Height = 42, Background = Tokens.DashboardSelected, CornerRadius = new CornerRadius(8), Child = CenteredText(metric.icon, metric.color, Tokens.FontHeading) });
        var stat = new StackPanel { Orientation = Orientation.Vertical };
        stat.Children.Add(new TextBlock { Text = metric.label, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody });
        stat.Children.Add(new TextBlock { Text = metric.value, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontMetric, FontFamily = Tokens.Mono });
        top.Children.Add(stat);
        rows.Children.Add(top);
        rows.Children.Add(new TextBlock { Text = metric.status, Foreground = metric.color, FontSize = Tokens.FontBody });
        card.Child = rows;
        return card;
    }

    private static Border AgentCard(string icon, string title, string? subtitle, string status, string description, SolidColorBrush color)
    {
        var card = new Border
        {
            Background = Tokens.DashboardCard,
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Padding = new Thickness(12),
            MinHeight = 124,
        };
        var stack = new StackPanel { Orientation = Orientation.Vertical, Spacing = 8 };
        var header = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = GridLength.Auto },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };
        header.Children.Add(new Border { Width = 34, Height = 34, Background = Tokens.DashboardSelected, CornerRadius = new CornerRadius(17), Child = CenteredText(icon, color, Tokens.FontHeading) });
        var titleStack = new StackPanel { Orientation = Orientation.Vertical, Margin = new Thickness(10, 0, 4, 0) };
        titleStack.Children.Add(new TextBlock { Text = title, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontHeading, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis });
        if (subtitle is not null) titleStack.Children.Add(new TextBlock { Text = subtitle, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody });
        Grid.SetColumn(titleStack, 1);
        header.Children.Add(titleStack);
        var statusText = StatusLabel(status, color);
        Grid.SetColumn(statusText, 2);
        header.Children.Add(statusText);
        stack.Children.Add(header);
        stack.Children.Add(new TextBlock { Text = description, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody, TextWrapping = TextWrapping.Wrap, MaxLines = 2 });
        card.Child = stack;
        return card;
    }

    private static FrameworkElement ActivityRow(string time, string icon, string name, string text, SolidColorBrush color)
    {
        var row = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(88) },
                new ColumnDefinition { Width = new GridLength(30) },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
            },
        };
        row.Children.Add(new TextBlock { Text = time, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody, VerticalAlignment = VerticalAlignment.Top });
        var mark = CenteredText(icon, color, Tokens.FontHeading);
        Grid.SetColumn(mark, 1);
        row.Children.Add(mark);
        var copy = new StackPanel { Orientation = Orientation.Vertical, Spacing = 3 };
        copy.Children.Add(new TextBlock { Text = name, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontBody, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        copy.Children.Add(new TextBlock { Text = text, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody, TextWrapping = TextWrapping.Wrap });
        Grid.SetColumn(copy, 2);
        row.Children.Add(copy);
        return row;
    }

    private static FrameworkElement BuildTimelineTrack()
    {
        string[] labels = { "10:20 AM", "10:30 AM", "10:40 AM", "10:50 AM", "11:00 AM" };
        SolidColorBrush[] colors = { Tokens.StatusGreen, Tokens.StatusBlue, Tokens.StatusAmber, Tokens.StatusPurple, Tokens.StatusGreen };
        var track = new Grid { Margin = new Thickness(6, 4, 20, 0) };
        track.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        track.RowDefinitions.Add(new RowDefinition { Height = new GridLength(16) });
        var labelGrid = new Grid();
        for (int i = 0; i < labels.Length; i++) labelGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        for (int i = 0; i < labels.Length; i++)
        {
            var label = new TextBlock { Text = labels[i], Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody, HorizontalAlignment = HorizontalAlignment.Center };
            Grid.SetColumn(label, i);
            labelGrid.Children.Add(label);
        }
        track.Children.Add(labelGrid);
        var dots = new Grid { VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 2, 0, 0), Background = Tokens.DashboardBorder, Height = 2 };
        for (int i = 0; i < colors.Length; i++) dots.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        for (int i = 0; i < colors.Length; i++)
        {
            var dot = new Ellipse { Width = 11, Height = 11, Fill = colors[i], HorizontalAlignment = HorizontalAlignment.Center };
            Grid.SetColumn(dot, i);
            dots.Children.Add(dot);
        }
        Grid.SetRow(dots, 1);
        track.Children.Add(dots);
        return track;
    }

    private static TextBlock SectionLabel(string text, Thickness? margin = null) => new()
    {
        Text = text,
        Foreground = Tokens.TextSecondary,
        FontSize = Tokens.FontBody,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        CharacterSpacing = 80,
        Margin = margin ?? new Thickness(),
    };

    private static FrameworkElement RepoHeader(string text) => new Border
    {
        BorderBrush = Tokens.DashboardBorder,
        BorderThickness = new Thickness(0, 1, 0, 0),
        Child = new TextBlock
        {
            Text = text,
            Foreground = Tokens.TextSecondary,
            FontSize = Tokens.FontBody,
            Padding = new Thickness(0, 12, 0, 10),
        },
    };

    private static StackPanel StatusLabel(string text, SolidColorBrush color)
    {
        var status = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        status.Children.Add(new Ellipse { Width = 7, Height = 7, Fill = color, VerticalAlignment = VerticalAlignment.Center });
        status.Children.Add(new TextBlock { Text = text, Foreground = Tokens.TextSecondary, FontSize = Tokens.FontBody });
        return status;
    }

    private static Button OutlinedButton(string text, string? badge)
    {
        var button = new Button
        {
            Background = Tokens.Transparent,
            BorderBrush = Tokens.DashboardBorder,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Padding = new Thickness(14, 9, 14, 9),
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
        };
        var content = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };
        content.Children.Add(new TextBlock { Text = text, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontHeading, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        if (badge is not null)
        {
            var hint = new Border { BorderBrush = Tokens.DashboardBorder, BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(4), Padding = new Thickness(5, 2, 5, 2), Child = CenteredText(badge, Tokens.TextSecondary, Tokens.FontBody) };
            Grid.SetColumn(hint, 1);
            content.Children.Add(hint);
        }
        button.Content = content;
        return button;
    }

    private static Button BuildIconButton(string glyph) => new()
    {
        Content = new TextBlock { Text = glyph, Foreground = Tokens.TextPrimary, FontSize = Tokens.FontHeading },
        Background = Tokens.Transparent,
        BorderBrush = Tokens.DashboardBorder,
        BorderThickness = new Thickness(1),
        CornerRadius = new CornerRadius(6),
        Padding = new Thickness(12, 7, 12, 7),
    };

    private static TextBlock CenteredText(string text, SolidColorBrush brush, double size) => new()
    {
        Text = text,
        Foreground = brush,
        FontSize = size,
        HorizontalAlignment = HorizontalAlignment.Center,
        VerticalAlignment = VerticalAlignment.Center,
    };

    private sealed record DashboardWorkspace(string Name, string Status, string Description, SolidColorBrush Color);
}
