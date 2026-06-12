using System;
using System.Collections.Immutable;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;

namespace Optimus.Splits;

/// <summary>
/// The per-pane tab strip (plan Phase 2 U4): a row of chips rendered imperatively from an immutable
/// <see cref="TabHeaderDto"/> array (KTD5) plus a trailing "+" affordance. It is a dumb view — it
/// raises intent events (<see cref="TabSelected"/>/<see cref="TabClosed"/>/<see cref="NewTabRequested"/>)
/// and never mutates model state itself; <see cref="PaneView"/> routes those to the controller and
/// the resulting snapshot drives the next <see cref="Render"/>. Headers are projected from a
/// value-type snapshot, never bound to a live surface object (issue-#2586 discipline).
/// </summary>
internal sealed class PaneTabStrip : Grid
{
    private const double StripHeight = 32.0;

    // Unread-notification dot (plan Phase 3 U7): a small magenta marker in a fixed leading slot,
    // dedicated to "unread" (DESIGN.md RISK #2) so it never collides with the teal focus border or
    // the blue pr-open badge. The slot is always reserved (constant width) so toggling unread never
    // shifts the title text.
    private const double UnreadSlotWidth = 14.0;
    private const double UnreadDotSize = 7.0;

    private readonly StackPanel _tabs;
    private readonly Button _zoomButton;

    /// <summary>Raised when the user clicks a tab chip body.</summary>
    public event Action<SurfaceId>? TabSelected;

    /// <summary>Raised when the user clicks a chip's close affordance.</summary>
    public event Action<SurfaceId>? TabClosed;

    /// <summary>Raised when the user clicks the trailing "+" button.</summary>
    public event Action? NewTabRequested;

    /// <summary>Raised when the user clicks the split-right button (side-by-side split).</summary>
    public event Action? SplitRightRequested;

    /// <summary>Raised when the user clicks the split-down button (stacked split).</summary>
    public event Action? SplitDownRequested;

    /// <summary>Raised when the user clicks the zoom button.</summary>
    public event Action? ZoomToggleRequested;

    public PaneTabStrip()
    {
        Height = StripHeight;
        Background = Tokens.Surface2;
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        _tabs = new StackPanel
        {
            Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal,
            VerticalAlignment = VerticalAlignment.Stretch,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        Grid.SetColumn(_tabs, 0);
        Children.Add(_tabs);

        // Trailing action cluster: the discoverable split/zoom affordances beside the new-tab "+".
        // Each surfaces its keyboard chord on hover (R3) and fires an intent event PaneView routes
        // through the same dispatch path the chord uses (R1/R6).
        var actions = new StackPanel
        {
            Orientation = Microsoft.UI.Xaml.Controls.Orientation.Horizontal,
            VerticalAlignment = VerticalAlignment.Stretch,
        };

        // Glyphs shade the half where the NEW pane lands: ◨ = right (split-right), ⬓ = bottom
        // (split-down). The existing pane keeps the unshaded half; the new pane takes focus (R4).
        Button splitRight = MakeFlatButton("◨", $"Split right ({ShortcutMap.DescribeChord(ShortcutAction.SplitRight)})");
        splitRight.Click += (_, _) => SplitRightRequested?.Invoke();
        actions.Children.Add(splitRight);

        Button splitDown = MakeFlatButton("⬓", $"Split down ({ShortcutMap.DescribeChord(ShortcutAction.SplitDown)})");
        splitDown.Click += (_, _) => SplitDownRequested?.Invoke();
        actions.Children.Add(splitDown);

        _zoomButton = MakeFlatButton("⤢", $"Zoom ({ShortcutMap.DescribeChord(ShortcutAction.ToggleZoom)})");
        _zoomButton.Click += (_, _) => ZoomToggleRequested?.Invoke();
        actions.Children.Add(_zoomButton);

        Button newTab = MakeFlatButton("+", "New tab");
        newTab.Click += (_, _) => NewTabRequested?.Invoke();
        actions.Children.Add(newTab);

        Grid.SetColumn(actions, 1);
        Children.Add(actions);
    }

    /// <summary>Rebuild the chips from the projected headers (cheap; called per snapshot change).</summary>
    public void Render(ImmutableArray<TabHeaderDto> headers)
    {
        _tabs.Children.Clear();
        foreach (TabHeaderDto header in headers)
        {
            _tabs.Children.Add(MakeChip(header));
        }
    }

    /// <summary>
    /// Reflect whether this pane is the zoomed one on the zoom button's appearance (R5). Driven from
    /// the controller snapshot's zoom state via <see cref="PaneView"/>; a pure view update.
    /// </summary>
    public void SetZoomActive(bool active)
    {
        _zoomButton.Foreground = active ? Tokens.TextPrimary : Tokens.TextMuted;
        _zoomButton.Background = active ? Tokens.SurfaceSelected : Tokens.Transparent;
    }

    private FrameworkElement MakeChip(TabHeaderDto header)
    {
        var chip = new Grid
        {
            Background = header.IsSelected ? Tokens.SurfaceSelected : Tokens.Transparent,
            Padding = new Thickness(4, 0, 2, 0),
            MinWidth = 90,
            MaxWidth = 220,
            VerticalAlignment = VerticalAlignment.Stretch,
        };
        chip.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(UnreadSlotWidth) });
        chip.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        chip.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        // Unread dot in the reserved leading slot (only painted when unread; the slot is always present
        // so read/unread tabs keep their titles aligned).
        if (header.Unread)
        {
            var dot = new Ellipse
            {
                Width = UnreadDotSize,
                Height = UnreadDotSize,
                Fill = Tokens.Unread,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
            };
            Grid.SetColumn(dot, 0);
            chip.Children.Add(dot);
        }

        var title = new TextBlock
        {
            Text = header.Title,
            Foreground = header.IsSelected ? Tokens.TextPrimary : Tokens.TextMuted,
            VerticalAlignment = VerticalAlignment.Center,
            TextTrimming = TextTrimming.CharacterEllipsis,
            FontSize = Tokens.FontBody,
        };
        Grid.SetColumn(title, 1);
        chip.Children.Add(title);

        Button close = MakeFlatButton("✕", "Close tab"); // ✕
        // Close before select: even if the chip's Tapped also fires, SelectTab on a now-removed
        // surface is a controller no-op, so ordering is harmless.
        close.Click += (_, _) => TabClosed?.Invoke(header.Id);
        Grid.SetColumn(close, 2);
        chip.Children.Add(close);

        chip.Tapped += (_, e) =>
        {
            e.Handled = true;
            TabSelected?.Invoke(header.Id);
        };
        return chip;
    }

    private static Button MakeFlatButton(string glyph, string tooltip)
    {
        var button = new Button
        {
            Content = new TextBlock { Text = glyph, FontSize = Tokens.FontMeta },
            Background = Tokens.Transparent,
            BorderThickness = new Thickness(0),
            Foreground = Tokens.TextMuted,
            Padding = new Thickness(8, 4, 8, 4),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ToolTipService.SetToolTip(button, tooltip);
        return button;
    }
}
