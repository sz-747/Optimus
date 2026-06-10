using System.Collections.Generic;

namespace Optimus.Core;

/// <summary>A normalized rectangle (any unit). Pure value type used for layout + navigation math.</summary>
public readonly record struct LayoutRect(double X, double Y, double Width, double Height)
{
    public double Right => X + Width;
    public double Bottom => Y + Height;
    public double CenterX => X + (Width / 2);
    public double CenterY => Y + (Height / 2);
}

/// <summary>
/// Pure geometry for the split tree (KTD3): leaf rectangles from a tree + bounds, directional
/// pane navigation, and the star-ratio ↔ divider-fraction conversion the WinUI
/// <c>GridSplitter</c> needs (U3). No WinUI dependency, fully unit-testable.
/// </summary>
public static class LayoutGeometry
{
    /// <summary>Clamp a divider fraction into the valid [0,1] range (bonsplit's <c>min(max(x,0),1)</c>).</summary>
    public static double Clamp01(double value) => value < 0 ? 0 : value > 1 ? 1 : value;

    /// <summary>
    /// Convert two star-sized track weights (read back from a dragged <c>GridSplitter</c>) into the
    /// first child's [0,1] fraction. Round-trips with the <c>fraction* / (1-fraction)*</c> mapping
    /// the view applies (U3); degenerate zero-total input falls back to a centered 0.5.
    /// </summary>
    public static double FractionFromStars(double firstStar, double secondStar)
    {
        double total = firstStar + secondStar;
        if (total <= 0)
        {
            return 0.5;
        }
        return Clamp01(firstStar / total);
    }

    /// <summary>
    /// Compute each leaf's rectangle by recursively splitting <paramref name="bounds"/> on each
    /// branch's orientation and divider fraction (KTD7).
    /// </summary>
    public static IReadOnlyDictionary<PaneId, LayoutRect> ComputeLeafRects(SplitNode root, LayoutRect bounds)
    {
        var map = new Dictionary<PaneId, LayoutRect>();
        Walk(root, bounds, map);
        return map;
    }

    private static void Walk(SplitNode node, LayoutRect rect, IDictionary<PaneId, LayoutRect> map)
    {
        switch (node)
        {
            case PaneLeaf leaf:
                map[leaf.Id] = rect;
                break;
            case SplitBranch branch:
                double f = Clamp01(branch.DividerPosition);
                if (branch.Orientation == Orientation.Vertical)
                {
                    // Side-by-side: first on the left, second on the right.
                    double w1 = rect.Width * f;
                    Walk(branch.First, rect with { Width = w1 }, map);
                    Walk(branch.Second, new LayoutRect(rect.X + w1, rect.Y, rect.Width - w1, rect.Height), map);
                }
                else
                {
                    // Stacked: first on top, second below.
                    double h1 = rect.Height * f;
                    Walk(branch.First, rect with { Height = h1 }, map);
                    Walk(branch.Second, new LayoutRect(rect.X, rect.Y + h1, rect.Width, rect.Height - h1), map);
                }
                break;
        }
    }

    /// <summary>
    /// The leaf nearest to <paramref name="from"/> in the given <paramref name="direction"/>, or
    /// <c>null</c> if none lies that way (R8). A candidate must sit on the correct side of the
    /// source's center; candidates whose perpendicular extent overlaps the source's are always
    /// preferred over non-overlapping ones, and ties break on center distance. Pure → testable.
    /// </summary>
    public static PaneId? NavigateFrom(SplitNode root, PaneId from, Direction direction)
    {
        IReadOnlyDictionary<PaneId, LayoutRect> rects = ComputeLeafRects(root, new LayoutRect(0, 0, 1, 1));
        if (!rects.TryGetValue(from, out LayoutRect src))
        {
            return null;
        }

        const double overlapPenalty = 1_000_000.0;
        PaneId? best = null;
        double bestScore = double.MaxValue;

        foreach (KeyValuePair<PaneId, LayoutRect> kv in rects)
        {
            if (kv.Key == from)
            {
                continue;
            }
            LayoutRect r = kv.Value;
            if (!InDirection(src, r, direction))
            {
                continue;
            }

            bool overlaps = AxisOverlap(src, r, direction);
            double dx = r.CenterX - src.CenterX;
            double dy = r.CenterY - src.CenterY;
            double score = ((dx * dx) + (dy * dy)) + (overlaps ? 0 : overlapPenalty);
            if (score < bestScore)
            {
                bestScore = score;
                best = kv.Key;
            }
        }

        return best;
    }

    private static bool InDirection(LayoutRect src, LayoutRect candidate, Direction direction)
    {
        const double eps = 1e-9;
        return direction switch
        {
            Direction.Left => candidate.CenterX < src.CenterX - eps,
            Direction.Right => candidate.CenterX > src.CenterX + eps,
            Direction.Up => candidate.CenterY < src.CenterY - eps,
            Direction.Down => candidate.CenterY > src.CenterY + eps,
            _ => false,
        };
    }

    private static bool AxisOverlap(LayoutRect src, LayoutRect candidate, Direction direction)
    {
        // For horizontal moves the perpendicular axis is Y; for vertical moves it is X.
        return direction is Direction.Left or Direction.Right
            ? src.Y < candidate.Bottom && candidate.Y < src.Bottom
            : src.X < candidate.Right && candidate.X < src.Right;
    }
}
