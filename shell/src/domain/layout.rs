//! Port of core/Splits/LayoutGeometry.cs — pure geometry for the split tree (KTD3): leaf
//! rectangles from a tree + bounds, directional pane navigation, and the star-ratio ↔
//! divider-fraction conversion the splitter chrome needs (U3). No UI dependency, fully
//! unit-testable.

use std::collections::BTreeMap;

use crate::domain::ids::PaneId;
use crate::domain::split_tree::{Direction, Orientation, SplitNode};

/// A normalized rectangle (any unit). Pure value type used for layout + navigation math.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LayoutRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LayoutRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center_x(&self) -> f64 {
        self.x + (self.width / 2.0)
    }

    pub fn center_y(&self) -> f64 {
        self.y + (self.height / 2.0)
    }
}

/// Clamp a divider fraction into the valid [0,1] range (bonsplit's `min(max(x,0),1)`).
pub fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// Convert two star-sized track weights (read back from a dragged splitter) into the first
/// child's [0,1] fraction. Round-trips with the `fraction* / (1-fraction)*` mapping the view
/// applies (U3); degenerate zero-total input falls back to a centered 0.5.
pub fn fraction_from_stars(first_star: f64, second_star: f64) -> f64 {
    let total = first_star + second_star;
    if total <= 0.0 {
        return 0.5;
    }
    clamp01(first_star / total)
}

/// Compute each leaf's rectangle by recursively splitting `bounds` on each branch's
/// orientation and divider fraction (KTD7).
pub fn compute_leaf_rects(root: &SplitNode, bounds: LayoutRect) -> BTreeMap<PaneId, LayoutRect> {
    let mut map = BTreeMap::new();
    walk(root, bounds, &mut map);
    map
}

fn walk(node: &SplitNode, rect: LayoutRect, map: &mut BTreeMap<PaneId, LayoutRect>) {
    match node {
        SplitNode::Leaf(leaf) => {
            map.insert(leaf.id, rect);
        }
        SplitNode::Branch(branch) => {
            let f = clamp01(branch.divider_position);
            if branch.orientation == Orientation::Vertical {
                // Side-by-side: first on the left, second on the right.
                let w1 = rect.width * f;
                walk(&branch.first, LayoutRect { width: w1, ..rect }, map);
                walk(
                    &branch.second,
                    LayoutRect::new(rect.x + w1, rect.y, rect.width - w1, rect.height),
                    map,
                );
            } else {
                // Stacked: first on top, second below.
                let h1 = rect.height * f;
                walk(&branch.first, LayoutRect { height: h1, ..rect }, map);
                walk(
                    &branch.second,
                    LayoutRect::new(rect.x, rect.y + h1, rect.width, rect.height - h1),
                    map,
                );
            }
        }
    }
}

/// The leaf nearest to `from` in the given `direction`, or `None` if none lies that way (R8).
/// A candidate must sit on the correct side of the source's center; candidates whose
/// perpendicular extent overlaps the source's are always preferred over non-overlapping ones,
/// and ties break on center distance. Pure → testable.
pub fn navigate_from(root: &SplitNode, from: PaneId, direction: Direction) -> Option<PaneId> {
    let rects = compute_leaf_rects(root, LayoutRect::new(0.0, 0.0, 1.0, 1.0));
    let src = *rects.get(&from)?;

    const OVERLAP_PENALTY: f64 = 1_000_000.0;
    let mut best: Option<PaneId> = None;
    let mut best_score = f64::MAX;

    for (&id, &r) in &rects {
        if id == from {
            continue;
        }
        if !in_direction(src, r, direction) {
            continue;
        }

        let overlaps = axis_overlap(src, r, direction);
        let dx = r.center_x() - src.center_x();
        let dy = r.center_y() - src.center_y();
        let score = (dx * dx) + (dy * dy) + if overlaps { 0.0 } else { OVERLAP_PENALTY };
        if score < best_score {
            best_score = score;
            best = Some(id);
        }
    }

    best
}

fn in_direction(src: LayoutRect, candidate: LayoutRect, direction: Direction) -> bool {
    const EPS: f64 = 1e-9;
    match direction {
        Direction::Left => candidate.center_x() < src.center_x() - EPS,
        Direction::Right => candidate.center_x() > src.center_x() + EPS,
        Direction::Up => candidate.center_y() < src.center_y() - EPS,
        Direction::Down => candidate.center_y() > src.center_y() + EPS,
    }
}

