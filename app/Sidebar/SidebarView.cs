using System;
using System.Collections.Immutable;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;

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

    private readonly StackPanel _rows;
    private readonly CapacityIndicatorViewModel _capacity;
    private readonly Button _newButton;
    private readonly TextBlock _capHint;
    private bool _capacityHooked;

    /// <summary>Raised when the user clicks a row body — select/focus that workspace.</summary>
    public event Action<WorkspaceId>? WorkspaceInvoked;

    /// <summary>Raised when the user clicks a row's close affordance.</summary>
    public event Action<WorkspaceId>? WorkspaceCloseRequested;

    /// <summary>Raised when the user clicks the trailing "+" button.</summary>
    public event Action? NewWorkspaceRequested;

    public SidebarView()
    {
        Width = PanelWidth;
        Background = Tokens.Surface1;
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

        _newButton = new Button
        {
            Content = "+  New workspace",
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left,
            Background = Tokens.Transparent,
            BorderBrush = Tokens.Transparent,
            Foreground = Tokens.TextMuted,
            Margin = new Thickness(4),
        };
        _newButton.Click += (_, _) => NewWorkspaceRequested?.Invoke();

        _capHint = new TextBlock
        {
            Foreground = Tokens.TextMuted,
            FontSize = Tokens.FontMeta,
            TextWrapping = TextWrapping.Wrap,
            Margin = new Thickness(10, 0, 10, 8),
        };

        var footer = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical };
        footer.Children.Add(new CapacityIndicatorView(_capacity));
        footer.Children.Add(_newButton);
        footer.Children.Add(_capHint);
        Grid.SetRow(footer, 1);
        Children.Add(footer);

        // Subscription lifetime (review fix): the sidebar is effectively an app-lifetime
        // singleton inside WorkspaceHost, but the capacity model outlives any visual tree, so
        // the VM↔model hookup is toggled symmetrically with the view's tree membership —
        // Unloaded detaches (no leak, no updates into an unloaded tree), Loaded reattaches and
        // refreshes from the model's current state. Both are idempotent (re-parent safe).
        HookCapacity();
        Loaded += (_, _) => HookCapacity();
        Unloaded += (_, _) => UnhookCapacity();
    }

    private void HookCapacity()
    {
        if (_capacityHooked)
        {
            return;
        }
        _capacityHooked = true;
        _capacity.PropertyChanged += OnCapacityPropertyChanged;
        _capacity.Attach();
        RefreshCapacityChrome();
    }

    private void UnhookCapacity()
    {
        if (!_capacityHooked)
        {
            return;
        }
        _capacityHooked = false;
        _capacity.Detach();
        _capacity.PropertyChanged -= OnCapacityPropertyChanged;
    }

    private void OnCapacityPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(CapacityIndicatorViewModel.IsAtCap)
            or nameof(CapacityIndicatorViewModel.HintText))
        {
            RefreshCapacityChrome();
        }
    }

    private void RefreshCapacityChrome()
    {
        _newButton.IsEnabled = !_capacity.IsAtCap;
        _capHint.Text = _capacity.HintText ?? "";
        _capHint.Visibility = _capacity.HintText is null ? Visibility.Collapsed : Visibility.Visible;
    }

    /// <summary>Marshal a capacity state change onto this view's UI thread (StateChanged fires on
    /// the 1 Hz ticker thread, plan U3). If the enqueue fails (dispatcher shutting down / not
    /// available), the update is DROPPED — never run inline, which would mutate WinUI objects
    /// off-thread (review fix). The next successful state change repaints.</summary>
    private void Dispatch(Action action)
    {
        if (DispatcherQueue is null || !DispatcherQueue.TryEnqueue(() => action()))
        {
            System.Diagnostics.Debug.WriteLine("[capacity] dropped a state update: dispatcher unavailable");
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
            Background = row.IsSelected ? Tokens.SurfaceSelected : Tokens.Transparent,
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
            Foreground = Tokens.TextPrimary,
            FontSize = Tokens.FontTitle,
            TextTrimming = TextTrimming.CharacterEllipsis,
        });

        // Line 2: branch (+dirty marker) and PR badge.
        var meta = new StackPanel { Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal, Spacing = 8 };
        if (row.GitBranch is string branch)
        {
            meta.Children.Add(new TextBlock
            {
                Text = row.GitDirty ? branch + " ●" : branch,
                Foreground = row.GitDirty ? Tokens.GitDirty : Tokens.GitBranch,
                FontSize = Tokens.FontMeta,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }
        if (row.PrBadge is string pr)
        {
            meta.Children.Add(new TextBlock
            {
                Text = pr,
                Foreground = PrBrush(row.PrStatus),
                FontSize = Tokens.FontMeta,
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
                Foreground = Tokens.TextMuted,
                FontSize = Tokens.FontMeta,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }

        // Line 4: latest notification text (survives marking-read — the "most recent" feed).
        if (row.LatestText is string latest && latest.Length > 0)
        {
            lines.Children.Add(new TextBlock
            {
                Text = latest,
                Foreground = Tokens.TextMuted,
                FontSize = Tokens.FontMeta,
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
                Foreground = Tokens.TextMuted,
                FontSize = Tokens.FontMeta,
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
                    new Ellipse { Width = 18, Height = 18, Fill = Tokens.Unread },
                    new TextBlock
                    {
                        Text = row.UnreadCount > 9 ? "9+" : row.UnreadCount.ToString(),
                        Foreground = Tokens.TextOnAccent,
                        FontSize = Tokens.FontCaption,
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
            FontSize = Tokens.FontCaption,
            Padding = new Thickness(4, 2, 4, 2),
            Background = Tokens.Transparent,
            BorderBrush = Tokens.Transparent,
            Foreground = Tokens.TextMuted,
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
        "merged" => Tokens.PrMerged,
        "closed" => Tokens.PrClosed,
        _ => Tokens.PrOpen,
    };
}
