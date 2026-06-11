using System;
using System.Collections.Immutable;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.UI;

namespace Optimus.Sidebar;

/// <summary>
/// The workspace sidebar (plan Phase 5 U4): a vertical list of rows rendered imperatively from an
/// immutable <see cref="SidebarRowDto"/> array — the same dumb-view discipline as
/// <c>PaneTabStrip</c> (KTD5 / issue-#2586: rows are value snapshots, never live observables). It
/// raises intent events (<see cref="WorkspaceInvoked"/>/<see cref="WorkspaceCloseRequested"/>/
/// <see cref="NewWorkspaceRequested"/>) and never mutates model state itself; the host routes those
/// to the <see cref="WorkspaceManager"/> and the resulting change drives the next <see cref="Render"/>.
/// </summary>
internal sealed class SidebarView : Grid
{
    public const double PanelWidth = 220.0;

    private static readonly SolidColorBrush PanelBackground = new(Color.FromArgb(0xFF, 0x12, 0x12, 0x12));
    private static readonly SolidColorBrush SelectedRow = new(Color.FromArgb(0xFF, 0x2D, 0x2D, 0x2D));
    private static readonly SolidColorBrush TransparentRow = new(Color.FromArgb(0x00, 0x00, 0x00, 0x00));
    private static readonly SolidColorBrush TitleText = new(Color.FromArgb(0xFF, 0xE6, 0xE6, 0xE6));
    private static readonly SolidColorBrush MetaText = new(Color.FromArgb(0xFF, 0x8A, 0x8A, 0x8A));
    private static readonly SolidColorBrush BranchText = new(Color.FromArgb(0xFF, 0x7F, 0xB3, 0x6E));
    private static readonly SolidColorBrush DirtyText = new(Color.FromArgb(0xFF, 0xD9, 0xA0, 0x4E));
    private static readonly SolidColorBrush PrOpen = new(Color.FromArgb(0xFF, 0x4D, 0x9C, 0xF0));
    private static readonly SolidColorBrush PrMerged = new(Color.FromArgb(0xFF, 0xA8, 0x7F, 0xE0));
    private static readonly SolidColorBrush PrClosed = new(Color.FromArgb(0xFF, 0xD9, 0x6A, 0x6A));
    private static readonly SolidColorBrush UnreadBadge = new(Color.FromArgb(0xFF, 0x4D, 0x9C, 0xF0));
    private static readonly SolidColorBrush BadgeText = new(Color.FromArgb(0xFF, 0xFF, 0xFF, 0xFF));

    private readonly StackPanel _rows;
    private readonly CapacityIndicatorViewModel _capacity;

    /// <summary>Raised when the user clicks a row body — select/focus that workspace.</summary>
    public event Action<WorkspaceId>? WorkspaceInvoked;

    /// <summary>Raised when the user clicks a row's close affordance.</summary>
    public event Action<WorkspaceId>? WorkspaceCloseRequested;

    /// <summary>Raised when the user clicks the trailing "+" button.</summary>
    public event Action? NewWorkspaceRequested;

    public SidebarView()
    {
        Width = PanelWidth;
        Background = PanelBackground;
        RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

        _rows = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical };
        var scroller = new ScrollViewer
        {
            Content = _rows,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        };
        Grid.SetRow(scroller, 0);
        Children.Add(scroller);

        // Footer (plan U6): always-visible capacity meter above the New-Workspace button; the
        // button greys out at the safe-zone cap with a one-line reason (DESIGN.md Thesis). The
        // governor may be null (U3 failure path) — the indicator then shows the dash placeholder
        // and never disables the button.
        _capacity = new CapacityIndicatorViewModel(App.Capacity, Dispatch);

        var newButton = new Button
        {
            Content = "+  New workspace",
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left,
            Background = TransparentRow,
            BorderBrush = TransparentRow,
            Foreground = MetaText,
            Margin = new Thickness(4),
            IsEnabled = !_capacity.IsAtCap,
        };
        newButton.Click += (_, _) => NewWorkspaceRequested?.Invoke();

        var capHint = new TextBlock
        {
            Foreground = Tokens.TextMuted,
            FontSize = Tokens.FontMeta,
            TextWrapping = TextWrapping.Wrap,
            Margin = new Thickness(10, 0, 10, 8),
            Text = _capacity.HintText ?? "",
            Visibility = _capacity.HintText is null ? Visibility.Collapsed : Visibility.Visible,
        };

