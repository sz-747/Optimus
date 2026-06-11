using System;
using Optimus.Core;
using Optimus.Design;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;

namespace Optimus.Sidebar;

/// <summary>
/// The always-visible safe-zone capacity meter (plan U6, DESIGN.md Thesis: "capacity is the one
/// number the user must never have to hunt for"). A count label ("X / Y terminals", Cascadia Mono
/// per DESIGN.md counts rule) over a thin fill bar; both take the level color — calm `text-muted`
/// → `git-dirty` amber at Warn → `pr-closed` red at Cap. Dumb view: renders whatever the
/// view-model says, no capacity math of its own.
/// </summary>
internal sealed class CapacityIndicatorView : StackPanel
{
    private const double BarHeight = 3;

    private readonly CapacityIndicatorViewModel _vm;
    private readonly TextBlock _label;
    private readonly Grid _track;
    private readonly Rectangle _fill;

    public CapacityIndicatorView(CapacityIndicatorViewModel vm)
    {
        _vm = vm ?? throw new ArgumentNullException(nameof(vm));
        Orientation = Microsoft.UI.Xaml.Controls.Orientation.Vertical;
        Spacing = 4; // xs
        Padding = new Thickness(10, 8, 10, 0); // matches sidebar row gutters

        _label = new TextBlock
        {
            FontFamily = Tokens.Mono,
            FontSize = Tokens.FontMeta,
        };
        Children.Add(_label);

        _fill = new Rectangle
        {
            Height = BarHeight,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        _track = new Grid
        {
            Height = BarHeight,
            Background = Tokens.Hairline,
            Children = { _fill },
        };
        _track.SizeChanged += (_, _) => UpdateFillWidth();
        Children.Add(_track);

        _vm.PropertyChanged += (_, _) => Render();
        Render();
    }

    private void Render()
    {
        SolidColorBrush level = LevelBrush(_vm.Level);
        _label.Text = _vm.LabelText;
        _label.Foreground = level;
        _fill.Fill = level;
        UpdateFillWidth();
    }

    private void UpdateFillWidth() => _fill.Width = Math.Max(0, _track.ActualWidth * _vm.FractionUsed);

    private static SolidColorBrush LevelBrush(CapacityLevel level) => level switch
    {
        CapacityLevel.Cap => Tokens.CapacityCap,
        CapacityLevel.Warn => Tokens.CapacityWarn,
        _ => Tokens.CapacityCalm,
    };
}