fn axis_overlap(src: LayoutRect, candidate: LayoutRect, direction: Direction) -> bool {
    // For horizontal moves the perpendicular axis is Y; for vertical moves it is X.
    match direction {
        Direction::Left | Direction::Right => {
            src.y < candidate.bottom() && candidate.y < src.bottom()
        }
        Direction::Up | Direction::Down => src.x < candidate.right() && candidate.x < src.right(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::ids::{BranchId, SurfaceId};
    use crate::domain::split_tree::{PaneLeaf, SplitBranch};

    fn pane(id: i32) -> Arc<SplitNode> {
        Arc::new(SplitNode::Leaf(PaneLeaf {
            id: PaneId(id),
            tabs: vec![SurfaceId(id)],
            selected: SurfaceId(id),
        }))
    }

    fn branch(
        id: i32,
        o: Orientation,
        first: Arc<SplitNode>,
        second: Arc<SplitNode>,
        divider: f64,
    ) -> Arc<SplitNode> {
        Arc::new(SplitNode::Branch(SplitBranch {
            id: BranchId(id),
            orientation: o,
            first,
            second,
            divider_position: divider,
        }))
    }

    fn assert_close(expected: f64, actual: f64) {
        assert!(
            (expected - actual).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    // ---- ComputeLeafRects ----------------------------------------------------------------

    #[test]
    fn vertical_split_lays_panes_side_by_side() {
        let root = branch(1, Orientation::Vertical, pane(1), pane(2), 0.5);

        let rects = compute_leaf_rects(&root, LayoutRect::new(0.0, 0.0, 100.0, 100.0));

        assert_eq!(LayoutRect::new(0.0, 0.0, 50.0, 100.0), rects[&PaneId(1)]);
        assert_eq!(LayoutRect::new(50.0, 0.0, 50.0, 100.0), rects[&PaneId(2)]);
    }

    #[test]
    fn horizontal_split_stacks_panes_with_divider_fraction() {
        let root = branch(1, Orientation::Horizontal, pane(1), pane(2), 0.25);

        let rects = compute_leaf_rects(&root, LayoutRect::new(0.0, 0.0, 100.0, 100.0));

        assert_eq!(LayoutRect::new(0.0, 0.0, 100.0, 25.0), rects[&PaneId(1)]);
        assert_eq!(LayoutRect::new(0.0, 25.0, 100.0, 75.0), rects[&PaneId(2)]);
    }

    // ---- NavigateFrom --------------------------------------------------------------------

    #[test]
    fn move_focus_right_from_left_pane_of_vertical_split_finds_right_pane() {
        let root = branch(1, Orientation::Vertical, pane(1), pane(2), 0.5);

        assert_eq!(
            Some(PaneId(2)),
            navigate_from(&root, PaneId(1), Direction::Right)
        );
        assert_eq!(
            Some(PaneId(1)),
            navigate_from(&root, PaneId(2), Direction::Left)
        );
    }

    #[test]
    fn move_focus_with_no_pane_in_direction_is_noop() {
        let root = branch(1, Orientation::Vertical, pane(1), pane(2), 0.5);

        assert_eq!(None, navigate_from(&root, PaneId(1), Direction::Left));
        assert_eq!(None, navigate_from(&root, PaneId(1), Direction::Up));
        assert_eq!(None, navigate_from(&root, PaneId(2), Direction::Right));
    }

    #[test]
    fn navigation_in_nested_tree_picks_directionally_correct_leaf() {
        // horizontal{ top = vertical{ p1, p2 }, bottom = p3 }
        let root = branch(
            1,
            Orientation::Horizontal,
            branch(2, Orientation::Vertical, pane(1), pane(2), 0.5),
            pane(3),
            0.5,
        );

        assert_eq!(
            Some(PaneId(2)),
            navigate_from(&root, PaneId(1), Direction::Right)
        );
        assert_eq!(
            Some(PaneId(1)),
            navigate_from(&root, PaneId(2), Direction::Left)
        );
        assert_eq!(
            Some(PaneId(3)),
            navigate_from(&root, PaneId(1), Direction::Down)
        );
        assert_eq!(
            Some(PaneId(3)),
            navigate_from(&root, PaneId(2), Direction::Down)
        );
        assert_eq!(None, navigate_from(&root, PaneId(1), Direction::Up));
    }

    #[test]
    fn navigation_prefers_the_geometrically_nearest_overlapping_leaf() {
        // vertical{ left = p1 (full height), right = horizontal{ p2 (thin top), p3 (tall bottom) } }
        // p1's vertical center (0.5) is nearer p3's center (0.6) than p2's (0.1), so Right → p3.
        let root = branch(
            1,
            Orientation::Vertical,
            pane(1),
            branch(2, Orientation::Horizontal, pane(2), pane(3), 0.2),
            0.5,
        );

        assert_eq!(
            Some(PaneId(3)),
            navigate_from(&root, PaneId(1), Direction::Right)
        );
    }

    // ---- Star ratio ↔ fraction -------------------------------------------------------------

    #[test]
    fn fraction_from_stars_maps_track_weights_to_unit_fraction() {
        let cases: [(f64, f64, f64); 4] = [
            (1.0, 1.0, 0.5),
            (3.0, 1.0, 0.75),
            (1.0, 3.0, 0.25),
            (0.0, 0.0, 0.5), // degenerate → centered
        ];
        for (a, b, expected) in cases {
            assert_close(expected, fraction_from_stars(a, b));
        }
    }

    #[test]
    fn fraction_from_stars_round_trips_with_the_views_star_mapping() {
        const FRACTION: f64 = 0.3;
        // The view sizes tracks as `fraction*` / `(1-fraction)*`; reading them back must recover it.
        let recovered = fraction_from_stars(FRACTION, 1.0 - FRACTION);
        assert_close(FRACTION, recovered);
    }

    #[test]
    fn clamp01_constrains_to_unit_interval() {
        let cases: [(f64, f64); 3] = [(-0.5, 0.0), (1.5, 1.0), (0.42, 0.42)];
        for (input, expected) in cases {
            assert_close(expected, clamp01(input));
        }
    }
}