        _capacity.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName is nameof(CapacityIndicatorViewModel.IsAtCap))
            {
                newButton.IsEnabled = !_capacity.IsAtCap;
            }
            else if (e.PropertyName is nameof(CapacityIndicatorViewModel.HintText))
            {
                capHint.Text = _capacity.HintText ?? "";
                capHint.Visibility = _capacity.HintText is null ? Visibility.Collapsed : Visibility.Visible;
            }
        };

        var footer = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical };
        footer.Children.Add(new CapacityIndicatorView(_capacity));
        footer.Children.Add(newButton);
        footer.Children.Add(capHint);
        Grid.SetRow(footer, 1);
        Children.Add(footer);
    }

    /// <summary>Marshal a capacity state change onto this view's UI thread (StateChanged fires on
    /// the 1 Hz ticker thread, plan U3).</summary>
    private void Dispatch(Action action)
    {
        if (DispatcherQueue is null || !DispatcherQueue.TryEnqueue(() => action()))
        {
            action();
        }
    }

    /// <summary>Rebuild the row list from a fresh value snapshot (cheap at sidebar scale).</summary>
    public void Render(ImmutableArray<SidebarRowDto> rows)
    {
        _rows.Children.Clear();
        foreach (SidebarRowDto row in rows)
        {
            _rows.Children.Add(BuildRow(row));
        }
    }

    private UIElement BuildRow(SidebarRowDto row)
    {
        var grid = new Grid
        {
            Background = row.IsSelected ? SelectedRow : TransparentRow,
            Padding = new Thickness(10, 8, 6, 8),
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
                new ColumnDefinition { Width = GridLength.Auto },
            },
        };

        var lines = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical, Spacing = 2 };
        Grid.SetColumn(lines, 0);

        // Line 1: title.
        lines.Children.Add(new TextBlock
        {
            Text = row.Title,
            Foreground = TitleText,
            FontSize = 13,
            TextTrimming = TextTrimming.CharacterEllipsis,
        });

        // Line 2: branch (+dirty marker) and PR badge.
        var meta = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal, Spacing = 8 };
        if (row.GitBranch is string branch)
        {
            meta.Children.Add(new TextBlock
            {
                Text = row.GitDirty ? branch + " ●" : branch,
                Foreground = row.GitDirty ? DirtyText : BranchText,
                FontSize = 11,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }
        if (row.PrBadge is string pr)
        {
            meta.Children.Add(new TextBlock
            {
                Text = pr,
                Foreground = PrBrush(row.PrStatus),
                FontSize = 11,
            });
        }
        if (meta.Children.Count > 0)
        {
            lines.Children.Add(meta);
        }

        // Line 3: working directory.
        if (row.Cwd is string cwd)
        {
            lines.Children.Add(new TextBlock
            {
                Text = cwd,
                Foreground = MetaText,
                FontSize = 11,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }

        // Line 4: latest notification text (survives marking-read — the "most recent" feed).
        if (row.LatestText is string latest && latest.Length > 0)
        {
            lines.Children.Add(new TextBlock
            {
                Text = latest,
                Foreground = MetaText,
                FontSize = 11,
                FontStyle = Windows.UI.Text.FontStyle.Italic,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }

        // Line 5: agent status / progress.
        if (row.Status is string status || row.Progress is not null)
        {
            string text = row.Status ?? "";
            if (row.Progress is string progress)
            {
                text = text.Length > 0 ? $"{text} · {progress}" : progress;
            }
            lines.Children.Add(new TextBlock
            {
                Text = text,
                Foreground = MetaText,
                FontSize = 11,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }

        grid.Children.Add(lines);

        // Trailing cluster: unread badge + close affordance.
        var trailing = new StackPanel
        {
            Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal,
            Spacing = 4,
            VerticalAlignment = VerticalAlignment.Top,
        };
        Grid.SetColumn(trailing, 1);

        if (row.UnreadCount > 0)
        {
            trailing.Children.Add(new Grid
            {
                VerticalAlignment = VerticalAlignment.Center,
                Children =
                {
                    new Ellipse { Width = 18, Height = 18, Fill = UnreadBadge },
                    new TextBlock
                    {
                        Text = row.UnreadCount > 9 ? "9+" : row.UnreadCount.ToString(),
                        Foreground = BadgeText,
                        FontSize = 10,
                        HorizontalAlignment = HorizontalAlignment.Center,
                        VerticalAlignment = VerticalAlignment.Center,
                    },
                },
            });
        }

        WorkspaceId id = row.Id; // capture the value, not the loop variable's final state
        var close = new Button
        {
            Content = "✕",
            FontSize = 10,
            Padding = new Thickness(4, 2, 4, 2),
            Background = TransparentRow,
            BorderBrush = TransparentRow,
            Foreground = MetaText,
        };
        close.Click += (_, _) => WorkspaceCloseRequested?.Invoke(id);
        trailing.Children.Add(close);

        grid.Children.Add(trailing);

        grid.PointerPressed += (_, e) =>
        {
            e.Handled = true;
            WorkspaceInvoked?.Invoke(id);
        };
        return grid;
    }

    private static SolidColorBrush PrBrush(string? status) => status switch
    {
        "merged" => PrMerged,
        "closed" => PrClosed,
        _ => PrOpen,
    };
}
