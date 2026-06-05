using System;
using System.Collections.Immutable;
using Cmux.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Cmux.Splits;

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

    private static readonly SolidColorBrush StripBackground = new(Color.FromArgb(0xFF, 0x16, 0x16, 0x16));
    private static readonly SolidColorBrush SelectedChip = new(Color.FromArgb(0xFF, 0x2D, 0x2D, 0x2D));
    private static readonly SolidColorBrush Transparent = new(Color.FromArgb(0x00, 0x00, 0x00, 0x00));
    private static readonly SolidColorBrush ActiveText = new(Color.FromArgb(0xFF, 0xE6, 0xE6, 0xE6));
    private static readonly SolidColorBrush MutedText = new(Color.FromArgb(0xFF, 0x8A, 0x8A, 0x8A));

    private readonly StackPanel _tabs;

    /// <summary>Raised when the user clicks a tab chip body.</summary>
    public event Action<SurfaceId>? TabSelected;

    /// <summary>Raised when the user clicks a chip's close affordance.</summary>
    public event Action<SurfaceId>? TabClosed;

    /// <summary>Raised when the user clicks the trailing "+" button.</summary>
    public event Action? NewTabRequested;

    public PaneTabStrip()
    {
        Height = StripHeight;
        Background = StripBackground;
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

        Button newTab = MakeFlatButton("+", "New tab");
        newTab.Click += (_, _) => NewTabRequested?.Invoke();
        Grid.SetColumn(newTab, 1);
        Children.Add(newTab);
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

    private FrameworkElement MakeChip(TabHeaderDto header)
    {
        var chip = new Grid
        {
            Background = header.IsSelected ? SelectedChip : Transparent,
            Padding = new Thickness(10, 0, 2, 0),
            MinWidth = 90,
            MaxWidth = 220,
            VerticalAlignment = VerticalAlignment.Stretch,
        };
        chip.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        chip.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var title = new TextBlock
        {
            Text = header.Title,
            Foreground = header.IsSelected ? ActiveText : MutedText,
            VerticalAlignment = VerticalAlignment.Center,
            TextTrimming = TextTrimming.CharacterEllipsis,
            FontSize = 12,
        };
        Grid.SetColumn(title, 0);
        chip.Children.Add(title);

        Button close = MakeFlatButton("✕", "Close tab"); // ✕
        // Close before select: even if the chip's Tapped also fires, SelectTab on a now-removed
        // surface is a controller no-op, so ordering is harmless.
        close.Click += (_, _) => TabClosed?.Invoke(header.Id);
        Grid.SetColumn(close, 1);
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
            Content = new TextBlock { Text = glyph, FontSize = 11 },
            Background = Transparent,
            BorderThickness = new Thickness(0),
            Foreground = MutedText,
            Padding = new Thickness(8, 4, 8, 4),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ToolTipService.SetToolTip(button, tooltip);
        return button;
    }
}
