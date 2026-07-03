//! Frontend-facing view DTOs. The domain's `TreeSnapshot` is an `Arc`-linked recursive tree
//! (`!Send`, not serde-friendly); the frontend wants a flat, serializable layout it can render
//! as absolutely-positioned panes without re-walking a tree in JS. This module flattens a
//! snapshot into normalized [0,1] rectangles — pane rects (what backs each terminal) and divider
//! handles (draggable splitters) — computed by one walk that mirrors `layout::compute_leaf_rects`.

use serde::Serialize;

use crate::domain::layout::{compute_leaf_rects, LayoutRect};
use crate::domain::split_tree::{Orientation, SplitNode, TreeSnapshot};

/// A normalized rectangle in [0,1] space; the frontend scales it to the viewport with percentages.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
pub struct RectView {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<LayoutRect> for RectView {
    fn from(r: LayoutRect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

/// One terminal pane: its id, the tabs it holds, which surface is selected/visible, its rectangle,
/// and transient focus/zoom flags the chrome styles on.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct PaneView {
    pub pane_id: i32,
    pub tabs: Vec<i32>,
    pub selected: i32,
    pub rect: RectView,
    pub focused: bool,
    pub zoomed: bool,
}

/// One draggable divider: the branch it adjusts, its orientation, current fraction, and the rect
/// of the branch region it sits inside (so the frontend positions the handle line).
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
pub struct DividerView {
    pub branch_id: i32,
    /// `"vertical"` = side-by-side panes (a vertical divider line); `"horizontal"` = stacked.
    pub orientation: &'static str,
    pub fraction: f64,
    pub rect: RectView,
}

/// The whole layout the frontend renders: flat panes + dividers, plus the focus/zoom/version the
/// chrome needs. `version` lets the frontend drop a stale render if snapshots arrive out of order.
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct TreeView {
    pub panes: Vec<PaneView>,
    pub dividers: Vec<DividerView>,
    pub focused_pane: i32,
    pub zoomed_pane: Option<i32>,
    pub version: i32,
}

/// Flatten a domain snapshot into the frontend view. Pane rectangles come from the tested
/// `compute_leaf_rects`; the divider walk reuses the same split math so a handle always sits on
/// the boundary between the two child rects.
pub fn build_tree_view(snapshot: &TreeSnapshot) -> TreeView {
    let (panes, dividers) = match &snapshot.root {
        None => (Vec::new(), Vec::new()),
        Some(root) => {
            let bounds = LayoutRect::new(0.0, 0.0, 1.0, 1.0);
            let rects = compute_leaf_rects(root, bounds);

            let mut panes = Vec::new();
            for leaf in root.leaves() {
                let rect = rects
                    .get(&leaf.id)
                    .copied()
                    .unwrap_or_else(|| LayoutRect::new(0.0, 0.0, 0.0, 0.0));
                panes.push(PaneView {
                    pane_id: leaf.id.0,
                    tabs: leaf.tabs.iter().map(|s| s.0).collect(),
                    selected: leaf.selected.0,
                    rect: rect.into(),
                    focused: leaf.id == snapshot.focused_pane,
                    zoomed: snapshot.zoomed_pane == Some(leaf.id),
                });
            }

            let mut dividers = Vec::new();
            collect_dividers(root, bounds, &mut dividers);
            (panes, dividers)
        }
    };

    TreeView {
        panes,
        dividers,
        focused_pane: snapshot.focused_pane.0,
        zoomed_pane: snapshot.zoomed_pane.map(|p| p.0),
        version: snapshot.version,
    }
}

/// Walk the tree recording one [`DividerView`] per branch, splitting `rect` exactly as
/// `layout::compute_leaf_rects` does so the handle lands on the child boundary.
fn collect_dividers(node: &SplitNode, rect: LayoutRect, out: &mut Vec<DividerView>) {
    let SplitNode::Branch(branch) = node else {
        return;
    };
    let f = branch.divider_position.clamp(0.0, 1.0);
    let (first_rect, second_rect, orientation) = if branch.orientation == Orientation::Vertical {
        let w1 = rect.width * f;
        (
            LayoutRect::new(rect.x, rect.y, w1, rect.height),
            LayoutRect::new(rect.x + w1, rect.y, rect.width - w1, rect.height),
            "vertical",
        )
    } else {
        let h1 = rect.height * f;
        (
            LayoutRect::new(rect.x, rect.y, rect.width, h1),
            LayoutRect::new(rect.x, rect.y + h1, rect.width, rect.height - h1),
            "horizontal",
        )
    };
    out.push(DividerView {
        branch_id: branch.id.0,
        orientation,
        fraction: f,
        rect: rect.into(),
    });
    collect_dividers(&branch.first, first_rect, out);
    collect_dividers(&branch.second, second_rect, out);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::ids::{BranchId, PaneId, SurfaceId};
    use crate::domain::split_tree::{PaneLeaf, SplitBranch};

    fn pane(id: i32) -> Arc<SplitNode> {
        Arc::new(SplitNode::Leaf(PaneLeaf {
            id: PaneId(id),
            tabs: vec![SurfaceId(id)],
            selected: SurfaceId(id),
        }))
    }

    fn snapshot(root: Option<Arc<SplitNode>>, focused: i32, zoomed: Option<i32>) -> TreeSnapshot {
        TreeSnapshot {
            root,
            focused_pane: PaneId(focused),
            zoomed_pane: zoomed.map(PaneId),
            version: 7,
        }
    }

    #[test]
    fn empty_tree_flattens_to_no_panes() {
        let view = build_tree_view(&snapshot(None, 0, None));
        assert!(view.panes.is_empty());
        assert!(view.dividers.is_empty());
        assert_eq!(7, view.version);
    }

    #[test]
    fn single_pane_fills_the_bounds_and_carries_focus() {
        let view = build_tree_view(&snapshot(Some(pane(1)), 1, None));
        assert_eq!(1, view.panes.len());
        let p = &view.panes[0];
        assert_eq!(
            RectView {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            },
            p.rect
        );
        assert!(p.focused);
        assert!(!p.zoomed);
        assert!(view.dividers.is_empty());
    }

    #[test]
    fn vertical_branch_yields_two_side_by_side_panes_and_one_divider() {
        let root = Arc::new(SplitNode::Branch(SplitBranch {
            id: BranchId(1),
            orientation: Orientation::Vertical,
            first: pane(1),
            second: pane(2),
            divider_position: 0.5,
        }));
        let view = build_tree_view(&snapshot(Some(root), 2, Some(2)));

        assert_eq!(2, view.panes.len());
        let left = view.panes.iter().find(|p| p.pane_id == 1).unwrap();
        let right = view.panes.iter().find(|p| p.pane_id == 2).unwrap();
        assert_eq!(
            RectView {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 1.0
            },
            left.rect
        );
        assert_eq!(
            RectView {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 1.0
            },
            right.rect
        );

        assert_eq!(1, view.dividers.len());
        assert_eq!("vertical", view.dividers[0].orientation);
        assert_eq!(BranchId(1).0, view.dividers[0].branch_id);

        // Zoom + focus flags surface on the right pane.
        assert!(right.focused && right.zoomed);
        assert!(!left.focused && !left.zoomed);
        assert_eq!(Some(2), view.zoomed_pane);
    }
}
